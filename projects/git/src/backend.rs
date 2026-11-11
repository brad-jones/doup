use crate::{CloneOptions, GitError};
use std::future::Future;

/// A strategy for carrying out Git operations; pick one when building a [`crate::Git`].
pub trait GitBackend: Send + Sync {
  /// Clones a repository. Call through [`crate::Git::clone_repo`], which validates `options` first.
  fn clone_repo(&self, options: &CloneOptions)
  -> impl Future<Output = Result<(), GitError>> + Send;
}
