use crate::{CloneOptions, GitError};
use std::future::Future;

/// A strategy for carrying out Git operations; call it through [`crate::Git`], which validates first.
pub trait GitBackend: Send + Sync {
  /// Clones a repository as described by `options`.
  ///
  /// # Errors
  ///
  /// Returns a [`GitError`] when `options` are invalid or the clone fails.
  fn clone_repo(&self, options: &CloneOptions)
  -> impl Future<Output = Result<(), GitError>> + Send;
}
