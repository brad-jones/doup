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

  /// The var's value is left out, as it may be a secret.
  #[error(
    "env var '{name}' is not a valid config path: segments are separated by `__` and must not be empty, or start or end with `_`"
  )]
  InvalidEnvVar { name: String },

  /// `path` is the `key_path` as written; `searched` is every place it was looked for, in order.
  #[error("key file '{}' not found, looked for {}", path.display(), quoted_paths(searched))]
  SshKeyNotFound { path: PathBuf, searched: Vec<PathBuf> },

  #[error("{} is not a valid OpenSSH private key", key_label(path))]
  InvalidSshKey { path: Option<PathBuf>, source: ssh_key::Error },

  #[error("{} must not be protected by a passphrase", key_label(path))]
  EncryptedSshKey { path: Option<PathBuf> },

  #[error("plugin '{plugin}' does not have a valid semver version requirement")]
  InvalidVersionRequirement { plugin: String, source: semver::Error },

  #[error("failed to read lock file '{}'", path.display())]
  ReadLock { path: PathBuf, source: std::io::Error },

  #[error("failed to write lock file '{}'", path.display())]
  WriteLock { path: PathBuf, source: std::io::Error },

  #[error("failed to parse lock file")]
  ParseLock(#[source] toml::de::Error),

  #[error("failed to serialize lock file")]
  SerializeLock(#[source] toml::ser::Error),

  #[error("lock file version {version} is not supported, it was probably written by a newer doup")]
  UnsupportedLockVersion { version: u64 },
}

fn key_label(path: &Option<PathBuf>) -> String {
  match path {
    Some(path) => format!("key file '{}'", path.display()),
    None => "key".to_string(),
  }
}

fn quoted_paths(paths: &[PathBuf]) -> String {
  paths.iter().map(|path| format!("'{}'", path.display())).collect::<Vec<_>>().join(" and ")
}
