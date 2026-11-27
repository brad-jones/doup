//! The plugin's errors, converted to the WIT `guest-error` at the export boundary.

use crate::bindings::doup::plugin::types::{ErrorKind, GuestError};
use std::error::Error;

/// Every way the plugin can fail. The GitHub token is never included in an error.
#[derive(Debug, thiserror::Error)]
pub enum GithubError {
  #[error("invalid config")]
  ParseConfig(#[source] serde_json::Error),

  #[error("invalid config")]
  ValidateConfig(#[source] garde::Report),

  #[error("already configured")]
  AlreadyConfigured,

  #[error("not configured")]
  NotConfigured,

  #[error("invalid pull request number {0}")]
  InvalidPrNumber(i64),

  /// `0` names what was empty, eg: `title`.
  #[error("{0} must not be empty")]
  Empty(&'static str),

  #[error("invalid version {0:?}, it must be non-empty without whitespace or `-->`")]
  InvalidVersion(String),

  #[error("request to {url} failed")]
  Request { url: String, source: wasi_fetch::Error },

  #[error("failed to read {url}")]
  ReadBody { url: String, source: wasi_fetch::Error },

  /// `message` is GitHub's own description of the error, when it gave one.
  #[error("request to {url} returned {status}{}", message.as_ref().map(|m| format!(": {m}")).unwrap_or_default())]
  Status { url: String, status: u16, message: Option<String> },

  #[error("request to {url} returned invalid JSON")]
  InvalidJson { url: String, source: serde_json::Error },

  /// Wraps a failure parsing a GraphQL response with the request it came from.
  #[error("request to {url} failed")]
  GraphqlResponse { url: String, source: Box<GithubError> },

  #[error("invalid GraphQL response")]
  InvalidGraphql(#[source] serde_json::Error),

  #[error("{}", .0.join("; "))]
  Graphql(Vec<String>),

  #[error("GraphQL response has no data")]
  NoData,

  /// `what` describes the response, eg: `pull request`.
  #[error("invalid GitHub {what} response")]
  InvalidResponse { what: &'static str, source: serde_json::Error },

  #[error("repository not found, or the github_token can not access it")]
  RepositoryNotFound,
}

impl GithubError {
  /// The broad category reported to the host.
  pub fn kind(&self) -> ErrorKind {
    match self {
      Self::ParseConfig(_) | Self::ValidateConfig(_) | Self::AlreadyConfigured | Self::NotConfigured => {
        ErrorKind::InvalidConfig
      }
      Self::InvalidPrNumber(_) | Self::Empty(_) | Self::InvalidVersion(_) => ErrorKind::InvalidInput,
      Self::Status { status: 404, .. } | Self::RepositoryNotFound => ErrorKind::NotFound,
      Self::Request { .. } | Self::ReadBody { .. } | Self::Status { .. } | Self::Graphql(_) => ErrorKind::Http,
      Self::InvalidJson { .. } | Self::InvalidGraphql(_) | Self::NoData | Self::InvalidResponse { .. } => {
        ErrorKind::InvalidResponse
      }
      Self::GraphqlResponse { source, .. } => source.kind(),
    }
  }
}

impl From<GithubError> for GuestError {
  fn from(error: GithubError) -> Self {
    let causes = std::iter::successors(error.source(), |&err| err.source()).map(ToString::to_string).collect();
    GuestError { kind: error.kind(), message: error.to_string(), causes }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn guest_errors_carry_the_kind_and_causes() {
    let error = GithubError::GraphqlResponse {
      url: "https://api.github.com/graphql".to_string(),
      source: Box::new(GithubError::RepositoryNotFound),
    };
    let guest = GuestError::from(error);
    assert_eq!(guest.kind, ErrorKind::NotFound);
    assert_eq!(guest.message, "request to https://api.github.com/graphql failed");
    assert_eq!(guest.causes, ["repository not found, or the github_token can not access it"]);
  }

  #[test]
  fn a_404_is_not_found() {
    let status = |status| GithubError::Status { url: "u".to_string(), status, message: None }.kind();
    assert_eq!(status(404), ErrorKind::NotFound);
    assert_eq!(status(500), ErrorKind::Http);
  }
}
