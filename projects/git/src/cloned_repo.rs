use crate::remote_url::redact;
use crate::{Auth, CloneOptions, Git, GitBackend, GitCommit, GitError, Identity, Worktree};
use std::fmt;
use std::path::{Path, PathBuf};

/// A repository that [`Git::clone_repo`] just cloned, with operations that act on it.
///
/// The clone's [`Auth`] is kept in memory, as it is never written into the clone itself, so that
/// later operations can reach the remote with the same credentials. Likewise its [`Identity`] is
/// only ever handed to the commands that commit, never written to config.
#[derive(Clone)]
pub struct ClonedRepo {
  git: Git,
  path: PathBuf,
  url: String,
  auth: Option<Auth>,
  identity: Identity,
}

impl ClonedRepo {
  pub(crate) fn new(git: Git, options: &CloneOptions) -> Self {
    Self {
      git,
      path: options.destination().to_path_buf(),
      url: options.url().to_owned(),
      auth: options.auth().cloned(),
      identity: options.identity().clone(),
    }
  }

  /// The clone's working tree.
  pub fn path(&self) -> &Path {
    &self.path
  }

  /// The URL the repository was cloned from, which is its `origin` remote.
  pub fn url(&self) -> &str {
    &self.url
  }

  /// The credentials the repository was cloned with, reused to reach `origin`.
  pub fn auth(&self) -> Option<&Auth> {
    self.auth.as_ref()
  }

  /// Who commits made in this clone's worktrees are attributed to.
  pub fn identity(&self) -> &Identity {
    &self.identity
  }

  pub(crate) fn git(&self) -> &Git {
    &self.git
  }

  /// Checks out `branch` in a new linked worktree in a temp dir, like `git worktree add --no-track
  /// -B <branch> <tmp> HEAD`, leaving this clone's own working tree alone.
  ///
  /// The branch starts at the clone's `HEAD` (the default branch, unless the clone was made with
  /// [`CloneOptions::with_branch`]), replacing any local branch of that name. `origin`'s tip of
  /// `branch` is recorded first, as the lease for [`Worktree::push_force`].
  ///
  /// Hooks never run in the worktree and its commits are never signed, whatever the user's Git
  /// config says.
  ///
  /// # Errors
  ///
  /// - [`GitError::EmptyBranch`] or [`GitError::InvalidBranch`] when `branch` is not a valid name.
  /// - Another [`GitError`] when the remote cannot be reached or the worktree cannot be created,
  ///   e.g. because `branch` is already checked out in this clone.
  pub async fn worktree_add(&self, branch: &str) -> Result<Worktree, GitError> {
    validate_branch(branch)?;
    let git = &self.git;
    let lease = crate::delegate_git!(git.remote_tips(self, Some(branch)).await)?.branch_tip;
    let worktree = Worktree::new(self.clone(), branch, lease)?;
    crate::delegate_git!(git.worktree_add(&worktree).await)?;
    Ok(worktree)
  }

  /// Deletes `branch` on `origin`, like `git push --force-with-lease origin :<branch>`, unless
  /// someone has pushed to it since doup last looked at it.
  ///
  /// The lease is the tip last fetched into `refs/remotes/origin/<branch>` (e.g. by
  /// [`Self::branch_authors`]), so a branch vetted before deleting it is only deleted while it
  /// still holds what was vetted. A branch that was never fetched is leased at the tip `origin`
  /// has just before deleting it. The local `refs/remotes/origin/<branch>` is deleted too.
  ///
  /// A branch that does not exist on `origin`, or is deleted by someone else meanwhile, is not an
  /// error: there is nothing left to delete.
  ///
  /// # Errors
  ///
  /// - [`GitError::EmptyBranch`] or [`GitError::InvalidBranch`] when `branch` is not a valid name.
  /// - [`GitError::DeleteDefaultBranch`] when `branch` is `origin`'s default branch.
  /// - [`GitError::StaleLease`] when `origin`'s branch has moved since it was last fetched.
  /// - [`GitError::PushRejected`] when `origin` refuses the deletion, e.g. it is protected.
  /// - Another [`GitError`] when the remote cannot be reached.
  pub async fn delete_remote_branch(&self, branch: &str) -> Result<(), GitError> {
    validate_branch(branch)?;
    let git = &self.git;
    let tips = crate::delegate_git!(git.remote_tips(self, Some(branch)).await)?;
    if tips.default_branch == branch {
      return Err(GitError::DeleteDefaultBranch { branch: branch.to_owned() });
    }
    let Some(remote_tip) = tips.branch_tip else {
      return Ok(());
    };
    let lease = crate::delegate_git!(git.tracking_tip(self, branch).await)?.unwrap_or(remote_tip);
    match crate::delegate_git!(git.delete_remote_branch(self, branch, &lease).await) {
      // A lease also fails when the branch is already gone, which is what was wanted.
      Err(GitError::StaleLease { branch: stale }) => {
        match crate::delegate_git!(git.remote_tips(self, Some(branch)).await)?.branch_tip {
          None => Ok(()),
          Some(_) => Err(GitError::StaleLease { branch: stale }),
        }
      }
      result => result,
    }
  }

