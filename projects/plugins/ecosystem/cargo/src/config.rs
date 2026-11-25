//! The plugin's config, validated as soon as it is given to `configure`.

use garde::Validate;
use serde::Deserialize;
use std::sync::OnceLock;

static CONFIG: OnceLock<Config> = OnceLock::new();

/// The plugin's config, as given by the `with` of the repo config.
#[derive(Deserialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct Config {
  /// Authenticates requests to the GitHub API, which are otherwise limited to 60 an hour.
  #[garde(inner(pattern(r"^[!-~]+$")))]
  pub github_token: Option<String>,
}

impl Config {
  fn parse(json: &str) -> Result<Self, String> {
    let config: Self = serde_json::from_str(json).map_err(|err| format!("invalid config: {err}"))?;
    config.validate().map_err(|err| format!("invalid config: {err}"))?;
    Ok(config)
  }
}

/// Parses and validates `json`, then keeps it for [`get`]. May only be called once.
pub fn init(json: &str) -> Result<(), String> {
  CONFIG.set(Config::parse(json)?).map_err(|_| "already configured".to_string())
}

/// The config given to [`init`], if there was one.
pub fn get() -> Option<&'static Config> {
  CONFIG.get()
}

#[cfg(test)]
mod tests {
  use super::Config;

  #[test]
  fn an_empty_config_is_valid() {
    assert_eq!(Config::parse("{}").unwrap().github_token, None);
  }

  #[test]
  fn the_github_token_is_read() {
    assert_eq!(Config::parse(r#"{"github_token":"abc"}"#).unwrap().github_token.as_deref(), Some("abc"));
  }

  #[test]
  fn unknown_keys_are_rejected() {
    let err = Config::parse(r#"{"githubTokn":"abc"}"#).err().unwrap();
    assert!(err.contains("githubTokn"), "{err}");
  }

  #[test]
  fn invalid_values_are_rejected() {
    assert!(Config::parse(r#"{"github_token":1}"#).is_err());
    assert!(Config::parse(r#"{"github_token":" "}"#).is_err());
    assert!(Config::parse("[]").is_err());
  }
}
