use crate::{ClonedRepo, Git, GitBackend, GitError};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use tempfile::TempDir;

/// A linked worktree of a [`ClonedRepo`] with one branch checked out, made by
/// [`ClonedRepo::worktree_add`], in which to commit changes and push them to `origin`.
///
/// Call [`Self::remove`] when done. If it is dropped instead, it is removed from disk and
/// unregistered from the clone on a best-effort basis, blocking the current thread to do so.
pub struct Worktree {
  repo: ClonedRepo,
  branch: String,
  path: PathBuf,
  /// What `origin`'s branch was last seen to point at: when the worktree was added, then after
  /// each push. `None` when the branch did not exist.
  lease: Mutex<Option<String>>,
  /// Holds the worktree, at `<dir>/worktree`, and an always empty hooks path, at `<dir>/hooks`.
  /// `None` once removed.
  dir: Option<TempDir>,
}

impl Worktree {
  pub(crate) fn new(repo: ClonedRepo, branch: &str, lease: Option<String>) -> Result<Self, GitError> {
    let dir = tempfile::Builder::new().prefix("doup-worktree-").tempdir()?;
    Ok(Self {
      repo,
      branch: branch.to_owned(),
      path: dir.path().join("worktree"),
      lease: Mutex::new(lease),
      dir: Some(dir),
    })
  }

  /// The worktree's working tree.
  pub fn path(&self) -> &Path {
    &self.path
  }

  /// The branch checked out in the worktree, and pushed to on `origin`.
  pub fn branch(&self) -> &str {
    &self.branch
  }

  /// The clone this is a worktree of.
  pub fn repo(&self) -> &ClonedRepo {
    &self.repo
  }

  /// A hooks path that never holds any hooks, so the user's own (e.g. a global `core.hooksPath`)
  /// never run here.
  pub(crate) fn hooks_path(&self) -> PathBuf {
    self.path.with_file_name("hooks")
  }

  /// Stages every change in the worktree (new, modified and deleted files) and commits it to the
  /// branch as the clone's [`crate::Identity`], like `git add --all && git commit --no-verify`.
  /// Returns the new commit's full hex id.
  ///
  /// # Errors
  ///
  /// - [`GitError::EmptyCommitMessage`] when `message` is blank.
  /// - [`GitError::NothingToCommit`] when the worktree has no changes.
  /// - Another [`GitError`] when the commit cannot be made.
  pub async fn commit(&self, message: impl AsRef<str>) -> Result<String, GitError> {
    let message = message.as_ref();
    if message.trim().is_empty() {
      return Err(GitError::EmptyCommitMessage);
    }
    let git = self.repo.git();
    crate::delegate_git!(git.worktree_commit(self, message).await)
  }

  /// Pushes the branch to the same name on `origin`, creating it there if needed, like `git push
  /// origin <branch>`. Only a fast-forward is allowed; use [`Self::push_force`] to overwrite.
  ///
  /// # Errors
  ///
  /// - [`GitError::PushRejected`] when `origin` refuses the update, e.g. it is not a fast-forward.
  /// - Another [`GitError`] when the remote cannot be reached.
  pub async fn push(&self) -> Result<(), GitError> {
    let git = self.repo.git();
    let pushed = crate::delegate_git!(git.worktree_push(self).await)?;
    self.set_lease(pushed);
    Ok(())
  }

  /// Overwrites the branch on `origin` with the worktree's, like `git push
  /// --force-with-lease=refs/heads/<branch>:<lease> origin <branch>`.
  ///
  /// The lease is the tip `origin` had when the worktree was added, or that this worktree last
  /// pushed. So a branch someone else has pushed to since is never overwritten, and neither is
  /// one created by someone else when the lease is that it did not exist.
  ///
  /// # Errors
  ///
  /// - [`GitError::StaleLease`] when `origin`'s branch no longer matches the lease.
  /// - [`GitError::PushRejected`] when `origin` refuses the update for another reason.
  /// - Another [`GitError`] when the remote cannot be reached.
  pub async fn push_force(&self) -> Result<(), GitError> {
    let git = self.repo.git();
    let lease = self.lease.lock().unwrap_or_else(PoisonError::into_inner).clone();
    let pushed = crate::delegate_git!(git.worktree_push_force(self, lease.as_deref()).await)?;
    self.set_lease(pushed);
    Ok(())
  }

  /// Removes the worktree from disk and unregisters it from the clone, like `git worktree remove
  /// --force`, discarding anything not pushed. The branch itself is left in the clone.
  ///
  /// # Errors
  ///
  /// Returns a [`GitError`] when the backend cannot remove the worktree. It is still removed on a
  /// best-effort basis, as when it is dropped.
  pub async fn remove(mut self) -> Result<(), GitError> {
    let git = self.repo.git();
    crate::delegate_git!(git.worktree_remove(&self).await)?;
    self.dir.take();
    Ok(())
  }

  fn set_lease(&self, tip: String) {
    *self.lease.lock().unwrap_or_else(PoisonError::into_inner) = Some(tip);
  }
}

impl Drop for Worktree {
  /// Does on disk what `git worktree remove --force` does: deletes the worktree's admin dir in the
  /// clone (`.git/worktrees/<id>`), which its `.git` file points at, then the worktree itself.
  fn drop(&mut self) {
    let Some(dir) = self.dir.take() else {
      return;
    };
    if let Some(admin) = admin_dir(&self.path) {
      let _ = std::fs::remove_dir_all(admin);
    }
    drop(dir);
  }
}

/// The admin dir that the linked worktree at `path` is registered under, from its `.git` file.
///
/// Only a dir directly under a `worktrees` dir is returned, so a damaged `.git` file can never
/// point the caller at anything else to delete.
pub(crate) fn admin_dir(path: &Path) -> Option<PathBuf> {
  let contents = std::fs::read_to_string(path.join(".git")).ok()?;
  let admin = path.join(contents.trim().strip_prefix("gitdir:")?.trim());
  let under_worktrees = admin.parent()?.file_name()? == "worktrees";
  under_worktrees.then_some(admin)
}

impl fmt::Debug for Worktree {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("Worktree")
      .field("repo", &self.repo)
      .field("branch", &self.branch)
      .field("path", &self.path)
      .field("lease", &self.lease)
      .finish()
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn only_trusts_admin_dirs_under_worktrees() {
    let dir = TempDir::new().unwrap();
    let write = |gitdir: &str| std::fs::write(dir.path().join(".git"), format!("gitdir: {gitdir}\n")).unwrap();
    write("/repo/.git/worktrees/x");
    assert_eq!(admin_dir(dir.path()), Some(PathBuf::from("/repo/.git/worktrees/x")));
    write("../repo/.git/worktrees/x");
    assert_eq!(admin_dir(dir.path()), Some(dir.path().join("../repo/.git/worktrees/x")));
    for bad in ["/repo/.git", "/", "/home/me"] {
      write(bad);
      assert_eq!(admin_dir(dir.path()), None, "{bad}");
    }
    std::fs::remove_file(dir.path().join(".git")).unwrap();
    assert_eq!(admin_dir(dir.path()), None);
  }
}