  /// Lists every branch on the `origin` remote, like `git ls-remote --heads origin`.
  ///
  /// The remote itself is asked, not the clone's tracking refs, so branches a shallow or
  /// single-branch clone did not fetch are included. Names are short (`main`, not
  /// `refs/heads/main`), sorted and free of duplicates.
  ///
  /// # Errors
  ///
  /// Returns a [`GitError`] when the remote cannot be reached or read.
  pub async fn list_remote_branches(&self) -> Result<Vec<String>, GitError> {
    let git = &self.git;
    let mut branches = crate::delegate_git!(git.list_remote_branches(self).await)?;
    branches.sort_unstable();
    branches.dedup();
    Ok(branches)
  }

  /// The short name of `origin`'s default branch, the branch its `HEAD` points at (e.g. `main`).
  ///
  /// # Errors
  ///
  /// Returns [`GitError::NoDefaultBranch`] when `origin`'s `HEAD` does not point at a branch, or
  /// another [`GitError`] when the remote cannot be reached or read.
  pub async fn default_branch(&self) -> Result<String, GitError> {
    let git = &self.git;
    Ok(crate::delegate_git!(git.remote_tips(self, None).await)?.default_branch)
  }

  /// Fetches the commits `branch` on `origin` has on top of `origin`'s default branch into
  /// `refs/remotes/origin/<branch>`, whatever state the clone is in.
  ///
  /// Only those commits are fetched, and without file contents: like `git fetch
  /// --filter=blob:none --shallow-exclude=<default branch>`. The clone therefore becomes a
  /// partial clone (`remote.origin.promisor`), and the fetched commits are shallow where
  /// `branch` leaves the default branch. That includes any merge commit that merged the default
  /// branch into `branch`, so history behind such a merge is not fetched (see
  /// [`Self::branch_commits`]).
  ///
  /// When `branch`'s tip is the default branch's tip (including when `branch` *is* the default
  /// branch) there is nothing to fetch, so nothing is fetched.
  ///
  /// # Errors
  ///
  /// - [`GitError::EmptyBranch`] when `branch` is blank.
  /// - [`GitError::BranchNotFound`] when `origin` has no such branch.
  /// - [`GitError::NoUniqueCommits`] when `branch` is behind the default branch, so has no
  ///   commits of its own (e.g. it was merged).
  /// - Another [`GitError`] when the remote cannot be reached or read.
  pub async fn branch_fetch(&self, branch: &str) -> Result<(), GitError> {
    self.fetch_unique(branch).await.map(drop)
  }

  /// Lists the commits on `origin`'s `branch` that are not on its default branch, newest first
  /// (like `git log origin/HEAD..origin/<branch>`), after fetching them with
  /// [`Self::branch_fetch`].
  ///
  /// Empty when `branch`'s tip is the default branch's tip, including for the default branch
  /// itself.
  ///
  /// The walk stops at a merge commit that merged the default branch into `branch`, so commits
  /// behind it are not listed. Such a commit is still listed, with [`GitCommit::is_merge`] set,
  /// so callers can tell when the list may be incomplete.
  ///
  /// # Errors
  ///
  /// As for [`Self::branch_fetch`], or a [`GitError`] when the fetched commits cannot be read.
  pub async fn branch_commits(&self, branch: &str) -> Result<Vec<GitCommit>, GitError> {
    if self.fetch_unique(branch).await?.is_none() {
      return Ok(Vec::new());
    }
    let git = &self.git;
    crate::delegate_git!(git.branch_commits(self, branch).await)
  }

