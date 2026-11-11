use crate::Auth;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};

/// What to clone, where to put it, and how to authenticate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloneOptions {
  url: String,
  destination: PathBuf,
  branch: Option<String>,
  depth: Option<NonZeroU32>,
  auth: Option<Auth>,
}

impl CloneOptions {
  /// Clone `url` into `destination`, which must not exist or be an empty directory.
  pub fn new(url: impl Into<String>, destination: impl Into<PathBuf>) -> Self {
    Self {
      url: url.into(),
      destination: destination.into(),
      branch: None,
      depth: None,
      auth: None,
    }
  }

  /// Check out this branch instead of the remote's default branch.
  #[must_use]
  pub fn with_branch(mut self, branch: impl Into<String>) -> Self {
    self.branch = Some(branch.into());
    self
  }

  /// Make a shallow clone holding only this many commits of history.
  #[must_use]
  pub fn with_depth(mut self, depth: NonZeroU32) -> Self {
    self.depth = Some(depth);
    self
  }

  /// Authenticate against the remote; without this the user's existing Git setup is used.
  #[must_use]
  pub fn with_auth(mut self, auth: Auth) -> Self {
    self.auth = Some(auth);
    self
  }

  pub fn url(&self) -> &str {
    &self.url
  }

  pub fn destination(&self) -> &Path {
    &self.destination
  }

  pub fn branch(&self) -> Option<&str> {
    self.branch.as_deref()
  }

  pub fn depth(&self) -> Option<NonZeroU32> {
    self.depth
  }

  pub fn auth(&self) -> Option<&Auth> {
    self.auth.as_ref()
  }
}
