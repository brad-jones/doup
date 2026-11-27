//! The errors returned by this crate.
//!
//! - [`PluginError`]: every way loading or calling a plugin can fail.
//! - [`GuestError`]: an error a plugin returned from one of its exports, carried by
//!   [`PluginError::Configure`] & [`PluginError::Guest`]. It is built from the WIT `guest-error`
//!   record so its [`GuestErrorKind`] can be matched on and its causes form a regular error chain.

use crate::host::types;
use std::fmt;
use std::path::PathBuf;

/// The broad category of a [`GuestError`], as reported by the plugin.
pub use crate::host::types::ErrorKind as GuestErrorKind;

/// An error from wasmtime, which does not implement [`std::error::Error`] itself.
pub type WasmtimeError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Every way loading or calling a plugin can fail.
///
/// Messages only describe their own context, the underlying cause is always available via
/// [`std::error::Error::source`], so render with `{:#}` (anyhow) or walk the chain to see it all.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PluginError {
  /// `searched` is every place the plugin was looked for, in order.
  #[error("plugin '{plugin}' not found, looked for {}", quoted_paths(searched))]
  NotFound { plugin: String, searched: Vec<PathBuf> },

  #[error("plugin '{plugin}' is a wkg package reference, which is not supported yet")]
  WkgUnsupported { plugin: String },

  #[error("failed to determine the current directory")]
  CurrentDir(#[source] std::io::Error),

  #[error("failed to create the wasmtime engine")]
  Engine(#[source] WasmtimeError),

  #[error("failed to create the plugin linker")]
  Linker(#[source] WasmtimeError),

  /// The component could not be read, compiled or linked against the host.
  #[error("failed to load plugin '{}'", path.display())]
  Load { path: PathBuf, source: WasmtimeError },

  /// The plugin's sandbox could not be created, eg: a directory to preopen does not exist.
  #[error("failed to create the sandbox for plugin '{plugin}'")]
  Sandbox { plugin: String, source: WasmtimeError },

  #[error("failed to instantiate plugin '{plugin}'")]
  Instantiate { plugin: String, source: WasmtimeError },

  #[error("failed to serialize the config of plugin '{plugin}'")]
  SerializeConfig { plugin: String, source: serde_json::Error },

  #[error("plugin '{plugin}' rejected its config")]
  Configure { plugin: String, source: GuestError },

  /// The guest trapped (eg: panicked) or the call failed within wasmtime, `export` is its WIT name.
  #[error("plugin '{plugin}' trapped in '{export}'")]
  Trap { plugin: String, export: &'static str, source: WasmtimeError },

  /// The guest returned an error from `export`, its WIT name.
  #[error("plugin '{plugin}' failed in '{export}'")]
  Guest { plugin: String, export: &'static str, source: GuestError },

  #[error("failed to create a temporary directory")]
  TempDir(#[source] std::io::Error),
}

impl PluginError {
  /// The error the plugin itself returned, if any.
  pub fn guest_error(&self) -> Option<&GuestError> {
    match self {
      Self::Configure { source, .. } | Self::Guest { source, .. } => Some(source),
      _ => None,
    }
  }
}

/// An error returned by a plugin, rebuilt from the WIT `guest-error` record.
///
/// [`Display`](fmt::Display) shows only the plugin's own message, the causes it reported are
/// available as a chain of [`std::error::Error::source`]s, or all at once from [`Self::causes`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuestError {
  kind: GuestErrorKind,
  message: String,
  source: Option<Box<GuestCause>>,
}

impl GuestError {
  /// The broad category of the error, as reported by the plugin.
  pub fn kind(&self) -> GuestErrorKind {
    self.kind
  }

  /// The plugin's description of the error, excluding its causes.
  pub fn message(&self) -> &str {
    &self.message
  }

  /// The messages of the underlying causes, outermost first.
  pub fn causes(&self) -> impl Iterator<Item = &str> {
    std::iter::successors(self.source.as_deref(), |cause| cause.source.as_deref()).map(|cause| cause.message.as_str())
  }
}

impl From<types::GuestError> for GuestError {
  fn from(error: types::GuestError) -> Self {
    // Built innermost first, so each cause can own the one below it.
    let source =
      error.causes.into_iter().rev().fold(None, |source, message| Some(Box::new(GuestCause { message, source })));
    Self { kind: error.kind, message: error.message, source }
  }
}

impl fmt::Display for GuestError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str(&self.message)
  }
}

impl std::error::Error for GuestError {
  fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
    self.source.as_deref().map(|cause| cause as _)
  }
}

/// One link in a [`GuestError`]'s chain of causes, the plugin only sends their messages.
#[derive(Debug, Clone, PartialEq, Eq)]
struct GuestCause {
  message: String,
  source: Option<Box<GuestCause>>,
}

impl fmt::Display for GuestCause {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str(&self.message)
  }
}

impl std::error::Error for GuestCause {
  fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
    self.source.as_deref().map(|cause| cause as _)
  }
}

fn quoted_paths(paths: &[PathBuf]) -> String {
  paths.iter().map(|path| format!("'{}'", path.display())).collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::error::Error;

  #[test]
  fn guest_errors_rebuild_their_cause_chain() {
    let error = GuestError::from(types::GuestError {
      kind: GuestErrorKind::Io,
      message: "failed to read Cargo.toml".to_string(),
      causes: vec!["permission denied".to_string(), "os error 13".to_string()],
    });

    assert_eq!(error.kind(), GuestErrorKind::Io);
    assert_eq!(error.to_string(), "failed to read Cargo.toml");
    assert_eq!(error.causes().collect::<Vec<_>>(), ["permission denied", "os error 13"]);

    let mut chain = Vec::new();
    let mut next: Option<&dyn Error> = Some(&error);
    while let Some(err) = next {
      chain.push(err.to_string());
      next = err.source();
    }
    assert_eq!(chain, ["failed to read Cargo.toml", "permission denied", "os error 13"]);
  }
}