  /// The email addresses of the authors of [`Self::branch_commits`], sorted and free of
  /// duplicates (like `git show -s --format=%ae`, without applying a mailmap).
  ///
  /// # Errors
  ///
  /// As for [`Self::branch_commits`].
  pub async fn branch_authors(&self, branch: &str) -> Result<Vec<String>, GitError> {
    let mut authors: Vec<String> =
      self.branch_commits(branch).await?.iter().map(|commit| commit.author_email().to_owned()).collect();
    authors.sort_unstable();
    authors.dedup();
    Ok(authors)
  }

  /// Whether `origin`'s `branch` is behind its default branch, ie: does not contain the default
  /// branch's tip, so a pull request from it is not tested against the latest default branch.
  ///
  /// A branch is *not* behind when its tip is the default branch's tip, or when one of its own
  /// commits (see [`Self::branch_commits`]) has the default branch's tip as a parent. That is
  /// either a branch built directly on top of the default branch, or one the default branch was
  /// later merged into. Every other branch is behind, including one with no commits of its own.
  ///
  /// # Errors
  ///
  /// - [`GitError::EmptyBranch`] when `branch` is blank.
  /// - [`GitError::BranchNotFound`] when `origin` has no such branch.
  /// - Another [`GitError`] when the remote cannot be reached or the fetched commits cannot be
  ///   read.
  pub async fn branch_is_behind(&self, branch: &str) -> Result<bool, GitError> {
    let default_tip = match self.fetch_unique(branch).await {
      Ok(Some(default_tip)) => default_tip,
      Ok(None) => return Ok(false),
      Err(GitError::NoUniqueCommits { .. }) => return Ok(true),
      Err(err) => return Err(err),
    };
    // The fetch stops where `branch` leaves the default branch, but raw commits keep their parent
    // ids, so a commit built on (or merging in) the current default tip still names it.
    let git = &self.git;
    let commits = crate::delegate_git!(git.branch_commits(self, branch).await)?;
    Ok(!commits.iter().any(|commit| commit.parents().contains(&default_tip)))
  }

  /// Fetches `branch`'s own commits, returning the default branch's tip they were fetched
  /// against, or `None` when there is nothing to fetch because `branch`'s tip is that tip.
  async fn fetch_unique(&self, branch: &str) -> Result<Option<String>, GitError> {
    if branch.trim().is_empty() {
      return Err(GitError::EmptyBranch);
    }
    let git = &self.git;
    let tips = crate::delegate_git!(git.remote_tips(self, Some(branch)).await)?;
    let tip = tips.branch_tip.ok_or_else(|| GitError::BranchNotFound { branch: branch.to_owned() })?;
    if tip == tips.default_tip {
      return Ok(None);
    }
    crate::delegate_git!(git.fetch_branch(self, branch, &tips.default_branch).await)?;
    Ok(Some(tips.default_tip))
  }
}

/// Checks `branch` is a name a branch can have, before it is handed to a backend.
fn validate_branch(branch: &str) -> Result<(), GitError> {
  if branch.trim().is_empty() {
    return Err(GitError::EmptyBranch);
  }
  if grit_lib::refs::check_ref_format::RefName::branch(branch).is_err() {
    return Err(GitError::InvalidBranch { branch: branch.to_owned() });
  }
  Ok(())
}

impl fmt::Debug for ClonedRepo {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("ClonedRepo")
      .field("git", &self.git)
      .field("path", &self.path)
      .field("url", &redact(&self.url))
      .field("auth", &self.auth)
      .field("identity", &self.identity)
      .finish()
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::GitCli;

  #[test]
  fn debug_redacts_secrets() {
    let options = CloneOptions::new("https://u:secret@github.com/a/b.git", "/tmp/b")
      .with_auth(Auth::Https { user: "u".into(), pass: "hunter2".into() });
    let out = format!("{:?}", ClonedRepo::new(Git::new(GitCli), &options));
    assert!(!out.contains("secret"), "{out}");
    assert!(!out.contains("hunter2"), "{out}");
  }
}
