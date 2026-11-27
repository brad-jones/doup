use std::path::PathBuf;

/// Every way loading, validating or saving a doup config can fail.
///
/// Messages only describe their own context, the underlying cause is always available via
/// [`std::error::Error::source`], so render with `{:#}` (anyhow) or walk the chain to see it all.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConfigError {
  #[error("failed to determine the current directory")]
  CurrentDir(#[source] std::io::Error),

  #[error("failed to read config file '{}'", path.display())]
  Read { path: PathBuf, source: std::io::Error },

  #[error("failed to write config file '{}'", path.display())]
  Write { path: PathBuf, source: std::io::Error },

  #[error("unsupported config file extension: '{}'", path.display())]
  UnsupportedExtension { path: PathBuf },

  #[error("config has no source path; use to_file instead")]
  NoSource,

  /// Wraps any parse, validation or hydration failure with the file it came from.
  #[error("invalid config file '{}'", path.display())]
  File { path: PathBuf, source: Box<ConfigError> },

  #[error("failed to parse config as JSON")]
  ParseJson(#[source] serde_json::Error),

  #[error("failed to parse config as YAML")]
  ParseYaml(#[source] serde_saphyr::Error),

  #[error("failed to serialize config as JSON")]
  SerializeJson(#[source] serde_json::Error),

  #[error("failed to serialize config as YAML")]
  SerializeYaml(#[source] serde_saphyr::SerializeError),

  /// Boxed as a `ValidationError` is large and would bloat every `Result`.
  #[error("config does not match its schema")]
  Schema(#[source] Box<jsonschema::ValidationError<'static>>),

  #[error("config matched its schema but could not be deserialized")]
  Deserialize(#[source] serde_json::Error),

  #[error("failed to read key '{}'", path.display())]
  ReadSshKey { path: PathBuf, source: std::io::Error },

  #[error("{} is not a valid OpenSSH private key", key_label(path))]
  InvalidSshKey { path: Option<PathBuf>, source: ssh_key::Error },

  #[error("{} must not be protected by a passphrase", key_label(path))]
  EncryptedSshKey { path: Option<PathBuf> },
}

fn key_label(path: &Option<PathBuf>) -> String {
  match path {
    Some(path) => format!("key file '{}'", path.display()),
    None => "key".to_string(),
  }
}
