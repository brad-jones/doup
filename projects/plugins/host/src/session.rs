//! Plugin session management for the WASM plugin host.
//!
//! A wasmtime `Store` holds a guest's memory and can only be used by one caller at a time, so
//! concurrent calls into a plugin need separate instances. This module provides:
//!
//! - [`PluginWorld`]: describes what differs between plugin worlds.
//! - [`PluginSession`]: a set of bindings paired with the `Store` that backs them.
//! - [`PluginSessionPool`]: resolves and pre-compiles the plugin component once, then hands out
//!   idle sessions via [`PluginSessionPool::checkout_session`] (creating and configuring new ones
//!   on demand) and takes them back via [`PluginSessionPool::checkin_session`].
//!
//! Sessions that trap or fail are dropped rather than returned to the pool.

use crate::host::{HostState, HttpPolicy, host_api};
use crate::wkg::is_wkg_reference;
use anyhow::{Context, Result, bail};
use doup_config::repo::DoupRepoPlugin;
use std::{
  any::TypeId,
  collections::HashMap,
  path::{Path, PathBuf},
  sync::{Arc, LazyLock, Mutex, OnceLock, PoisonError},
};
use wasmtime::Config;
use wasmtime::{
  Engine, Store,
  component::{HasSelf, Linker},
};
use wasmtime_wasi::{WasiCtxBuilder, p2::add_to_linker_async};
use wasmtime_wasi_http::p2::add_only_http_to_linker_async;

/// Fallback engine for pools that were not given one. Engines are expensive and meant to be shared.
static DEFAULT_ENGINE: OnceLock<Engine> = OnceLock::new();

/// Linkers are kept here per world as it's possible different worlds may have slightly different requirements.
/// [`PluginWorld::linker`] provides the default, worlds can override if they wish.
static WORLD_LINKERS: LazyLock<Mutex<HashMap<TypeId, Arc<Linker<HostState>>>>> = LazyLock::new(Default::default);

/// What differs between plugin worlds. Implemented by a marker type per world.
/// See EcosystemWorld & PlatformWorld for the two current implemented worlds.
pub trait PluginWorld: 'static {
  /// The compiled plugin, cheap to instantiate many times (eg: `EcosystemPre<HostState>`).
  type Pre;

  /// The generated bindings for one instance (eg: `Ecosystem`).
  type Bindings;

  /// Factory for [`World::Pre`], typical implementation:
  ///
  /// ```ignore
  /// fn pre(engine: &Engine, linker: &Linker<HostState>, path: &Path) -> Result<Self::Pre> {
  ///   let component = Component::from_file(engine, path)?;
  ///   let pre = {{WORLD_NAME}}Pre::new(linker.instantiate_pre(&component)?)?;
  ///   Ok(pre)
  /// }
  /// ```
  fn pre(engine: &Engine, linker: &Linker<HostState>, path: &Path) -> Result<Self::Pre>;

  /// Factory for [`World::Bindings`], typical implementation:
  ///
  /// ```ignore
  /// async fn instantiate(pre: &Self::Pre, store: &mut Store<HostState>) -> Result<Self::Bindings> {
  ///   Ok(pre.instantiate_async(store).await?)
  /// }
  /// ```
  fn instantiate(pre: &Self::Pre, store: &mut Store<HostState>) -> impl Future<Output = Result<Self::Bindings>> + Send;

  /// Calls the world's `configure` export as part of session creation so
  /// that a guest component is always configured before being used.
  ///
  /// Typical implementation:
  ///
  /// ```ignore
  /// async fn configure(
  ///   bindings: &Self::Bindings,
  ///   store: &mut Store<HostState>,
  ///   config: &str,
  /// ) -> wasmtime::Result<Result<(), String>> {
  ///   bindings.call_configure(store, config).await
  /// }
  /// ```
  fn configure(
    bindings: &Self::Bindings,
    store: &mut Store<HostState>,
    config: &str,
  ) -> impl Future<Output = wasmtime::Result<Result<(), String>>> + Send;

  /// The default linker that will be used, worlds are free to override.
  /// The result of this method is cached into [`WORLD_LINKERS`].
  ///
  /// `wasi:http` is always linked because a component that imports it can't be instantiated
  /// otherwise. Whether requests actually go out is decided per session by [`Http`].
  fn linker(engine: &Engine) -> Result<Linker<HostState>> {
    let mut linker = Linker::new(engine);
    add_to_linker_async(&mut linker)?;
    add_only_http_to_linker_async(&mut linker)?;
    host_api::add_to_linker::<HostState, HasSelf<HostState>>(&mut linker, |state| state)?;
    Ok(linker)
  }

  /// Override to give a world different WASI permissions.
  fn wasi(builder: &mut WasiCtxBuilder) -> Result<&mut WasiCtxBuilder> {
    Ok(builder.inherit_stdout().inherit_stderr())
  }
}

