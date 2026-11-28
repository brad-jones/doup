//! Host-side support for `ecosystem` WASM plugins.
//!
//! An ecosystem plugin implements the `ecosystem` world defined in the WIT
//! package (`../wit`) and teaches doup about a package ecosystem. This module
//! generates the component bindings for that world and exposes
//! [`EcosystemPlugin`], a handle that runs calls against pooled plugin sessions.

use crate::error::PluginError;
use crate::host::{ContainerMounts, ContainerPolicy, HostState, HttpPolicy};
use crate::session::PluginSessionPool;
use crate::world::PluginWorld;
use doup_config::repo::DoupRepoPlugin;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use tempfile::TempDir;
use testcontainers::core::AccessMode;
use wasmtime::component::{Component, Linker, bindgen};
use wasmtime::{Engine, Store};
use wasmtime_wasi::FsPerms;

bindgen!({
  path: "../wit",
  world: "ecosystem",
  exports: { default: async },
  with: {
    "doup:plugin/host-api": super::host::host_api,
    "doup:plugin/types": super::host::types,
  },
});

/// Marker type binding the `ecosystem` WIT world to the generic session pool.
struct EcosystemWorld;

impl PluginWorld for EcosystemWorld {
  type Pre = EcosystemPre<HostState>;
  type Bindings = Ecosystem;

  fn pre(engine: &Engine, linker: &Linker<HostState>, path: &Path) -> wasmtime::Result<Self::Pre> {
    let component = Component::from_file(engine, path)?;
    let pre = EcosystemPre::new(linker.instantiate_pre(&component)?)?;
    Ok(pre)
  }

  async fn instantiate(pre: &Self::Pre, store: &mut Store<HostState>) -> wasmtime::Result<Self::Bindings> {
    pre.instantiate_async(store).await
  }

  async fn configure(
    bindings: &Self::Bindings,
    store: &mut Store<HostState>,
    config: &str,
  ) -> wasmtime::Result<Result<(), GuestError>> {
    bindings.call_configure(store, config).await
  }
}

/// A loaded ecosystem plugin.
///
/// Each call checks a session out of an internal pool, invokes the plugin
/// export, and returns the session to the pool.
pub struct EcosystemPlugin {
  session_pool: PluginSessionPool<EcosystemWorld>,

  /// Backs the `/container-cache` dir of every container run by this plugin, created on first use.
  /// It's deleted along with the plugin.
  container_cache: Mutex<Option<TempDir>>,
}

impl EcosystemPlugin {
  /// Creates a plugin handle for `plugin`, resolving its module from `module_search_dirs`.
  pub fn new(plugin: &DoupRepoPlugin, module_search_dirs: Vec<PathBuf>) -> Self {
    Self {
      session_pool: PluginSessionPool::new(None, plugin, module_search_dirs), //
      container_cache: Mutex::new(None),
    }
  }

  /// The host path of the `/container-cache` dir, created if it doesn't exist yet.
  fn container_cache_dir(&self) -> Result<PathBuf, PluginError> {
    let mut cache = self.container_cache.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(dir) = cache.as_ref() {
      return Ok(dir.path().to_path_buf());
    }
    let dir = tempfile::Builder::new().prefix("doup-container-cache-").tempdir().map_err(PluginError::TempDir)?;
    let path = dir.path().to_path_buf();
    *cache = Some(dir);
    Ok(path)
  }

  /// Like [`Self::new`], but eagerly checks a session out and straight back in.
  ///
  /// This exercises the plugin (compilation, instantiation and configuration) so that
  /// failures surface here rather than during real work. The warmed session stays pooled.
  ///
  /// # Errors
  ///
  /// Returns an error if the plugin cannot be loaded, instantiated or configured.
  pub async fn load(plugin: &DoupRepoPlugin, module_search_dirs: Vec<PathBuf>) -> Result<Self, PluginError> {
    let instance = Self::new(plugin, module_search_dirs);
    let session = instance.session_pool.checkout_session().await?;
    instance.session_pool.checkin_session(session);
    Ok(instance)
  }

  /// Lists the dependencies found in `workspace_dir`.
  /// The guest gets a read-only view of the workspace.
  pub async fn discover_deps(&self, workspace_dir: &Path) -> Result<Vec<Dependency>, PluginError> {
    let workspace = workspace_dir.to_path_buf();
    let policy =
      self.session_pool.default_policy().wasi(move |b| b.preopened_dir(&workspace, "/workspace", FsPerms::ReadOnly));
    self.session_pool.call_export("discover-deps", policy, async |b, a| b.call_discover_deps(a).await).await
  }

