use crate::{CloneOptions, ClonedRepo, GitCommit, GitError, Worktree};
use std::future::Future;

/// What `origin` says about its default branch and, optionally, one other branch.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RemoteTips {
  /// The short name of the branch `origin`'s `HEAD` points at.
  pub default_branch: String,
  /// The full hex id of the commit at the tip of the default branch.
  pub default_tip: String,
  /// The full hex id of the commit at the tip of the requested branch, when it exists.
  pub branch_tip: Option<String>,
}

/// A strategy for carrying out Git operations.
///
/// Call these through [`crate::Git`] and [`ClonedRepo`] instead, which validate input and
/// normalise results so every backend behaves the same.
pub trait GitBackend: Send + Sync {
  /// Clones a repository as described by `options`.
  ///
  /// # Errors
  ///
  /// Returns a [`GitError`] when `options` are invalid or the clone fails.
  fn clone_repo(&self, options: &CloneOptions) -> impl Future<Output = Result<(), GitError>> + Send;

  /// Lists the short names of the branches on `repo`'s `origin` remote, in any order.
  ///
  /// # Errors
  ///
  /// Returns a [`GitError`] when the remote cannot be reached or read.
  fn list_remote_branches(&self, repo: &ClonedRepo) -> impl Future<Output = Result<Vec<String>, GitError>> + Send;

  /// Asks `origin` for its default branch and the tips of it and of `branch`, like
  /// `git ls-remote --symref origin HEAD refs/heads/<branch>`.
  ///
  /// # Errors
  ///
  /// Returns [`GitError::NoDefaultBranch`] when `origin`'s `HEAD` does not point at a branch, or
  /// another [`GitError`] when the remote cannot be reached or read.
  fn remote_tips(
    &self,
    repo: &ClonedRepo,
    branch: Option<&str>,
  ) -> impl Future<Output = Result<RemoteTips, GitError>> + Send;

  /// Fetches the commits on `origin`'s `branch` that are not on its `exclude` branch, without file
  /// contents, into `refs/remotes/origin/<branch>`. Like `git fetch --filter=blob:none
  /// --shallow-exclude=refs/heads/<exclude> origin +refs/heads/<branch>:refs/remotes/origin/<branch>`.
  ///
  /// # Errors
  ///
  /// Returns [`GitError::NoUniqueCommits`] when every commit on `branch` is on `exclude`,
  /// [`GitError::BranchNotFound`] when `branch` does not exist, or another [`GitError`] when the
  /// remote cannot be reached or read.
  fn fetch_branch(
    &self,
    repo: &ClonedRepo,
    branch: &str,
    exclude: &str,
  ) -> impl Future<Output = Result<(), GitError>> + Send;

  /// Lists the commits reachable from the local `refs/remotes/origin/<branch>`, newest first like
  /// `git rev-list`, without touching the remote.
  ///
  /// # Errors
  ///
  /// Returns a [`GitError`] when the ref or its commits cannot be read.
  fn branch_commits(
    &self,
    repo: &ClonedRepo,
    branch: &str,
  ) -> impl Future<Output = Result<Vec<GitCommit>, GitError>> + Send;

  /// The full hex id of the local `refs/remotes/origin/<branch>`, or `None` when the clone has no
  /// such ref, without touching the remote.
  ///
  /// # Errors
  ///
  /// Returns a [`GitError`] when the clone's refs cannot be read.
  fn tracking_tip(
    &self,
    repo: &ClonedRepo,
    branch: &str,
  ) -> impl Future<Output = Result<Option<String>, GitError>> + Send;

  /// Deletes `branch` on `origin` as long as it is still at `lease`, then deletes the local
  /// `refs/remotes/origin/<branch>` if the clone has it. Like `git push --no-verify
  /// --force-with-lease=refs/heads/<branch>:<lease> origin :refs/heads/<branch>`.
  ///
  /// # Errors
  ///
  /// Returns [`GitError::StaleLease`] when `origin`'s branch is not at `lease` (including when it
  /// no longer exists), [`GitError::PushRejected`] when `origin` refuses the deletion for another
  /// reason, or another [`GitError`] when the remote cannot be reached.
  fn delete_remote_branch(
    &self,
    repo: &ClonedRepo,
    branch: &str,
    lease: &str,
  ) -> impl Future<Output = Result<(), GitError>> + Send;

  /// Creates a linked worktree of `worktree.repo()` at `worktree.path()` with `worktree.branch()`
  /// created or reset at the clone's `HEAD`, like `git worktree add --no-track -B <branch> <path>
  /// HEAD`. Hooks never run, see [`Worktree`].
  ///
  /// # Errors
  ///
  /// Returns a [`GitError`] when the worktree cannot be created.
  fn worktree_add(&self, worktree: &Worktree) -> impl Future<Output = Result<(), GitError>> + Send;

  /// Stages every change in `worktree` and commits it as the clone's identity, unsigned and
  /// without hooks, returning the new commit's full hex id.
  ///
  /// # Errors
  ///
  /// Returns [`GitError::NothingToCommit`] when there are no changes, or another [`GitError`] when
  /// the commit cannot be made.
  fn worktree_commit(
    &self,
    worktree: &Worktree,
    message: &str,
  ) -> impl Future<Output = Result<String, GitError>> + Send;

  /// Pushes the worktree's branch to the same name on `origin` without forcing, returning the
  /// full hex id pushed.
  ///
  /// # Errors
  ///
  /// Returns [`GitError::PushRejected`] when `origin` refuses the update, or another [`GitError`]
  /// when the remote cannot be reached.
  fn worktree_push(&self, worktree: &Worktree) -> impl Future<Output = Result<String, GitError>> + Send;

  /// Like [`Self::worktree_push`], but overwrites `origin`'s branch as long as it is still at
  /// `lease`, or still does not exist when `lease` is `None`.
  ///
  /// # Errors
  ///
  /// Returns [`GitError::StaleLease`] when the lease does not hold, otherwise as for
  /// [`Self::worktree_push`].
  fn worktree_push_force(
    &self,
    worktree: &Worktree,
    lease: Option<&str>,
  ) -> impl Future<Output = Result<String, GitError>> + Send;

  /// Deletes `worktree` and unregisters it from its clone, like `git worktree remove --force`.
  ///
  /// # Errors
  ///
  /// Returns a [`GitError`] when the worktree cannot be removed.
  fn worktree_remove(&self, worktree: &Worktree) -> impl Future<Output = Result<(), GitError>> + Send;
}
