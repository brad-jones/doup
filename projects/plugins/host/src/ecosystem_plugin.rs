//! Host-side support for `ecosystem` WASM plugins.
//!
//! An ecosystem plugin implements the `ecosystem` world defined in the WIT
//! package (`../wit`) and teaches doup about a package ecosystem. This module
//! generates the component bindings for that world and exposes
//! [`EcosystemPlugin`], a handle that runs calls against pooled plugin sessions.

use crate::host::{ContainerMounts, ContainerPolicy, HostState, HttpPolicy, SessionPolicy};
use crate::session::PluginSessionPool;
use crate::world::PluginWorld;
use anyhow::Result;
use doup_config::repo::DoupRepoPlugin;
use std::path::{Path, PathBuf};
use testcontainers::core::AccessMode;
use wasmtime::component::{Component, Linker, bindgen};
use wasmtime::{Engine, Store};
use wasmtime_wasi::FsPerms;

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
      session_pool: PluginSessionPool::new(None, plugin, module_search_dirs), //
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

  /// Lists the dependencies found in `workspace_dir`.
  /// The guest gets a read-only view of the workspace.
  pub async fn discover_deps(&self, workspace_dir: &Path) -> Result<Vec<Dependency>> {
    let workspace = workspace_dir.to_path_buf();
    let policy =
      SessionPolicy::new().wasi(move |b| Ok(b.preopened_dir(&workspace, "/workspace", FsPerms::ReadOnly)?));
    let mut session = self.session_pool.create_session(policy).await?;
    let result = session.call_async(async |b, a| b.call_discover_deps(a).await).await?;
    result.map_err(anyhow::Error::msg)
  }

  /// Looks up the versions available for `dependency`.
  /// The guest is able to make HTTP calls but no other I/O is granted.
  pub async fn lookup_versions(&self, dependency: &Dependency) -> Result<Option<Versions>> {
    let policy = SessionPolicy::new().http(HttpPolicy::Allowed);
    let mut session = self.session_pool.create_session(policy).await?;
    let dependency = dependency.clone();
    let result = session.call_async(async |b, a| b.call_lookup_versions(a, dependency).await).await?;
    result.map_err(anyhow::Error::msg)
  }

  /// Downloads release notes for for `dependency`.
  /// The guest is able to make HTTP calls but no other I/O is granted.
  pub async fn fetch_release_notes(&self, dependency: &Dependency, to: &str) -> Result<Vec<ReleaseNote>> {
    let policy = SessionPolicy::new().http(HttpPolicy::Allowed);
    let mut session = self.session_pool.create_session(policy).await?;
    let dependency = dependency.clone();
    let to = to.to_string();
    let result = session.call_async(async |b, a| b.call_fetch_release_notes(a, dependency, to).await).await?;
    result.map_err(anyhow::Error::msg)
  }

  /// Updates every occurrence of `dependency` found in `workspace_dir` to version `to`.
  ///
  /// The guest gets a read-write view of the workspace, is able to make HTTP calls & may run containers.
  /// It also gets an empty, read-write `/container-init` dir that is mounted into those containers,
  /// keeping the guest's scripts, config, etc out of the workspace. It's deleted after the call.
  pub async fn apply_updates(&self, workspace_dir: &Path, dependency: &Dependency, to: &str) -> Result<()> {
    let workspace_dir = workspace_dir.to_path_buf();

    // Must be bound to a variable, not chained inline: the dir is deleted when the `TempDir` is dropped,
    // so it has to outlive the session. The policy closure only gets a copy of the path.
    let init_tmp = tempfile::Builder::new().prefix("doup-container-init-").tempdir()?;
    let init_dir = init_tmp.path().to_path_buf();

    let policy = SessionPolicy::new()
      .http(HttpPolicy::Allowed)
      .containers(ContainerPolicy::Allowed(ContainerMounts {
        workspace: workspace_dir.clone(),
        workspace_access: AccessMode::ReadWrite,
        init: init_dir.clone(),
        init_access: AccessMode::ReadOnly,
      }))
      .wasi(move |b| {
        Ok(
          b //
            .preopened_dir(&workspace_dir, "/workspace", FsPerms::ReadWrite)?
            .preopened_dir(&init_dir, "/container-init", FsPerms::ReadWrite)?,
        )
      });

    let mut session = self.session_pool.create_session(policy).await?;
    let dependency = dependency.clone();
    let to = to.to_string();
    let result = session.call_async(async |b, a| b.call_apply_updates(a, dependency, to).await).await?;
    result.map_err(anyhow::Error::msg)
  }
}