  /// Looks up the versions available for `dependency`.
  /// The guest is able to make HTTP calls but no other I/O is granted.
  pub async fn lookup_versions(&self, dependency: &Dependency) -> Result<Option<Versions>, PluginError> {
    let policy = self.session_pool.default_policy().http(HttpPolicy::Allowed);
    let dependency = dependency.clone();
    self
      .session_pool
      .call_export("lookup-versions", policy, async |b, a| b.call_lookup_versions(a, dependency).await)
      .await
  }

  /// Downloads release notes for for `dependency`.
  /// The guest is able to make HTTP calls but no other I/O is granted.
  pub async fn fetch_release_notes(&self, dependency: &Dependency, to: &str) -> Result<Vec<ReleaseNote>, PluginError> {
    let policy = self.session_pool.default_policy().http(HttpPolicy::Allowed);
    let dependency = dependency.clone();
    let to = to.to_string();
    self
      .session_pool
      .call_export("fetch-release-notes", policy, async |b, a| b.call_fetch_release_notes(a, dependency, to).await)
      .await
  }

  /// Updates every occurrence of `dependency` found in `workspace_dir` to version `to`.
  ///
  /// The guest gets a read-write view of the workspace, is able to make HTTP calls & may run containers.
  /// It also gets an empty, read-write `/container-init` dir that is mounted into those containers,
  /// keeping the guest's scripts, config, etc out of the workspace. It's deleted after the call.
  ///
  /// Containers also get a read-write `/container-cache` dir that the guest can't see. It lives as long as
  /// this [`EcosystemPlugin`], so is shared by every call, letting the tooling in the containers keep its
  /// caches (eg: downloaded packages) between runs. The host doesn't know or care what is stored in it.
  pub async fn apply_updates(
    &self,
    workspace_dir: &Path,
    dependency: &Dependency,
    to: &str,
  ) -> Result<(), PluginError> {
    let workspace_dir = workspace_dir.to_path_buf();
    let cache_dir = self.container_cache_dir()?;

    // Must be bound to a variable, not chained inline: the dir is deleted when the `TempDir` is dropped,
    // so it has to outlive the session. The policy closure only gets a copy of the path.
    let init_tmp = tempfile::Builder::new().prefix("doup-container-init-").tempdir().map_err(PluginError::TempDir)?;
    let init_dir = init_tmp.path().to_path_buf();

    let policy = self
      .session_pool
      .default_policy()
      .http(HttpPolicy::Allowed)
      .containers(ContainerPolicy::Allowed(ContainerMounts {
        workspace: workspace_dir.clone(),
        workspace_access: AccessMode::ReadWrite,
        init: init_dir.clone(),
        init_access: AccessMode::ReadOnly,
        cache: cache_dir,
      }))
      .wasi(move |b| {
        b //
          .preopened_dir(&workspace_dir, "/workspace", FsPerms::ReadWrite)?
          .preopened_dir(&init_dir, "/container-init", FsPerms::ReadWrite)
      });

    let dependency = dependency.clone();
    let to = to.to_string();
    self
      .session_pool
      .call_export("apply-updates", policy, async |b, a| b.call_apply_updates(a, dependency, to).await)
      .await
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn plugin(reference: &str) -> DoupRepoPlugin {
    DoupRepoPlugin { plugin: reference.to_string(), with: None }
  }

  #[tokio::test]
  async fn missing_plugins_report_everywhere_searched() {
    let dir = tempfile::tempdir().unwrap();
    let Err(err) = EcosystemPlugin::load(&plugin("missing.wasm"), vec![dir.path().to_path_buf()]).await else {
      panic!("expected the plugin to be missing");
    };
    let PluginError::NotFound { plugin, searched } = &err else { panic!("unexpected error: {err}") };
    assert_eq!(plugin, "missing.wasm");
    assert_eq!(searched.last(), Some(&dir.path().join("missing.wasm")));
  }

  #[tokio::test]
  async fn wkg_references_are_not_supported_yet() {
    let Err(err) = EcosystemPlugin::load(&plugin("doup:cargo@1.0.0"), Vec::new()).await else {
      panic!("expected wkg references to be rejected");
    };
    assert!(matches!(&err, PluginError::WkgUnsupported { plugin } if plugin == "doup:cargo@1.0.0"), "{err}");
  }
}
