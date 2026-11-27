//! Why [`super::reconcile`] decided on each [`super::Action`].
//!
//! Every reason renders, via [`fmt::Display`], as one or more full sentences written for the people
//! who maintain the repo. They end up in doup's output, and a [`CloseReason`] is posted as a comment
//! on the pull request being closed.

use super::PrNumber;
use std::fmt;

/// Why doup is opening a new pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CreateReason {
  /// No pull request has ever been opened for this update.
  NoExistingPr,

  /// The most recent pull request for this update, `number`, was closed while proposing `version`,
  /// and there's now a different version to propose.
  PreviousPrClosed { number: PrNumber, version: String },
}

impl fmt::Display for CreateReason {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      Self::NoExistingPr => write!(f, "A new version is available and there is no pull request for it yet."),
      Self::PreviousPrClosed { number, version } => write!(
        f,
        "Pull request {number}, which proposed {version}, was closed but a different version is now available."
      ),
    }
  }
}

/// Why doup is rebuilding an open pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum UpdateReason {
  /// The pull request proposes `previous`, but there's a newer version to propose.
  NewerVersion { previous: String },

  /// The pull request proposes the right version, but its branch has fallen behind the default branch.
  BehindDefaultBranch,
}

impl fmt::Display for UpdateReason {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      Self::NewerVersion { previous } => {
        write!(f, "This pull request proposed {previous}, but a newer version is now available.")
      }
      Self::BehindDefaultBranch => write!(
        f,
        "This pull request had fallen behind the default branch, so it was rebuilt on top of it to run its checks \
         against the latest changes."
      ),
    }
  }
}

/// Why doup is leaving an update alone.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkipReason {
  /// The open pull request `number` already proposes this version & is up to date with the default branch.
  AlreadyProposed { number: PrNumber },

  /// Someone other than doup has pushed to the open pull request `number`, so rebuilding it would throw their
  /// work away.
  ForeignCommits { number: PrNumber },

  /// The pull request `number`, proposing this exact version, was closed without being merged. That's how
  /// maintainers tell doup to ignore a version.
  PreviouslyClosed { number: PrNumber },
}

impl fmt::Display for SkipReason {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      Self::AlreadyProposed { number } => {
        write!(f, "Pull request {number} already proposes this version and is up to date with the default branch.")
      }
      Self::ForeignCommits { number } => write!(
        f,
        "Someone other than doup has pushed to pull request {number}, so doup is leaving it alone rather than \
         overwrite their work."
      ),
      Self::PreviouslyClosed { number } => write!(
        f,
        "Pull request {number}, which proposed this version, was closed without being merged, so doup won't \
         propose it again."
      ),
    }
  }
}

/// Why doup is closing an open pull request. Posted as a comment on the pull request as it's closed.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CloseReason {
  /// The dependency no longer has any update available.
  UpToDate,

  /// The pull request proposes a version within the dependency's version constraints, but the dependency is
  /// already at the newest such version. Only an update outside its constraints is left.
  NoUpdateWithinConstraints,

  /// The pull request proposes the latest version, outside the dependency's version constraints, but that
  /// version is now within them so doup's other pull request for the dependency proposes it instead.
  LatestWithinConstraints,

  /// doup can no longer find the dependency the pull request updates.
  DependencyNotFound,
}

impl fmt::Display for CloseReason {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      Self::UpToDate => write!(
        f,
        "This dependency is already up to date on the default branch, so this pull request is no longer needed."
      ),
      Self::NoUpdateWithinConstraints => write!(
        f,
        "This dependency is already at the newest version allowed by its version constraints, so this pull request \
         is no longer needed. Any newer version, which may contain breaking changes, is proposed by a separate pull \
         request."
      ),
      Self::LatestWithinConstraints => write!(
        f,
        "The latest version of this dependency is now allowed by its version constraints, so doup proposes it in its \
         other pull request for this dependency instead."
      ),
      Self::DependencyNotFound => write!(
        f,
        "doup can no longer find this dependency in the repository. It may have been removed, or its ecosystem may \
         no longer be configured for doup."
      ),
    }
  }
}

/// Why doup is deleting one of its branches.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DeleteReason {
  /// The pull request `number` that uses the branch is being closed by doup.
  PrClosing { number: PrNumber },

  /// The pull request `number` that used the branch was closed.
  PrClosed { number: PrNumber },

  /// No pull request uses the branch and doup has no update to push to it.
  Orphaned,
}

impl fmt::Display for DeleteReason {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      Self::PrClosing { number } => write!(f, "Pull request {number}, which uses this branch, is being closed."),
      Self::PrClosed { number } => {
        write!(f, "Pull request {number}, which used this branch, was closed so the branch is no longer needed.")
      }
      Self::Orphaned => write!(f, "No pull request uses this branch and doup has no update to push to it."),
    }
  }
}
