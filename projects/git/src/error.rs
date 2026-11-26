use std::path::PathBuf;
use std::process::ExitStatus;

/// Every way a Git operation can fail, regardless of which backend ran it.
///
/// Remote URLs held by these variants never include credentials.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum GitError {
  #[error("invalid remote url '{url}'")]
  InvalidUrl { url: String },

  #[error("{auth} authentication cannot be used with remote url '{url}'")]
  AuthMismatch { url: String, auth: &'static str },

  #[error("ssh path '{}' cannot be passed to ssh", path.display())]
  InvalidSshPath { path: PathBuf },

  #[error("ssh key is not a valid OpenSSH private key")]
  InvalidSshKey(#[source] ssh_key::Error),

  #[error("ssh key must not be protected by a passphrase")]
  EncryptedSshKey,

  #[error("ssh key type '{algorithm}' is not supported")]
  UnsupportedSshKey { algorithm: String },

  #[error("Windows' OpenSSH client was not found at '{}'", path.display())]
  SshNotFound { path: PathBuf },

  #[error("failed to start the in-memory ssh agent")]
  SshAgent(#[source] std::io::Error),

  #[error("branch name must not be empty")]
  EmptyBranch,

  #[error("'{branch}' is not a valid branch name")]
  InvalidBranch { branch: String },

  #[error("'{name} <{email}>' is not a valid commit identity")]
  InvalidIdentity { name: String, email: String },

  #[error("commit message must not be empty")]
  EmptyCommitMessage,

  #[error("there are no changes to commit")]
  NothingToCommit,

  #[error("push to remote branch '{branch}' was rejected: {reason}")]
  PushRejected { branch: String, reason: String },

  #[error("remote branch '{branch}' changed since it was last seen, so it was not overwritten")]
  StaleLease { branch: String },

  #[error("destination '{}' already exists and is not an empty directory", path.display())]
  DestinationNotEmpty { path: PathBuf },

  #[error("authentication failed for '{url}'")]
  AuthenticationFailed { url: String },

  #[error("repository '{url}' not found")]
  RepositoryNotFound { url: String },

  #[error("remote branch '{branch}' not found")]
  BranchNotFound { branch: String },

  #[error("remote '{url}' has no default branch")]
  NoDefaultBranch { url: String },

  #[error("remote branch '{branch}' has no commits that are not already on the default branch")]
  NoUniqueCommits { branch: String },

  #[error("commit '{id}' could not be parsed")]
  InvalidCommit { id: String },

  #[error("remote '{url}' is unreachable: {reason}")]
  RemoteUnreachable { url: String, reason: String },

  #[error("the git executable was not found on PATH")]
  GitNotInstalled,

  #[error("git failed ({status}): {stderr}")]
  CommandFailed { status: ExitStatus, stderr: String },

  #[error(transparent)]
  Grit(grit_lib::error::Error),

  #[error(transparent)]
  Io(#[from] std::io::Error),

  #[error("background git task failed")]
  Join(#[from] tokio::task::JoinError),
}
