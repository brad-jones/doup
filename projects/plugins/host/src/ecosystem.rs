//! Host-side support for `ecosystem` WASM plugins.
//!
//! An ecosystem plugin implements the `ecosystem` world defined in the WIT
//! package (`../wit`) and teaches doup about a package ecosystem. This module
//! generates the component bindings for that world and exposes
//! [`EcosystemPlugin`], a handle that runs calls against pooled plugin sessions.

use crate::host::HostState;
use crate::session::{PluginSessionPool, PluginWorld};
use anyhow::Result;
use doup_config::repo::DoupRepoPlugin;
use std::path::{Path, PathBuf};
use wasmtime::{
  Engine, Store,
  component::{Component, Linker, bindgen},
};

bindgen!({
  path: "../wit",
  world: "ecosystem",
  exports: { default: async },
  with: { "doup:plugin/host-api": super::host::host_api },
});

/// Marker type binding the `ecosystem` WIT world to the generic session pool.
struct EcosystemWorld;

impl PluginWorld for EcosystemWorld {
  type Pre = EcosystemPre<HostState>;
  type Bindings = Ecosystem;

  fn pre(engine: &Engine, linker: &Linker<HostState>, path: &Path) -> Result<Self::Pre> {
    let component = Component::from_file(engine, path)?;
    let pre = EcosystemPre::new(linker.instantiate_pre(&component)?)?;
    Ok(pre)
  }

  async fn instantiate(pre: &Self::Pre, store: &mut Store<HostState>) -> Result<Self::Bindings> {
    Ok(pre.instantiate_async(store).await?)
  }

  async fn configure(
    bindings: &Self::Bindings,
    store: &mut Store<HostState>,
    config: &str,
  ) -> wasmtime::Result<Result<(), String>> {
    bindings.call_configure(store, config).await
  }
}

/// A loaded ecosystem plugin.
///
/// Each call checks a session out of an internal pool, invokes the plugin
/// export, and returns the session to the pool.
pub struct EcosystemPlugin {
  session_pool: PluginSessionPool<EcosystemWorld>,
}

impl EcosystemPlugin {
  /// Creates a plugin handle for `plugin`, resolving its module from `module_search_dirs`.
  pub fn new(plugin: &DoupRepoPlugin, module_search_dirs: Vec<PathBuf>) -> Self {
    Self {
      session_pool: PluginSessionPool::new(None, plugin, module_search_dirs),
    }
  }

  /// Like [`Self::new`], but eagerly checks a session out and straight back in.
  ///
  /// This exercises the plugin (compilation, instantiation and configuration) so that
  /// failures surface here rather than during real work. The warmed session stays pooled.
  ///
  /// # Errors
  ///
  /// Returns an error if the plugin cannot be loaded, instantiated or configured.
  pub async fn load(plugin: &DoupRepoPlugin, module_search_dirs: Vec<PathBuf>) -> Result<Self> {
    let instance = Self::new(plugin, module_search_dirs);
    let session = instance.session_pool.checkout_session().await?;
    instance.session_pool.checkin_session(session, Ok(()))?;
    Ok(instance)
  }

  /// Calls the plugin's `discover-deps` export.
  ///
  /// # Errors
  ///
  /// Returns an error if a session cannot be checked out, the call traps, or the plugin
  /// itself reports an error.
  pub async fn discover_deps(&self) -> Result<Vec<Dependency>> {
    let mut session = self.session_pool.checkout_session().await?;
    let result = session.bindings.call_discover_deps(&mut session.store).await;
    self
      .session_pool
      .checkin_session(session, result)?
      .map_err(anyhow::Error::msg)
  }
}