/// A session is the combination of a set of bindings and their store.
/// Exclusive access to the `Store` is what makes concurrent calls need a pool.
///
/// The `Store` is what holds the guests memory space, so anytime a new store
/// is created, the world's `configure` export needs to be called.
///
/// [`PluginSessionPool`] ensures this happen anytime a session is checked out of the pool.
pub struct PluginSession<W: PluginWorld> {
  pub store: Store<HostState>,
  pub bindings: W::Bindings,
}

/// Hands out [`PluginSession`] instances, creating more whenever none are idle.
///
/// Typical usage:
///
/// ```ignore
/// pub struct MyPlugin {
///   session_pool: PluginSessionPool<MyWorld>,
/// }
///
/// impl MyPlugin {
///   pub fn new(plugin: &DoupRepoPlugin, module_search_dirs: Vec<PathBuf>) -> Self {
///     Self {
///       session_pool: PluginSessionPool::new(None, plugin, module_search_dirs),
///     }
///   }
///
///   pub async fn hello_world(&self) -> Result<String> {
///     let mut session = self.session_pool.checkout_session().await?;
///     let result = session.bindings.call_hello_world(&mut session.store).await;
///     self.session_pool.checkin_session(session, result)
///   }
/// }
/// ```
pub struct PluginSessionPool<W: PluginWorld> {
  engine: Option<Engine>,
  plugin: DoupRepoPlugin,
  pre: OnceLock<W::Pre>,
  session_pool: Mutex<Vec<PluginSession<W>>>,
  module_search_dirs: Vec<PathBuf>,
}

impl<W: PluginWorld> PluginSessionPool<W> {
  /// Creates a new session pool
  ///
  /// - `engine`: An optional wasmtime [`Engine`] to share. When `None` a lazily created process wide default is used.
  /// - `plugin`: A reference to [`DoupRepoPlugin`] representing the plugin to load.
  /// - `module_search_dirs`: A list of additional dir's to prefix to a relative plugin path. _(the current working dir is always included)_
  pub fn new(engine: Option<Engine>, plugin: &DoupRepoPlugin, module_search_dirs: Vec<PathBuf>) -> Self {
    Self {
      engine,
      plugin: plugin.clone(),
      pre: OnceLock::new(),
      session_pool: Mutex::new(Vec::new()),
      module_search_dirs,
    }
  }

  /// The engine in use: the one supplied at construction, else the process wide default.
  fn engine(&self) -> Result<&Engine> {
    if let Some(engine) = &self.engine {
      return Ok(engine);
    }
    if let Some(engine) = DEFAULT_ENGINE.get() {
      return Ok(engine);
    }
    let mut config = Config::new();
    config.wasm_component_model(true);
    let engine = Engine::new(&config)?;
    Ok(DEFAULT_ENGINE.get_or_init(|| engine))
  }

  /// Creates and caches the linker for the given world.
  fn world_linker(&self) -> Result<Arc<Linker<HostState>>> {
    let mut map = WORLD_LINKERS.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(linker) = map.get(&TypeId::of::<W>()) {
      return Ok(linker.clone());
    }

    let linker = Arc::new(W::linker(self.engine()?)?);
    map.insert(TypeId::of::<W>(), linker.clone());
    Ok(linker)
  }

