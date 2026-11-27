//! The plugin's errors, converted to the WIT `guest-error` at the export boundary.

use crate::bindings::doup::plugin::{
  host_api::RunError,
  types::{ErrorKind, GuestError},
};
use std::error::Error;
use std::path::PathBuf;

/// Every way the plugin can fail. The GitHub token is never included in an error.
#[derive(Debug, thiserror::Error)]
pub enum CargoError {
  #[error("invalid config")]
  ParseConfig(#[source] serde_json::Error),

  #[error("invalid config")]
  ValidateConfig(#[source] garde::Report),

  #[error("already configured")]
  AlreadyConfigured,

  #[error("failed to read {}", path.display())]
  Read { path: PathBuf, source: std::io::Error },

  #[error("failed to write {}", path.display())]
  Write { path: PathBuf, source: std::io::Error },

  #[error("failed to walk {}", path.display())]
  Walk { path: PathBuf, source: ignore::Error },

  #[error("{} is not beneath {}", path.display(), root.display())]
  OutsideRoot { path: PathBuf, root: PathBuf, source: std::path::StripPrefixError },

  #[error("failed to parse {}", path.display())]
  ParseToml { path: PathBuf, source: toml::de::Error },

  #[error("failed to parse {}", path.display())]
  ParseTomlEdit { path: PathBuf, source: toml_edit::TomlError },

  /// Wraps a failure to update a manifest with its path.
  #[error("failed to update {}", path.display())]
  Manifest { path: PathBuf, source: Box<CargoError> },

  /// Wraps a failure to read a declaration with the manifest & dependency it's from.
  #[error("invalid dependency {dependency} in {}", manifest.display())]
  Declaration { manifest: PathBuf, dependency: String, source: Box<CargoError> },

  /// Wraps a failure to update a lockfile with its path, relative to the workspace.
  #[error("failed to update {path}")]
  Lockfile { path: String, source: Box<CargoError> },

  #[error("invalid version requirement `{requirement}` for {name}")]
  InvalidRequirement { name: String, requirement: String, source: semver::Error },

  #[error("invalid locked version `{version}`")]
  InvalidLockedVersion { version: String, source: semver::Error },

  #[error("invalid version `{version}` for {name}")]
  InvalidVersion { name: String, version: String, source: semver::Error },

  #[error("{name} has no occurrences")]
  NoOccurrences { name: String },

  #[error("registry `{alias}` is not defined in any .cargo/config.toml")]
  UndefinedRegistry { alias: String },

  #[error("registry `{alias}` uses `{url}`, only sparse+https:// indexes are supported")]
  UnsupportedRegistry { alias: String, url: String },

  #[error("request to {url} failed")]
  Request { url: String, source: wasi_fetch::Error },

  #[error("failed to read {url}")]
  ReadBody { url: String, source: wasi_fetch::Error },

  #[error("request to {url} returned {status}")]
  Status { url: String, status: u16 },

  #[error("invalid index entry for {name}")]
  InvalidIndexEntry { name: String, source: serde_json::Error },

  /// `what` describes the response, eg: `crates.io`.
  #[error("invalid {what} response")]
  InvalidResponse { what: &'static str, source: serde_json::Error },

  /// `command` is what was run in the container, eg: `cargo update --workspace`.
  #[error("`{command}` {}", describe_run_error(error))]
  Container { command: String, error: RunError },
}

impl CargoError {
  /// The broad category reported to the host.
  pub fn kind(&self) -> ErrorKind {
    match self {
      Self::ParseConfig(_) | Self::ValidateConfig(_) | Self::AlreadyConfigured => ErrorKind::InvalidConfig,
      Self::Read { .. } | Self::Write { .. } | Self::Walk { .. } | Self::OutsideRoot { .. } => ErrorKind::Io,
      Self::ParseToml { .. }
      | Self::ParseTomlEdit { .. }
      | Self::InvalidRequirement { .. }
      | Self::InvalidLockedVersion { .. }
      | Self::NoOccurrences { .. }
      | Self::UndefinedRegistry { .. }
      | Self::UnsupportedRegistry { .. } => ErrorKind::InvalidWorkspace,
      Self::InvalidVersion { .. } => ErrorKind::InvalidInput,
      Self::Status { status: 404, .. } => ErrorKind::NotFound,
      Self::Request { .. } | Self::ReadBody { .. } | Self::Status { .. } => ErrorKind::Http,
      Self::InvalidIndexEntry { .. } | Self::InvalidResponse { .. } => ErrorKind::InvalidResponse,
      Self::Container { .. } => ErrorKind::Container,
      Self::Manifest { source, .. } | Self::Declaration { source, .. } | Self::Lockfile { source, .. } => source.kind(),
    }
  }
}

impl From<CargoError> for GuestError {
  fn from(error: CargoError) -> Self {
    let causes = std::iter::successors(error.source(), |&err| err.source()).map(ToString::to_string).collect();
    GuestError { kind: error.kind(), message: error.to_string(), causes }
  }
}

/// Completes the sentence "`<command>` …", as the generated `RunError` only has a `Debug` style `Display`.
fn describe_run_error(error: &RunError) -> String {
  match error {
    RunError::NotPermitted => "is not permitted to run a container here".to_string(),
    RunError::InvalidRequest(message) => format!("is an invalid container request: {message}"),
    RunError::Runtime(message) => format!("could not be run: {message}"),
    RunError::Exited(exit) => {
      let code = exit.code.map_or_else(|| "unknown".to_string(), |code| code.to_string());
      match exit.stderr.trim() {
        "" => format!("exited with code {code}"),
        stderr => format!("exited with code {code}: {stderr}"),
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::bindings::doup::plugin::host_api::RunExit;

  #[test]
  fn wrapped_errors_keep_the_kind_of_their_cause() {
    let error = CargoError::Lockfile {
      path: "Cargo.lock".to_string(),
      source: Box::new(CargoError::Container {
        command: "cargo update".to_string(),
        error: RunError::Exited(RunExit {
          code: Some(101),
          stdout: String::new(),
          stderr: "error: boom\n".to_string(),
        }),
      }),
    };
    let guest = GuestError::from(error);
    assert_eq!(guest.kind, ErrorKind::Container);
    assert_eq!(guest.message, "failed to update Cargo.lock");
    assert_eq!(guest.causes, ["`cargo update` exited with code 101: error: boom"]);
  }
}
