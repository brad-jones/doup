use crate::remote_url::redact;
use crate::{Auth, CloneOptions, Git, GitBackend, GitCommit, GitError};
use std::fmt;
use std::path::{Path, PathBuf};

/// A repository that [`Git::clone_repo`] just cloned, with operations that act on it.
///
/// The clone's [`Auth`] is kept in memory, as it is never written into the clone itself, so that
/// later operations can reach the remote with the same credentials.
#[derive(Clone)]
pub struct ClonedRepo {
  git: Git,
  path: PathBuf,
  url: String,
  auth: Option<Auth>,
}

impl ClonedRepo {
  pub(crate) fn new(git: Git, options: &CloneOptions) -> Self {
    Self {
      git,
      path: options.destination().to_path_buf(),
      url: options.url().to_owned(),
      auth: options.auth().cloned(),
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
    if !self.fetch_unique(branch).await? {
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

  /// Fetches `branch`'s own commits, returning `false` when it has none to fetch because its tip
  /// is the default branch's tip.
  async fn fetch_unique(&self, branch: &str) -> Result<bool, GitError> {
    if branch.trim().is_empty() {
      return Err(GitError::EmptyBranch);
    }
    let git = &self.git;
    let tips = crate::delegate_git!(git.remote_tips(self, Some(branch)).await)?;
    let tip = tips.branch_tip.ok_or_else(|| GitError::BranchNotFound { branch: branch.to_owned() })?;
    if tip == tips.default_tip {
      return Ok(false);
    }
    crate::delegate_git!(git.fetch_branch(self, branch, &tips.default_branch).await)?;
    Ok(true)
  }
}

impl fmt::Debug for ClonedRepo {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("ClonedRepo")
      .field("git", &self.git)
      .field("path", &self.path)
      .field("url", &redact(&self.url))
      .field("auth", &self.auth)
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