  /// Resolves the final path to a WASM file from a [`DoupRepoPlugin`] reference.
  fn module_path(&self) -> Result<PathBuf> {
    let plugin_ref = &self.plugin.plugin;
    let path = Path::new(plugin_ref);

    if path.is_absolute() {
      if path.is_file() {
        return Ok(path.to_path_buf());
      }
      bail!("plugin not found: {}", path.display());
    }

    let mut searched = vec![std::env::current_dir()?.join(path)];
    for candidate in self.module_search_dirs.iter().map(|dir| dir.join(path)) {
      if !searched.contains(&candidate) {
        searched.push(candidate);
      }
    }
    if let Some(found) = searched.iter().find(|candidate| candidate.is_file()) {
      return Ok(found.clone());
    }

    // TODO: Remember we also need to somehow handle doup.lock
    // when we start downloading packages from the internet
    if is_wkg_reference(plugin_ref) {
      bail!("wkg package references are not supported yet: {plugin_ref}");
    }

    bail!(
      "plugin not found: {} (searched: {})",
      path.display(),
      searched.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")
    )
  }

  // Pre-compiles the component & caches the result.
  // Subsequent sessions will re-use this instance.
  fn component_pre(&self) -> Result<&W::Pre> {
    if let Some(pre) = self.pre.get() {
      return Ok(pre);
    }

    let linker = self.world_linker()?;
    let pre = W::pre(self.engine()?, &linker, &self.module_path()?)?;
    Ok(self.pre.get_or_init(|| pre))
  }

  /// Creates a new session which includes the bindings and their Store.
  /// Ensures that the new Store is configured by calling the `configure` export
  /// & passing along any plugin config from the original [`DoupRepoPlugin`] reference.
  ///
  /// This method may be called directly, by-passing the pool, for cases where
  /// custom WASI context overrides need to be given. Do not attempt to check
  /// the resulting session into the pool, just drop it.
  ///
  /// `http_policy` decides whether the guest may make outbound `wasi:http` requests,
  /// pooled sessions never can, see [`Self::checkout_session`].
  pub async fn create_session(
    &self,
    http_policy: HttpPolicy,
    wasi: impl FnOnce(&mut WasiCtxBuilder) -> Result<&mut WasiCtxBuilder>,
  ) -> Result<PluginSession<W>> {
    let mut store = Store::new(self.engine()?, HostState::new(http_policy, |b| wasi(W::wasi(b)?))?);

    let bindings = W::instantiate(self.component_pre()?, &mut store).await?;

    let plugin_config = serde_json::to_string(&self.plugin.with.clone().unwrap_or_default())?;
    W::configure(&bindings, &mut store, &plugin_config)
      .await?
      .map_err(anyhow::Error::msg)
      .with_context(|| format!("plugin {} rejected its config", self.plugin.plugin))?;

    Ok(PluginSession { store, bindings })
  }

  /// Takes an idle session, or creates and configures a new one. Hand it back with [`Self::checkin_session`].
  /// A session that is never checked in (error, or the caller's future cancelled) is simply dropped.
  ///
  /// Pooled sessions have no HTTP access, use [`Self::create_session`] for calls that need it.
  pub async fn checkout_session(&self) -> Result<PluginSession<W>> {
    let session = self.session_pool.lock().unwrap_or_else(PoisonError::into_inner).pop();
    match session {
      Some(session) => Ok(session),
      None => self.create_session(HttpPolicy::Denied, |b| Ok(b)).await,
    }
  }

  /// Returns the session to the pool if the call succeeded, and unwraps the call's result.
  pub fn checkin_session<R>(&self, session: PluginSession<W>, result: wasmtime::Result<R>) -> Result<R> {
    // An Err is a trap or host failure, so the session is dropped rather than reused.
    let value = result?;

    // Check the session back into the pool
    self.session_pool.lock().unwrap_or_else(PoisonError::into_inner).push(session);

    // Hand back the inner result
    Ok(value)
  }
}
