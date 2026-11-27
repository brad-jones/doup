//! The plugins doup drives, behind traits so that something other than WASM plugins (eg: fakes in tests)
//! can stand in for them.
//!
//! [`WasmPlugins`] loads the real thing: [`PlatformPlugin`]s & [`EcosystemPlugin`]s.

use doup_config::repo::DoupRepoPlugin;
use doup_plugin_host::ecosystem_plugin::{Dependency, ReleaseNote, Versions};
use doup_plugin_host::platform_plugin::{PullRequest, PullRequestContent};
use doup_plugin_host::{EcosystemPlugin, PlatformPlugin, PluginError};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A hosting platform, where pull requests are opened. See the `platform` world in the WIT for the contract.
pub trait Platform: Send + Sync + 'static {
  /// Opens a new pull request from `head_branch`, which must already have been pushed.
  fn create_pr(
    &self,
    head_branch: &str,
    content: &PullRequestContent,
  ) -> impl Future<Output = Result<PullRequest, PluginError>> + Send;

  /// Replaces the base branch, title, body & version of the open pull request `number`.
  fn update_pr(
    &self,
    number: i64,
    content: &PullRequestContent,
  ) -> impl Future<Output = Result<PullRequest, PluginError>> + Send;

  /// Closes the pull request `number` without merging it, first posting `comment` to it when given.
  fn close_pr(
    &self,
    number: i64,
    comment: Option<&str>,
  ) -> impl Future<Output = Result<PullRequest, PluginError>> + Send;

  /// Lists every open pull request whose head branch starts with `head_branch_prefix`.
  fn list_open_prs(
    &self,
    head_branch_prefix: &str,
  ) -> impl Future<Output = Result<Vec<PullRequest>, PluginError>> + Send;

  /// Lists every closed (not merged) pull request whose head branch is one of `head_branches`.
  fn list_closed_prs(
    &self,
    head_branches: &[String],
  ) -> impl Future<Output = Result<Vec<PullRequest>, PluginError>> + Send;
}

/// A package ecosystem, whose dependencies doup updates. See the `ecosystem` world in the WIT for the contract.
pub trait Ecosystem: Send + Sync + 'static {
  /// Lists the dependencies found in `workspace_dir`.
  fn discover_deps(&self, workspace_dir: &Path) -> impl Future<Output = Result<Vec<Dependency>, PluginError>> + Send;

  /// Looks up the versions `dependency` may be updated to, `None` when it is up to date.
  fn lookup_versions(
    &self,
    dependency: &Dependency,
  ) -> impl Future<Output = Result<Option<Versions>, PluginError>> + Send;

  /// Fetches the release notes of every version of `dependency` up to `to`.
  fn fetch_release_notes(
    &self,
    dependency: &Dependency,
    to: &str,
  ) -> impl Future<Output = Result<Vec<ReleaseNote>, PluginError>> + Send;

  /// Updates every occurrence of `dependency` in `workspace_dir` to version `to`.
  fn apply_updates(
    &self,
    workspace_dir: &Path,
    dependency: &Dependency,
    to: &str,
  ) -> impl Future<Output = Result<(), PluginError>> + Send;
}

/// Loads the plugins a repo's config asks for.
pub trait PluginLoader: Send + Sync + 'static {
  type Platform: Platform;
  type Ecosystem: Ecosystem;

  /// Loads `plugin`, looking for a relative module path in `module_search_dirs`.
  fn load_platform(
    &self,
    plugin: &DoupRepoPlugin,
    module_search_dirs: Vec<PathBuf>,
  ) -> impl Future<Output = Result<Self::Platform, PluginError>> + Send;

  /// Loads `plugin`, looking for a relative module path in `module_search_dirs`.
  fn load_ecosystem(
    &self,
    plugin: &DoupRepoPlugin,
    module_search_dirs: Vec<PathBuf>,
  ) -> impl Future<Output = Result<Self::Ecosystem, PluginError>> + Send;
}

/// Loads real WASM plugins, see [`PlatformPlugin::load`] & [`EcosystemPlugin::load`].
#[derive(Debug, Clone, Copy, Default)]
pub struct WasmPlugins;

impl PluginLoader for WasmPlugins {
  type Platform = PlatformPlugin;
  type Ecosystem = EcosystemPlugin;

  async fn load_platform(
    &self,
    plugin: &DoupRepoPlugin,
    module_search_dirs: Vec<PathBuf>,
  ) -> Result<PlatformPlugin, PluginError> {
    PlatformPlugin::load(plugin, module_search_dirs).await
  }

