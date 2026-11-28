//! Host-side support for `platform` WASM plugins.
//!
//! A platform plugin implements the `platform` world defined in the WIT
//! package (`../wit`) and teaches doup about a hosting platform. This module
//! generates the component bindings for that world and exposes
//! [`PlatformPlugin`], a handle that runs calls against pooled plugin sessions.

use crate::error::PluginError;
use crate::host::{HostState, HttpPolicy, SessionPolicy};
use crate::session::PluginSessionPool;
use crate::world::PluginWorld;
use doup_config::repo::DoupRepoPlugin;
use std::path::{Path, PathBuf};
use wasmtime::{
  Engine, Store,
  component::{Component, Linker, bindgen},
};

bindgen!({
  path: "../wit",
  world: "platform",
  exports: { default: async },
  with: {
    "doup:plugin/host-api": super::host::host_api,
    "doup:plugin/types": super::host::types,
  },
});

/// Marker type binding the `platform` WIT world to the generic session pool.
struct PlatformWorld;

impl PluginWorld for PlatformWorld {
  type Pre = PlatformPre<HostState>;
  type Bindings = Platform;

  fn pre(engine: &Engine, linker: &Linker<HostState>, path: &Path) -> wasmtime::Result<Self::Pre> {
    let component = Component::from_file(engine, path)?;
    let pre = PlatformPre::new(linker.instantiate_pre(&component)?)?;
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

  /// Every export talks to the platform's API, so all sessions may make HTTP calls. No other I/O is granted.
  fn default_policy() -> SessionPolicy {
    SessionPolicy::new().http(HttpPolicy::Allowed).wasi(|b| Ok(b.inherit_stdout().inherit_stderr()))
  }
}

/// A loaded platform plugin.
///
/// Each call checks a session out of an internal pool, invokes the plugin
/// export, and returns the session to the pool.
///
/// Every session is granted HTTP access, as every export talks to the platform's API, but no other I/O.
pub struct PlatformPlugin {
  session_pool: PluginSessionPool<PlatformWorld>,
}

impl PlatformPlugin {
  /// Creates a plugin handle for `plugin`, resolving its module from `module_search_dirs`.
  pub fn new(plugin: &DoupRepoPlugin, module_search_dirs: Vec<PathBuf>) -> Self {
    Self { session_pool: PluginSessionPool::new(None, plugin, module_search_dirs) }
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

  /// Opens a new pull request from `head_branch`, which must already have been pushed.
  pub async fn create_pr(&self, head_branch: &str, content: &PullRequestContent) -> Result<PullRequest, PluginError> {
    let head_branch = head_branch.to_string();
    let content = content.clone();
    self
      .session_pool
      .call_pooled_export("create-pr", async |b, a| b.call_create_pr(a, head_branch, content).await)
      .await
  }

  /// Replaces the base branch, title, body & version of the open pull request `number`.
  pub async fn update_pr(&self, number: i64, content: &PullRequestContent) -> Result<PullRequest, PluginError> {
    let content = content.clone();
    self.session_pool.call_pooled_export("update-pr", async |b, a| b.call_update_pr(a, number, content).await).await
  }

  /// Closes the pull request `number` without merging it, first posting `comment` to it when given.
  pub async fn close_pr(&self, number: i64, comment: Option<&str>) -> Result<PullRequest, PluginError> {
    let comment = comment.map(str::to_string);
    self.session_pool.call_pooled_export("close-pr", async |b, a| b.call_close_pr(a, number, comment).await).await
  }

  pub async fn list_open_prs(&self, head_branch_prefix: &str) -> Result<Vec<PullRequest>, PluginError> {
    let prefix = head_branch_prefix.to_string();
    self.session_pool.call_pooled_export("list-open-prs", async |b, a| b.call_list_open_prs(a, prefix).await).await
  }

  pub async fn list_closed_prs(&self, head_branches: &[String]) -> Result<Vec<PullRequest>, PluginError> {
    let branches = head_branches.to_vec();
    self
      .session_pool
      .call_pooled_export("list-closed-prs", async |b, a| b.call_list_closed_prs(a, branches).await)
      .await
  }
}
