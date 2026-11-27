//! What happened during a run, returned by [`crate::Updater::run`].

use crate::error::{ActionError, DependencyError, RepoError};
use crate::reconcile::Action;

/// The outcome of updating every repo, in the order they are configured.
#[derive(Debug, Default)]
pub struct UpdateReport {
  pub(crate) repos: Vec<RepoReport>,
}

impl UpdateReport {
  pub fn repos(&self) -> &[RepoReport] {
    &self.repos
  }

  /// The repos that hit any error, see [`RepoReport::is_success`].
  pub fn failed_repos(&self) -> impl Iterator<Item = &RepoReport> {
    self.repos.iter().filter(|repo| !repo.is_success())
  }

  /// Whether every repo was updated without any error.
  pub fn is_success(&self) -> bool {
    self.repos.iter().all(RepoReport::is_success)
  }
}

/// The outcome of updating one repo.
#[derive(Debug)]
pub struct RepoReport {
  pub(crate) origin: String,
  pub(crate) error: Option<RepoError>,
  pub(crate) dependency_errors: Vec<DependencyError>,
  pub(crate) actions: Vec<ActionReport>,
}

impl RepoReport {
  pub(crate) fn new(origin: String) -> Self {
    Self { origin, error: None, dependency_errors: Vec::new(), actions: Vec::new() }
  }

  /// The repo's origin URL, with any password removed.
  pub fn origin(&self) -> &str {
    &self.origin
  }

  /// The error that stopped the repo being updated, if any. Any actions already planned were not carried out.
  pub fn error(&self) -> Option<&RepoError> {
    self.error.as_ref()
  }

  /// The dependencies that could not be considered for updates. Their pull requests & branches were left alone.
  pub fn dependency_errors(&self) -> &[DependencyError] {
    &self.dependency_errors
  }

  /// Every action planned for the repo, in the order planned, with its outcome.
  pub fn actions(&self) -> &[ActionReport] {
    &self.actions
  }

  /// Whether the repo was updated without any error.
  pub fn is_success(&self) -> bool {
    self.error.is_none()
      && self.dependency_errors.is_empty()
      && self.actions.iter().all(|action| !action.outcome.is_failure())
  }
}

/// An action & what came of it.
#[derive(Debug)]
pub struct ActionReport {
  pub(crate) action: Action,
  pub(crate) outcome: ActionOutcome,
}

impl ActionReport {
  pub fn action(&self) -> &Action {
    &self.action
  }

  pub fn outcome(&self) -> &ActionOutcome {
    &self.outcome
  }
}

/// What came of an [`Action`].
#[derive(Debug)]
#[non_exhaustive]
pub enum ActionOutcome {
  /// A dry run: the action was planned but not carried out.
  Planned,

  /// The action was an [`Action::Skip`], so there was nothing to do.
  Skipped,

  /// The action was carried out. `url` is that of the pull request it acted on, if any.
  Done { url: Option<String> },

  /// The action failed.
  Failed(ActionError),

  /// The action was not attempted, as an action it depends on failed (eg: a branch is not deleted when closing its
  /// pull request failed, as that would close the pull request without explaining why).
  NotAttempted,
}

impl ActionOutcome {
  /// Whether the action failed or could not be attempted.
  pub fn is_failure(&self) -> bool {
    matches!(self, Self::Failed(_) | Self::NotAttempted)
  }
}
