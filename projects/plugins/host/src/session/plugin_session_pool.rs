use super::plugin_session::PluginSession;
use crate::error::PluginError;
use crate::host::{HostState, SessionPolicy, types};
use crate::wkg::is_wkg_reference;
use crate::world::PluginWorld;
use doup_config::repo::DoupRepoPlugin;
use std::{
  any::TypeId,
  collections::HashMap,
  path::{Path, PathBuf},
  sync::{Arc, LazyLock, Mutex, OnceLock, PoisonError},
};
use wasmtime::Config;
use wasmtime::component::{Accessor, Linker};
use wasmtime::{Engine, Store};

type Result<T, E = PluginError> = std::result::Result<T, E>;

/// Fallback engine for pools that were not given one. Engines are expensive and meant to be shared.
static DEFAULT_ENGINE: OnceLock<Engine> = OnceLock::new();

/// Linkers are kept here per world as it's possible different worlds may have slightly different requirements.
/// [`PluginWorld::linker`] provides the default, worlds can override if they wish.
static WORLD_LINKERS: LazyLock<Mutex<HashMap<TypeId, Arc<Linker<HostState>>>>> = LazyLock::new(Default::default);

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
///   pub async fn hello_world(&self) -> Result<String, PluginError> {
///     self.session_pool
///       .call_export("hello-world", self.session_pool.default_policy(), async |b, a| b.call_hello_world(a).await)
///       .await
///   }
/// }
/// ```
pub struct PluginSessionPool<W: PluginWorld> {
  /// The engine supplied at construction, when `None` the process wide default is used.
  engine: Option<Engine>,

  /// The plugin reference & its config from the repo's doup config.
  plugin: DoupRepoPlugin,

  /// The compiled component, built on first use & shared by every session.
  pre: OnceLock<W::Pre>,

  /// The plugin's config serialised as JSON, built on first use as every session is configured with the same value.
  plugin_config: OnceLock<String>,

  /// Idle, already configured sessions waiting to be checked out.
  session_pool: Mutex<Vec<PluginSession<W>>>,

