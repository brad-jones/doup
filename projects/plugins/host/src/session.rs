//! Logic shared by every plugin world: lazily compiling the plugin, pooling instances and
//! configuring each new instance. A world only supplies the bits that depend on its generated
//! bindings by implementing [`World`].

use super::host::HostState;
use super::plugin_config_json;
use super::resolve::resolve_plugin_path;
use super::runtime::PluginRuntime;
use anyhow::{Context, Result};
use doup_config::repo::DoupRepoPlugin;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::OnceCell;
use wasmtime::Store;
use wasmtime_wasi::WasiCtxBuilder;

/// What differs between plugin worlds. Implemented by a marker type per world (eg: `EcosystemWorld`).
pub(crate) trait World: 'static {
  /// The compiled plugin, cheap to instantiate many times (eg: `EcosystemPre<HostState>`).
  type Pre: Send + Sync;

  /// The generated bindings for one instance (eg: `Ecosystem`).
  type Bindings: Send;

  fn pre(runtime: &PluginRuntime, wasm_path: &Path) -> Result<Self::Pre>;

  fn instantiate(pre: &Self::Pre, store: &mut Store<HostState>) -> impl Future<Output = Result<Self::Bindings>> + Send;

  /// Calls the world's `configure` export. The inner `Err` is the plugin rejecting its config.
  fn configure(
    bindings: &Self::Bindings,
    store: &mut Store<HostState>,
    config: &str,
  ) -> impl Future<Output = wasmtime::Result<Result<(), String>>> + Send;

  /// Override to give a world different WASI permissions.
  fn wasi(builder: &mut WasiCtxBuilder) {
    builder.inherit_stdout().inherit_stderr();
  }
}

/// One plugin instance. Exclusive access to the `Store` is what makes concurrent calls need a pool.
pub(crate) struct Session<W: World> {
  pub store: Store<HostState>,
  pub bindings: W::Bindings,
}

/// Hands out configured instances of a plugin, creating more whenever none are idle.
pub(crate) struct SessionPool<W: World> {
  runtime: Arc<PluginRuntime>,
  plugin: DoupRepoPlugin,
  search_dirs: Vec<PathBuf>,
  pre: OnceCell<W::Pre>,
  idle: Mutex<Vec<Session<W>>>,
}

impl<W: World> SessionPool<W> {
  /// Does no I/O; the plugin is only located and compiled on first use.
  pub fn new(runtime: &Arc<PluginRuntime>, plugin: &DoupRepoPlugin, search_dirs: Vec<PathBuf>) -> Self {
    Self {
      runtime: Arc::clone(runtime),
      plugin: plugin.clone(),
      search_dirs,
      pre: OnceCell::new(),
      idle: Mutex::new(Vec::new()),
    }
  }

  /// Takes an idle session, or creates and configures a new one. Hand it back with [`Self::checkin`].
  /// A session that is never checked in (error, or the caller's future cancelled) is simply dropped.
  pub async fn checkout(&self) -> Result<Session<W>> {
    let idle = self.idle.lock().unwrap_or_else(PoisonError::into_inner).pop();
    match idle {
      Some(session) => Ok(session),
      None => self.create().await,
    }
  }

  /// Returns the session to the pool if the call succeeded, and unwraps the call's result.
  pub fn checkin<R>(&self, session: Session<W>, result: wasmtime::Result<R>) -> Result<R> {
    // An Err is a trap or host failure, so the session is dropped rather than reused.
    let value = result?;
    // A poisoned lock still holds a valid Vec, so keep going.
    self.idle.lock().unwrap_or_else(PoisonError::into_inner).push(session);
    Ok(value)
  }

  async fn create(&self) -> Result<Session<W>> {
    let pre = self.pre().await?;
    let mut store = Store::new(&self.runtime.engine, HostState::new(W::wasi));
    let bindings = W::instantiate(pre, &mut store).await?;

    // Every instance has its own guest memory, so every instance must be configured.
    W::configure(&bindings, &mut store, &plugin_config_json(&self.plugin)?)
      .await?
      .map_err(anyhow::Error::msg)
      .with_context(|| format!("plugin {} rejected its config", self.plugin.plugin))?;

    Ok(Session { store, bindings })
  }

  async fn pre(&self) -> Result<&W::Pre> {
    self
      .pre
      .get_or_try_init(|| async {
        // TODO: pass every search dir once `resolve_plugin_path` accepts them.
        let wasm_path = resolve_plugin_path(&self.plugin.plugin, self.search_dirs.first().map(PathBuf::as_path))?;
        W::pre(&self.runtime, &wasm_path)
      })
      .await
  }
}
