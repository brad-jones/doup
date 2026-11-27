//! The plugin's config, validated as soon as it is given to `configure`.

use garde::Validate;
use serde::Deserialize;
use std::sync::OnceLock;

static CONFIG: OnceLock<Config> = OnceLock::new();

const DEFAULT_API_URL: &str = "https://api.github.com";

/// The plugin's config, as given by the `with` of the repo config.
#[derive(Deserialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct Config {
  /// The repository that pull requests are managed in, eg: `brad-jones/doup`.
  #[garde(pattern(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$"))]
  pub repository: String,

  /// Authenticates requests to the GitHub API. The GraphQL API rejects unauthenticated requests.
  #[garde(pattern(r"^[!-~]+$"))]
  pub github_token: String,

  /// The base URL of the GitHub REST API, defaults to `https://api.github.com`.
  /// Set this for GitHub Enterprise Server, eg: `https://github.example.com/api/v3`.
  #[garde(inner(pattern(r"^https?://[!-~]+$")))]
  pub api_url: Option<String>,
}

impl Config {
  fn parse(json: &str) -> Result<Self, String> {
    let config: Self = serde_json::from_str(json).map_err(|err| format!("invalid config: {err}"))?;
    config.validate().map_err(|err| format!("invalid config: {err}"))?;
    Ok(config)
  }

  /// The owner and name halves of [`Self::repository`].
  pub fn owner_and_name(&self) -> (&str, &str) {
    // Validation guarantees exactly one `/`.
    self.repository.split_once('/').unwrap_or((&self.repository, ""))
  }

  /// The GraphQL endpoint for [`Self::api_url`].
  ///
  /// github.com serves GraphQL at `https://api.github.com/graphql` whereas
  /// GitHub Enterprise Server serves REST at `/api/v3` and GraphQL at `/api/graphql`.
  pub fn graphql_url(&self) -> String {
    let api_url = self.rest_base_url();
    match api_url.strip_suffix("/v3") {
      Some(base) => format!("{base}/graphql"),
      None => format!("{api_url}/graphql"),
    }
  }

  /// The REST endpoint for `path` (eg: `/repos/acme/foo/pulls`) under [`Self::api_url`].
  pub fn rest_url(&self, path: &str) -> String {
    format!("{}/{}", self.rest_base_url(), path.trim_start_matches('/'))
  }

  fn rest_base_url(&self) -> &str {
    self.api_url.as_deref().unwrap_or(DEFAULT_API_URL).trim_end_matches('/')
  }
}

/// Parses and validates `json`, then keeps it for [`get`]. May only be called once.
pub fn init(json: &str) -> Result<(), String> {
  CONFIG.set(Config::parse(json)?).map_err(|_| "already configured".to_string())
}

/// The config given to [`init`].
pub fn get() -> Result<&'static Config, String> {
  CONFIG.get().ok_or_else(|| "not configured".to_string())
}

#[cfg(test)]
mod tests {
  use super::Config;

  const VALID: &str = r#"{"repository":"acme/foo","github_token":"abc"}"#;

  #[test]
  fn a_minimal_config_is_valid() {
    let config = Config::parse(VALID).unwrap();
    assert_eq!(config.owner_and_name(), ("acme", "foo"));
    assert_eq!(config.github_token, "abc");
    assert_eq!(config.api_url, None);
  }

  #[test]
  fn required_keys_are_required() {
    assert!(Config::parse(r#"{"repository":"acme/foo"}"#).is_err());
    assert!(Config::parse(r#"{"github_token":"abc"}"#).is_err());
    assert!(Config::parse("{}").is_err());
  }

  #[test]
  fn unknown_keys_are_rejected() {
    let err = Config::parse(r#"{"repository":"acme/foo","github_token":"abc","foo":"bar"}"#).err().unwrap();
    assert!(err.contains("foo"), "{err}");
  }

  #[test]
  fn invalid_values_are_rejected() {
    for json in [
      r#"{"repository":"acme","github_token":"abc"}"#,
      r#"{"repository":"acme/foo/bar","github_token":"abc"}"#,
      r#"{"repository":"acme/foo","github_token":" "}"#,
      r#"{"repository":"acme/foo","github_token":1}"#,
      r#"{"repository":"acme/foo","github_token":"abc","api_url":"ftp://example.com"}"#,
      "[]",
    ] {
      assert!(Config::parse(json).is_err(), "{json}");
    }
  }

  #[test]
  fn the_graphql_url_is_derived_from_the_api_url() {
    let with_api_url = |api_url: Option<&str>| {
      let mut config = Config::parse(VALID).unwrap();
      config.api_url = api_url.map(str::to_string);
      config.graphql_url()
    };
    assert_eq!(with_api_url(None), "https://api.github.com/graphql");
    assert_eq!(with_api_url(Some("https://api.github.com/")), "https://api.github.com/graphql");
    assert_eq!(with_api_url(Some("https://ghe.example.com/api/v3")), "https://ghe.example.com/api/graphql");
    assert_eq!(with_api_url(Some("https://ghe.example.com/api/v3/")), "https://ghe.example.com/api/graphql");
  }

  #[test]
  fn rest_urls_are_joined_to_the_api_url() {
    let with_api_url = |api_url: Option<&str>, path: &str| {
      let mut config = Config::parse(VALID).unwrap();
      config.api_url = api_url.map(str::to_string);
      config.rest_url(path)
    };
    assert_eq!(with_api_url(None, "/repos/acme/foo/pulls"), "https://api.github.com/repos/acme/foo/pulls");
    assert_eq!(with_api_url(None, "repos/acme/foo/pulls"), "https://api.github.com/repos/acme/foo/pulls");
    assert_eq!(
      with_api_url(Some("https://ghe.example.com/api/v3/"), "/repos/acme/foo/pulls/1"),
      "https://ghe.example.com/api/v3/repos/acme/foo/pulls/1"
    );
  }
}
