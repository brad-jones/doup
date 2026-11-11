//! Every Git operation doup needs, behind a [`GitBackend`] that can be swapped out.
//!
//! ```no_run
//! use git::{CloneOptions, Git, GitCli};
//!
//! # async fn run() -> Result<(), git::GitError> {
//! let git = Git::new(GitCli);
//! git.clone_repo(CloneOptions::new("https://github.com/a/b.git", "b")).await?;
//! # Ok(())
//! # }
//! ```

mod auth;
mod backend;
mod cli;
mod error;
mod grit;
mod options;
mod remote_url;
mod ssh_agent;

#[cfg(test)]
mod test_support;

pub use auth::Auth;
pub use backend::GitBackend;
pub use cli::GitCli;
pub use error::GitError;
pub use grit::Grit;
pub use options::CloneOptions;

use remote_url::{RemoteUrl, redact};
use std::io::ErrorKind;
use std::path::Path;

/// The entrypoint for Git operations, delegating to the backend it was built with.
#[derive(Debug, Clone, Default)]
pub struct Git<B> {
  backend: B,
}

impl<B: GitBackend> Git<B> {
  pub fn new(backend: B) -> Self {
    Self { backend }
  }

  /// Clones a repository as described by `options`.
  ///
  /// # Errors
  ///
  /// Returns a [`GitError`] when `options` are invalid or the clone fails.
  pub async fn clone_repo(&self, options: CloneOptions) -> Result<(), GitError> {
    validate(&options).await?;
    self.backend.clone_repo(&options).await
  }
}

/// Checks done up front so both backends reject bad input identically.
async fn validate(options: &CloneOptions) -> Result<(), GitError> {
  let remote = RemoteUrl::parse(options.url())?;
  if let Some(auth) = options.auth() {
    let matches_remote = matches!(
      (auth, &remote),
      (Auth::Https { .. }, RemoteUrl::Http(_)) | (Auth::Ssh { .. }, RemoteUrl::Ssh)
    );
    if !matches_remote {
      return Err(GitError::AuthMismatch {
        url: redact(options.url()),
        auth: auth.kind(),
      });
    }
  }
  if options
    .branch()
    .is_some_and(|branch| branch.trim().is_empty())
  {
    return Err(GitError::EmptyBranch);
  }
  ensure_empty_destination(options.destination()).await
}

async fn ensure_empty_destination(path: &Path) -> Result<(), GitError> {
  let not_empty = || GitError::DestinationNotEmpty {
    path: path.to_path_buf(),
  };
  match tokio::fs::read_dir(path).await {
    Ok(mut entries) => match entries.next_entry().await? {
      Some(_) => Err(not_empty()),
      None => Ok(()),
    },
    Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
    Err(err) if err.kind() == ErrorKind::NotADirectory => Err(not_empty()),
    Err(err) => Err(err.into()),
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::sync::Mutex;
  use tempfile::TempDir;

  /// Records what reaches the backend so validation can be tested without Git.
  #[derive(Default)]
  struct Recorder(Mutex<Vec<CloneOptions>>);

  impl GitBackend for Recorder {
    async fn clone_repo(&self, options: &CloneOptions) -> Result<(), GitError> {
      self.0.lock().unwrap().push(options.clone());
      Ok(())
    }
  }

  async fn clone_with(options: CloneOptions) -> (Result<(), GitError>, usize) {
    let git = Git::new(Recorder::default());
    let result = git.clone_repo(options).await;
    let calls = git.backend.0.lock().unwrap().len();
    (result, calls)
  }

  fn https() -> Auth {
    Auth::Https {
      user: "u".into(),
      pass: "p".into(),
    }
  }

  fn ssh() -> Auth {
    Auth::Ssh { key: "k".into() }
  }

  #[tokio::test]
  async fn delegates_valid_options_to_the_backend() {
    let dir = TempDir::new().unwrap();
    let options = CloneOptions::new("https://github.com/a/b.git", dir.path().join("b"))
      .with_branch("main")
      .with_auth(https());
    let (result, calls) = clone_with(options).await;
    result.unwrap();
    assert_eq!(calls, 1);
  }

  #[tokio::test]
  async fn accepts_an_existing_empty_destination() {
    let dir = TempDir::new().unwrap();
    let (result, _) = clone_with(CloneOptions::new("git@github.com:a/b.git", dir.path())).await;
    result.unwrap();
  }

  #[tokio::test]
  async fn rejects_a_non_empty_destination() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("file"), "x").unwrap();
    let (result, calls) = clone_with(CloneOptions::new("git@github.com:a/b.git", dir.path())).await;
    assert!(matches!(result, Err(GitError::DestinationNotEmpty { .. })));
    assert_eq!(calls, 0);
  }

  #[tokio::test]
  async fn rejects_a_file_as_destination() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("file");
    std::fs::write(&file, "x").unwrap();
    let (result, _) = clone_with(CloneOptions::new("git@github.com:a/b.git", file)).await;
    assert!(matches!(result, Err(GitError::DestinationNotEmpty { .. })));
  }

  #[tokio::test]
  async fn rejects_invalid_urls() {
    let (result, calls) = clone_with(CloneOptions::new("", "/tmp/never")).await;
    assert!(matches!(result, Err(GitError::InvalidUrl { .. })));
    assert_eq!(calls, 0);
  }

  #[tokio::test]
  async fn rejects_auth_that_does_not_match_the_url() {
    for (url, auth) in [
      ("git@github.com:a/b.git", https()),
      ("file:///srv/repo.git", https()),
      ("https://github.com/a/b.git", ssh()),
      ("/srv/repo.git", ssh()),
    ] {
      let (result, calls) = clone_with(CloneOptions::new(url, "/tmp/never").with_auth(auth)).await;
      assert!(
        matches!(result, Err(GitError::AuthMismatch { .. })),
        "{url}"
      );
      assert_eq!(calls, 0);
    }
  }

  #[tokio::test]
  async fn rejects_an_empty_branch() {
    let options = CloneOptions::new("git@github.com:a/b.git", "/tmp/never").with_branch(" ");
    let (result, _) = clone_with(options).await;
    assert!(matches!(result, Err(GitError::EmptyBranch)));
  }
}