  /// Extra dirs searched for a relative plugin path, the current working dir is always searched first.
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
      plugin_config: OnceLock::new(),
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
    let engine = Engine::new(&config).map_err(|err| PluginError::Engine(err.into_boxed_dyn_error()))?;
    Ok(DEFAULT_ENGINE.get_or_init(|| engine))
  }

  /// Creates and caches the linker for the given world.
  fn world_linker(&self) -> Result<Arc<Linker<HostState>>> {
    let mut map = WORLD_LINKERS.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(linker) = map.get(&TypeId::of::<W>()) {
      return Ok(linker.clone());
    }

    let linker = Arc::new(W::linker(self.engine()?).map_err(|err| PluginError::Linker(err.into_boxed_dyn_error()))?);
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
      return Err(PluginError::NotFound { plugin: plugin_ref.clone(), searched: vec![path.to_path_buf()] });
    }

    let mut searched = vec![std::env::current_dir().map_err(PluginError::CurrentDir)?.join(path)];
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
      return Err(PluginError::WkgUnsupported { plugin: plugin_ref.clone() });
    }

    Err(PluginError::NotFound { plugin: plugin_ref.clone(), searched })
  }

  // Pre-compiles the component & caches the result.
  // Subsequent sessions will re-use this instance.
  fn component_pre(&self) -> Result<&W::Pre> {
    if let Some(pre) = self.pre.get() {
      return Ok(pre);
    }

    let linker = self.world_linker()?;
    let path = self.module_path()?;
    let pre = W::pre(self.engine()?, &linker, &path)
      .map_err(|err| PluginError::Load { path, source: err.into_boxed_dyn_error() })?;
    Ok(self.pre.get_or_init(|| pre))
  }

  /// The policy sessions of this world start from, see [`PluginWorld::default_policy`].
  /// Build on it to grant a call more, eg: `pool.default_policy().http(HttpPolicy::Allowed)`.
  pub fn default_policy(&self) -> SessionPolicy {
    W::default_policy()
  }

  /// The plugin's config as JSON, serialised once & cached for the lifetime of the pool.
  fn plugin_config(&self) -> Result<&str> {
    if let Some(config) = self.plugin_config.get() {
      return Ok(config);
    }
    let config = serde_json::to_string(&self.plugin.with.clone().unwrap_or_default())
      .map_err(|source| PluginError::SerializeConfig { plugin: self.plugin.plugin.clone(), source })?;
    Ok(self.plugin_config.get_or_init(|| config))
  }

  /// Creates a new session which includes the bindings and their Store.
  /// Ensures that the new Store is configured by calling the `configure` export
  /// & passing along any plugin config from the original [`DoupRepoPlugin`] reference.
  ///
  /// This method may be called directly, by-passing the pool, for cases where
  /// custom WASI context overrides need to be given. Do not attempt to check
  /// the resulting session into the pool, just drop it.
  ///
  /// The `configure` export always runs with no I/O whatsoever (no HTTP, containers, stdio or files)
  /// so it can only parse & store its config, `policy` is only applied once it has returned.
  ///
  /// `policy` decides the guest's WASI context and which capabilities beyond WASI (eg: outbound
  /// `wasi:http` requests) it is granted. It is used exactly as given, so it should usually be built
  /// on [`Self::default_policy`]. Pooled sessions use the default as is, see [`Self::checkout_session`].
  pub async fn create_session(&self, policy: SessionPolicy) -> Result<PluginSession<W>> {
    let plugin = || self.plugin.plugin.clone();
    let state = HostState::new(policy)
      .map_err(|err| PluginError::Sandbox { plugin: plugin(), source: err.into_boxed_dyn_error() })?;
    let mut store = Store::new(self.engine()?, state);
    let bindings = W::instantiate(self.component_pre()?, &mut store)
      .await
      .map_err(|err| PluginError::Instantiate { plugin: plugin(), source: err.into_boxed_dyn_error() })?;

    let plugin_config = self.plugin_config()?;
    W::configure(&bindings, &mut store, plugin_config)
      .await
      .map_err(|err| self.trap("configure", err))?
      .map_err(|err| PluginError::Configure { plugin: plugin(), source: err.into() })?;

    // Only now that the guest is configured is it given the I/O its policy describes.
    store.data_mut().grant();

    Ok(PluginSession { store, bindings })
  }

  /// Calls the `async func` export named `export` (its WIT name, used in errors) on a new session
  /// created with `policy`, see [`Self::create_session`]. The session is dropped afterwards.
  ///
  /// ```ignore
  /// let policy = self.session_pool.default_policy().http(HttpPolicy::Allowed);
  /// self.session_pool.call_export("my-export", policy, async |b, a| b.call_my_export(a).await).await
  /// ```
  ///
  /// # Errors
  ///
  /// Returns [`PluginError::Trap`] if the guest traps, [`PluginError::Guest`] if it returns an
  /// error, or any error from [`Self::create_session`].
  pub async fn call_export<R>(
    &self,
    export: &'static str,
    policy: SessionPolicy,
    call: impl AsyncFnOnce(&W::Bindings, &Accessor<HostState>) -> wasmtime::Result<Result<R, types::GuestError>>,
  ) -> Result<R> {
    let mut session = self.create_session(policy).await?;
    session.call_async(call).await.map_err(|err| self.trap(export, err))?.map_err(|err| self.guest_error(export, err))
  }

  /// Like [`Self::call_export`], but runs on a pooled session (see [`Self::checkout_session`]) that is
  /// returned to the pool afterwards, so the session is reused by later calls instead of being dropped.
  /// A guest returning an error leaves the session healthy so it's still returned, a trap drops it.
  ///
  /// Pooled sessions only get the world's [`Self::default_policy`], so this suits worlds whose calls
  /// all need the same capabilities.
  ///
  /// ```ignore
  /// self.session_pool.call_pooled_export("my-export", async |b, a| b.call_my_export(a).await).await
  /// ```
  ///
  /// # Errors
  ///
  /// Returns [`PluginError::Trap`] if the guest traps, [`PluginError::Guest`] if it returns an
  /// error, or any error from [`Self::checkout_session`].
  pub async fn call_pooled_export<R>(
    &self,
    export: &'static str,
    call: impl AsyncFnOnce(&W::Bindings, &Accessor<HostState>) -> wasmtime::Result<Result<R, types::GuestError>>,
  ) -> Result<R> {
    let mut session = self.checkout_session().await?;
    let result = session.call_async(call).await.map_err(|err| self.trap(export, err))?;
    self.checkin_session(session);
    result.map_err(|err| self.guest_error(export, err))
  }

  /// Wraps an error returned by the guest's `export` as a [`PluginError::Guest`].
  fn guest_error(&self, export: &'static str, err: types::GuestError) -> PluginError {
    PluginError::Guest { plugin: self.plugin.plugin.clone(), export, source: err.into() }
  }

  /// Wraps a wasmtime level failure of `export` as a [`PluginError::Trap`].
  fn trap(&self, export: &'static str, err: wasmtime::Error) -> PluginError {
    PluginError::Trap { plugin: self.plugin.plugin.clone(), export, source: err.into_boxed_dyn_error() }
  }

  /// Takes an idle session, or creates and configures a new one. Hand it back with [`Self::checkin_session`].
  /// A session that is never checked in (error, or the caller's future cancelled) is simply dropped.
  ///
  /// Pooled sessions are created with the world's [`Self::default_policy`], use [`Self::create_session`]
  /// (or [`Self::call_export`]) for calls that need more, eg: HTTP or container access. See also
  /// [`Self::call_pooled_export`] which handles the check out & in around a call.
  pub async fn checkout_session(&self) -> Result<PluginSession<W>> {
    let session = self.session_pool.lock().unwrap_or_else(PoisonError::into_inner).pop();
    match session {
      Some(session) => Ok(session),
      None => self.create_session(self.default_policy()).await,
    }
  }

  /// Returns a healthy session to the pool. A session whose call trapped must be dropped instead.
  pub fn checkin_session(&self, session: PluginSession<W>) {
    self.session_pool.lock().unwrap_or_else(PoisonError::into_inner).push(session);
  }
}
