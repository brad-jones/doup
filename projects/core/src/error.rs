//! The errors doup can run into while updating repos.
//!
//! None of them stop the whole run: a [`RepoError`] stops one repo, a [`DependencyError`] stops one
//! dependency & an [`ActionError`] stops one action. They are all collected into the
//! [`crate::UpdateReport`].
//!
//! Git & plugin errors are boxed, as they're large & errors are rare, keeping every `Result` small.
//!
//! Messages only describe their own context, the underlying cause is always available via
//! [`std::error::Error::source`], so walk the chain (or render with `{:#}` in anyhow) to see it all.

use crate::reconcile::PrNumber;
use doup_config::ConfigError;
use doup_git::GitError;
use doup_plugin_host::PluginError;

/// Why a repo could not be updated at all.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RepoError {
  #[error("the repo uses SSH auth, but no key was loaded")]
  SshKeyNotLoaded,

  #[error("failed to create a temporary directory to clone into")]
  TempDir(#[source] std::io::Error),

  #[error("failed to clone the repo")]
  Clone(#[source] Box<GitError>),

  #[error("failed to load the repo's doup config")]
  Config(#[source] ConfigError),

  #[error("failed to load the platform plugin")]
  LoadPlatform(#[source] Box<PluginError>),

  #[error("failed to load an ecosystem plugin")]
  LoadEcosystem(#[source] Box<PluginError>),

  #[error("failed to discover the repo's dependencies")]
  DiscoverDeps(#[source] Box<PluginError>),

  #[error("failed to list the remote's branches")]
  ListBranches(#[source] Box<GitError>),

  #[error("failed to inspect branch '{branch}'")]
  InspectBranch { branch: String, source: Box<GitError> },

  #[error("failed to read the remote's default branch")]
  DefaultBranch(#[source] Box<GitError>),

  #[error("failed to list open pull requests")]
  ListOpenPrs(#[source] Box<PluginError>),

  #[error("failed to list closed pull requests")]
  ListClosedPrs(#[source] Box<PluginError>),

  /// The task updating the repo panicked.
  #[error("updating the repo panicked")]
  Panicked(#[source] tokio::task::JoinError),
}

/// Why one dependency of a repo could not be considered for updates.
///
/// Its existing pull requests & branches are left alone, as there's no telling whether they're still wanted.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DependencyError {
  #[error("failed to look up the versions of '{dependency}' with plugin '{ecosystem}'")]
  LookupVersions { ecosystem: String, dependency: String, source: Box<PluginError> },
}

/// Why an [`crate::Action`] failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ActionError {
  #[error("failed to check out branch '{branch}'")]
  Checkout { branch: String, source: Box<GitError> },

  #[error("failed to apply the update")]
  ApplyUpdate(#[source] Box<PluginError>),

  #[error("failed to commit the update")]
  Commit(#[source] Box<GitError>),

  #[error("failed to push branch '{branch}'")]
  Push { branch: String, source: Box<GitError> },

  #[error("failed to open the pull request")]
  CreatePr(#[source] Box<PluginError>),

  #[error("failed to update pull request {number}")]
  UpdatePr { number: PrNumber, source: Box<PluginError> },

  #[error("failed to close pull request {number}")]
  ClosePr { number: PrNumber, source: Box<PluginError> },

  #[error("failed to delete branch '{branch}'")]
  DeleteBranch { branch: String, source: Box<GitError> },

  /// The plugin the update belongs to is not loaded. Only possible if the action wasn't planned by doup.
  #[error("ecosystem plugin '{plugin}' is not loaded")]
  UnknownEcosystem { plugin: String },
}