  async fn load_ecosystem(
    &self,
    plugin: &DoupRepoPlugin,
    module_search_dirs: Vec<PathBuf>,
  ) -> Result<EcosystemPlugin, PluginError> {
    EcosystemPlugin::load(plugin, module_search_dirs).await
  }
}

impl Platform for PlatformPlugin {
  async fn create_pr(&self, head_branch: &str, content: &PullRequestContent) -> Result<PullRequest, PluginError> {
    PlatformPlugin::create_pr(self, head_branch, content).await
  }

  async fn update_pr(&self, number: i64, content: &PullRequestContent) -> Result<PullRequest, PluginError> {
    PlatformPlugin::update_pr(self, number, content).await
  }

  async fn close_pr(&self, number: i64, comment: Option<&str>) -> Result<PullRequest, PluginError> {
    PlatformPlugin::close_pr(self, number, comment).await
  }

  async fn list_open_prs(&self, head_branch_prefix: &str) -> Result<Vec<PullRequest>, PluginError> {
    PlatformPlugin::list_open_prs(self, head_branch_prefix).await
  }

  async fn list_closed_prs(&self, head_branches: &[String]) -> Result<Vec<PullRequest>, PluginError> {
    PlatformPlugin::list_closed_prs(self, head_branches).await
  }
}

impl Ecosystem for EcosystemPlugin {
  async fn discover_deps(&self, workspace_dir: &Path) -> Result<Vec<Dependency>, PluginError> {
    EcosystemPlugin::discover_deps(self, workspace_dir).await
  }

  async fn lookup_versions(&self, dependency: &Dependency) -> Result<Option<Versions>, PluginError> {
    EcosystemPlugin::lookup_versions(self, dependency).await
  }

  async fn fetch_release_notes(&self, dependency: &Dependency, to: &str) -> Result<Vec<ReleaseNote>, PluginError> {
    EcosystemPlugin::fetch_release_notes(self, dependency, to).await
  }

  async fn apply_updates(&self, workspace_dir: &Path, dependency: &Dependency, to: &str) -> Result<(), PluginError> {
    EcosystemPlugin::apply_updates(self, workspace_dir, dependency, to).await
  }
}

impl<T: Platform> Platform for Arc<T> {
  fn create_pr(
    &self,
    head_branch: &str,
    content: &PullRequestContent,
  ) -> impl Future<Output = Result<PullRequest, PluginError>> + Send {
    T::create_pr(self, head_branch, content)
  }

  fn update_pr(
    &self,
    number: i64,
    content: &PullRequestContent,
  ) -> impl Future<Output = Result<PullRequest, PluginError>> + Send {
    T::update_pr(self, number, content)
  }

  fn close_pr(
    &self,
    number: i64,
    comment: Option<&str>,
  ) -> impl Future<Output = Result<PullRequest, PluginError>> + Send {
    T::close_pr(self, number, comment)
  }

  fn list_open_prs(
    &self,
    head_branch_prefix: &str,
  ) -> impl Future<Output = Result<Vec<PullRequest>, PluginError>> + Send {
    T::list_open_prs(self, head_branch_prefix)
  }

  fn list_closed_prs(
    &self,
    head_branches: &[String],
  ) -> impl Future<Output = Result<Vec<PullRequest>, PluginError>> + Send {
    T::list_closed_prs(self, head_branches)
  }
}

impl<T: Ecosystem> Ecosystem for Arc<T> {
  fn discover_deps(&self, workspace_dir: &Path) -> impl Future<Output = Result<Vec<Dependency>, PluginError>> + Send {
    T::discover_deps(self, workspace_dir)
  }

  fn lookup_versions(
    &self,
    dependency: &Dependency,
  ) -> impl Future<Output = Result<Option<Versions>, PluginError>> + Send {
    T::lookup_versions(self, dependency)
  }

  fn fetch_release_notes(
    &self,
    dependency: &Dependency,
    to: &str,
  ) -> impl Future<Output = Result<Vec<ReleaseNote>, PluginError>> + Send {
    T::fetch_release_notes(self, dependency, to)
  }

  fn apply_updates(
    &self,
    workspace_dir: &Path,
    dependency: &Dependency,
    to: &str,
  ) -> impl Future<Output = Result<(), PluginError>> + Send {
    T::apply_updates(self, workspace_dir, dependency, to)
  }
}
