use super::file::ConfigFile;
use anyhow::{Context, Result};
use jsonschema::ValidationError;
use serde::{Deserialize, Serialize, Serializer, ser::Error as _};
use serde_json::Value;
use std::fs::read_to_string;
use std::path::{Path, PathBuf};

/// Embed the Json Schema, we will use this for runtime validation.
#[jsonschema::validator(path = "../../schemas/bot.schema.json")]
struct DoupBotConfigFileSchema;

/// Configuration to bootstrap the doup dependency updater bot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DoupBotConfigFile {
  /// Ensure `$schema` round trips if the user has added it to their config.
  /// see: https://json-schema.org/understanding-json-schema/reference/schema
  #[serde(rename = "$schema", default, skip_serializing_if = "Option::is_none")]
  schema_uri: Option<String>,

  /// The git implementation used to clone every repo. Applies globally.
  #[serde(default, skip_serializing_if = "is_default")]
  pub git_backend: GitBackendKind,

  /// The git repositories that the bot will update.
  #[serde(default)]
  pub repos: Vec<DoupBotRepo>,

  /// Where this config was loaded from; `None` if built in memory.
  #[serde(skip)]
  source: Option<PathBuf>,
}

impl ConfigFile for DoupBotConfigFile {
  const DEFAULT_NAMES: &'static [&'static str] = &["doupbot.json", "doupbot.yaml", "doupbot.yml"];

  const ENV_PREFIX: &'static str = "DOUPBOT";

  const VALIDATE: for<'i> fn(&'i Value) -> Result<(), ValidationError<'i>> =
    DoupBotConfigFileSchema::validate;

  fn source(&self) -> Option<&Path> {
    self.source.as_deref()
  }

  fn set_source(&mut self, path: PathBuf) {
    self.source = Some(path)
  }

  /// Reads any `key_path` into `key` and checks every SSH key is a usable OpenSSH key.
  fn hydrate(&mut self) -> Result<()> {
    for repo in &mut self.repos {
      let Some(DoupBotRepoAuth::Ssh { key, key_path }) = &mut repo.auth else {
        continue;
      };
      if let Some(path) = key_path {
        let text = read_to_string(&path)
          .with_context(|| format!("failed to read key {}", path.display()))?;
        *key = Some(text);
      }
      let text = key.as_deref().unwrap_or_default();
      validate_openssh_key(text).with_context(|| match key_path {
        Some(path) => format!("invalid key file {}", path.display()),
        None => "invalid key".to_string(),
      })?;
    }
    Ok(())
  }
}

/// Which git implementation to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitBackendKind {
  /// Shells out to the `git` executable.
  #[default]
  GitCli,

  /// Uses the pure Rust `grit-lib` implementation.
  Grit,
}

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
  *value == T::default()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DoupBotRepo {
  /// The git origin URL (https or ssh accepted). If no authentication details
  /// are given, doup assumes git is already configured correctly.
  pub origin: String,

  /// The authentication strategy to use against the origin.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub auth: Option<DoupBotRepoAuth>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum DoupBotRepoAuth {
  Https {
    /// The username to use with a HTTPS origin.
    user: String,

    /// The password to use with a HTTPS origin.
    pass: String,
  },
  Ssh {
    /// The private key to use with with an SSH origin. Must not be protected by a passphrase.
    /// Always `Some` once loaded, even when it was given as a `key_path`.
    ///
    /// In OpenSSH format. eg:
    ///
    /// ```txt
    /// -----BEGIN OPENSSH PRIVATE KEY-----
    /// ...
    /// -----END OPENSSH PRIVATE KEY-----
    /// ```
    #[serde(default)]
    key: Option<String>,

    /// Set when the key was given as a file path, so that it round trips as a path.
    #[serde(default)]
    key_path: Option<PathBuf>,
  },
}

/// The schema allows `key` or `key_path` but never both, so only one is written back out.
impl Serialize for DoupBotRepoAuth {
  fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
    #[derive(Serialize)]
    #[serde(untagged)]
    enum Out<'a> {
      Https { user: &'a str, pass: &'a str },
      Key { key: &'a str },
      KeyPath { key_path: &'a Path },
    }

    match self {
      Self::Https { user, pass } => Out::Https { user, pass }.serialize(serializer),
      Self::Ssh {
        key_path: Some(key_path),
        ..
      } => Out::KeyPath { key_path }.serialize(serializer),
      Self::Ssh {
        key: Some(key),
        key_path: None,
      } => Out::Key { key }.serialize(serializer),
      Self::Ssh { .. } => Err(S::Error::custom("ssh auth needs a key or a key_path")),
    }
  }
}

fn validate_openssh_key(text: &str) -> Result<()> {
  let key = ssh_key::PrivateKey::from_openssh(text)?;
  anyhow::ensure!(
    !key.is_encrypted(),
    "key must not be protected by a passphrase"
  );
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::config::env::vars_to_value;
  use crate::config::file::{Loaded, load_with_env};
  use assert_fs::{TempDir, prelude::*};
  use serde_json::json;

  const KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
QyNTUxOQAAACDKd+K7eJM8z8sJsrpdUXVA8hBGUJp64dup4cIBPuct5gAAAIifO9uBnzvb
gQAAAAtzc2gtZWQyNTUxOQAAACDKd+K7eJM8z8sJsrpdUXVA8hBGUJp64dup4cIBPuct5g
AAAEA6+0Orvqxdr2p9QlEVw8/9gv6ZvCln+LCN5AniT8H4L8p34rt4kzzPywmyul1RdUDy
EEZQmnrh26nhwgE+5y3mAAAAAAECAwQF
-----END OPENSSH PRIVATE KEY-----
";

  const ENCRYPTED_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAACmFlczI1Ni1jdHIAAAAGYmNyeXB0AAAAGAAAABBDpXpMKy
CdjRXpTI1eQiu3AAAAGAAAAAEAAAAzAAAAC3NzaC1lZDI1NTE5AAAAIHoVXI0VfEIfNnNd
ldB06YkKBCP507gNV+SOAG1N7sDBAAAAkPmfdNXYgE3sy3LHiexh3dWD6z+46Ha+56zQsP
MTgSCv2R3P2TKcAWM6xBz2F+DPl2LmJjPnQpjnWmsSdQide8PEFAWBzMwD+5eqbaAYc9CV
e2ib0aFVKiVUzE6DRDzus8KkMcXKYrMd1Rqveqfv0ViLubdeLr4ELJewvgpCOVvbKVNoCH
y5yYJ2PSeOZakF6g==
-----END OPENSSH PRIVATE KEY-----
";

  fn config_json(repo: serde_json::Value) -> String {
    json!({ "repos": [repo] }).to_string()
  }

  fn only_repo(config: DoupBotConfigFile) -> DoupBotRepo {
    config.repos.into_iter().next().unwrap()
  }

  #[test]
  fn none_auth_round_trips() {
    let json = config_json(json!({ "origin": "https://github.com/a/b.git" }));
    let config = DoupBotConfigFile::from_json(&json).unwrap();
    assert_eq!(only_repo(config.clone()).auth, None);
    assert_eq!(
      DoupBotConfigFile::from_json(&config.to_json().unwrap()).unwrap(),
      config
    );
  }

  #[test]
  fn https_auth_round_trips() {
    let json = config_json(
      json!({ "origin": "https://github.com/a/b.git", "auth": { "user": "u", "pass": "p" } }),
    );
    let config = DoupBotConfigFile::from_json(&json).unwrap();
    assert_eq!(
      only_repo(config.clone()).auth,
      Some(DoupBotRepoAuth::Https {
        user: "u".into(),
        pass: "p".into()
      })
    );
    assert_eq!(
      DoupBotConfigFile::from_yaml(&config.to_yaml().unwrap()).unwrap(),
      config
    );
  }

  #[test]
  fn inline_ssh_key_round_trips() {
    let json = config_json(json!({ "origin": "git@github.com:a/b.git", "auth": { "key": KEY } }));
    let config = DoupBotConfigFile::from_json(&json).unwrap();
    assert_eq!(
      only_repo(config.clone()).auth,
      Some(DoupBotRepoAuth::Ssh {
        key: Some(KEY.into()),
        key_path: None
      })
    );
    assert_eq!(
      DoupBotConfigFile::from_json(&config.to_json().unwrap()).unwrap(),
      config
    );
  }

  #[test]
  fn ssh_key_path_is_read_and_round_trips_as_path() {
    let dir = TempDir::new().unwrap();
    let file = dir.child("id_ed25519");
    file.write_str(KEY).unwrap();
    let path = file.path().to_string_lossy().into_owned();

    let json =
      config_json(json!({ "origin": "git@github.com:a/b.git", "auth": { "key_path": path } }));
    let config = DoupBotConfigFile::from_json(&json).unwrap();
    assert_eq!(
      only_repo(config.clone()).auth,
      Some(DoupBotRepoAuth::Ssh {
        key: Some(KEY.into()),
        key_path: Some(file.path().to_path_buf())
      })
    );

    let out: serde_json::Value = serde_json::from_str(&config.to_json().unwrap()).unwrap();
    assert_eq!(out["repos"][0]["auth"], json!({ "key_path": path }));
  }

  #[test]
  fn ssh_key_file_with_invalid_contents_is_rejected() {
    let dir = TempDir::new().unwrap();
    let file = dir.child("id_ed25519");
    file.write_str("not a key").unwrap();

    let json = config_json(
      json!({ "origin": "git@github.com:a/b.git", "auth": { "key_path": file.path() } }),
    );
    assert!(DoupBotConfigFile::from_json(&json).is_err());
  }

  #[test]
  fn missing_ssh_key_file_is_rejected() {
    let json = config_json(
      json!({ "origin": "git@github.com:a/b.git", "auth": { "key_path": "/does/not/exist" } }),
    );
    assert!(DoupBotConfigFile::from_json(&json).is_err());
  }

  #[test]
  fn encrypted_ssh_key_is_rejected() {
    let json =
      config_json(json!({ "origin": "git@github.com:a/b.git", "auth": { "key": ENCRYPTED_KEY } }));
    assert!(DoupBotConfigFile::from_json(&json).is_err());
  }

  #[test]
  fn parses_the_documented_yaml() {
    let yaml = format!(
      "
$schema: ./path/to/bot.schema.json
repos:
  - origin: https://forge-provider.com/path/to/repo.git
  - origin: https://forge-provider.com/path/to/repo.git
    auth:
      user: foo
      pass: bar
  - origin: git@forge-provider.com:path/to/repo.git
    auth:
      key: |
{}
",
      KEY
        .lines()
        .map(|l| format!("        {l}"))
        .collect::<Vec<_>>()
        .join("\n")
    );
    let config = DoupBotConfigFile::from_yaml(&yaml).unwrap();
    assert_eq!(config.repos.len(), 3);
    assert_eq!(config.repos[0].auth, None);
    assert_eq!(
      config.repos[2].auth,
      Some(DoupBotRepoAuth::Ssh {
        key: Some(KEY.into()),
        key_path: None
      })
    );
    assert_eq!(
      DoupBotConfigFile::from_yaml(&config.to_yaml().unwrap()).unwrap(),
      config
    );
  }

  #[test]
  fn git_backend_defaults_to_git_cli_and_round_trips() {
    let json = config_json(json!({ "origin": "https://github.com/a/b.git" }));
    let config = DoupBotConfigFile::from_json(&json).unwrap();
    assert_eq!(config.git_backend, GitBackendKind::GitCli);
    assert!(!config.to_json().unwrap().contains("git_backend"));

    let json =
      json!({ "git_backend": "grit", "repos": [{ "origin": "https://github.com/a/b.git" }] });
    let config = DoupBotConfigFile::from_json(&json.to_string()).unwrap();
    assert_eq!(config.git_backend, GitBackendKind::Grit);
    assert_eq!(
      DoupBotConfigFile::from_json(&config.to_json().unwrap()).unwrap(),
      config
    );
  }

  #[test]
  fn schema_rejects_unknown_git_backend() {
    let repo = json!({ "origin": "https://github.com/a/b.git" });
    assert!(
      DoupBotConfigFileSchema::validate(&json!({ "git_backend": "libgit2", "repos": [repo] }))
        .is_err()
    );
  }

  #[test]
  fn schema_requires_at_least_one_repo() {
    assert!(DoupBotConfigFileSchema::validate(&json!({})).is_err());
    assert!(DoupBotConfigFileSchema::validate(&json!({ "repos": [] })).is_err());
  }

  #[test]
  fn schema_rejects_mismatched_auth() {
    for repo in [
      json!({ "origin": "git@github.com:a/b.git", "auth": { "user": "u", "pass": "p" } }),
      json!({ "origin": "https://github.com/a/b.git", "auth": { "key": KEY } }),
      json!({ "origin": "https://github.com/a/b.git", "auth": { "key_path": "/k" } }),
      json!({ "origin": "https://github.com/a/b.git", "auth": { "user": "u" } }),
      json!({ "origin": "git@github.com:a/b.git", "auth": { "key": KEY, "key_path": "/k" } }),
      json!({ "origin": "git@github.com:a/b.git", "auth": {} }),
      json!({ "origin": "git@github.com:a/b.git", "auth": { "key": "/k" } }),
      json!({ "origin": "git@github.com:a/b.git", "auth": { "key_path": "" } }),
    ] {
      assert!(
        DoupBotConfigFileSchema::validate(&json!({ "repos": [repo.clone()] })).is_err(),
        "{repo}"
      );
    }
  }

  #[test]
  fn schema_key_only_accepts_openssh_key_text() {
    let valid = |key: &str| {
      DoupBotConfigFileSchema::validate(
        &json!({ "repos": [{ "origin": "git@github.com:a/b.git", "auth": { "key": key } }] }),
      )
      .is_ok()
    };

    assert!(valid(KEY));
    assert!(!valid("/home/me/.ssh/id_ed25519"));
    assert!(!valid("not a key"));
    assert!(!valid(
      "-----BEGIN RSA PRIVATE KEY-----\nabc\n-----END RSA PRIVATE KEY-----"
    ));
  }

  /// Writes a config file with one repo, then loads it with the given env vars applied.
  fn load_with(vars: &[(&str, &str)]) -> (TempDir, Loaded<DoupBotConfigFile>) {
    let dir = TempDir::new().unwrap();
    dir
      .child("doupbot.json")
      .write_str(&config_json(
        json!({ "origin": "https://github.com/a/b.git" }),
      ))
      .unwrap();
    let env = vars_to_value(
      vars.iter().map(|(k, v)| (k.to_string(), v.to_string())),
      DoupBotConfigFile::ENV_PREFIX,
    );
    let loaded = load_with_env::<DoupBotConfigFile>(Some(dir.path()), env).unwrap();
    (dir, loaded)
  }

  #[test]
  fn env_overrides_file_values() {
    let (_dir, loaded) = load_with(&[("DOUPBOT_REPOS_0_ORIGIN", "https://github.com/c/d.git")]);
    assert_eq!(loaded.merged.repos[0].origin, "https://github.com/c/d.git");
    assert_eq!(
      loaded.file.unwrap().repos[0].origin,
      "https://github.com/a/b.git"
    );
  }

  #[test]
  fn env_can_add_array_items() {
    let (_dir, loaded) = load_with(&[("DOUPBOT_REPOS_1_ORIGIN", "https://github.com/c/d.git")]);
    assert_eq!(loaded.merged.repos.len(), 2);
    assert_eq!(loaded.file.unwrap().repos.len(), 1);
  }

  #[test]
  fn env_can_supply_partial_overrides() {
    let (_dir, loaded) = load_with(&[
      ("DOUPBOT_REPOS_0_AUTH_USER", "u"),
      ("DOUPBOT_REPOS_0_AUTH_PASS", "p"),
    ]);
    assert_eq!(
      loaded.merged.repos[0].auth,
      Some(DoupBotRepoAuth::Https {
        user: "u".into(),
        pass: "p".into()
      })
    );
    assert_eq!(loaded.file.unwrap().repos[0].auth, None);
  }

  #[test]
  fn env_can_supply_key_path_via_double_underscore() {
    let dir = TempDir::new().unwrap();
    let key_file = dir.child("id_ed25519");
    key_file.write_str(KEY).unwrap();
    dir
      .child("doupbot.json")
      .write_str(&config_json(json!({ "origin": "git@github.com:a/b.git" })))
      .unwrap();
    let env = vars_to_value(
      [(
        "DOUPBOT_REPOS__0__AUTH__KEY_PATH".to_string(),
        key_file.path().to_string_lossy().into_owned(),
      )],
      DoupBotConfigFile::ENV_PREFIX,
    );

    let loaded = load_with_env::<DoupBotConfigFile>(Some(dir.path()), env).unwrap();
    assert_eq!(
      loaded.merged.repos[0].auth,
      Some(DoupBotRepoAuth::Ssh {
        key: Some(KEY.into()),
        key_path: Some(key_file.path().to_path_buf())
      })
    );
    assert_eq!(loaded.file.unwrap().repos[0].auth, None);
  }

  #[test]
  fn invalid_env_override_is_rejected() {
    let dir = TempDir::new().unwrap();
    dir
      .child("doupbot.json")
      .write_str(&config_json(
        json!({ "origin": "https://github.com/a/b.git" }),
      ))
      .unwrap();
    let env = vars_to_value(
      [("DOUPBOT_REPOS_0_AUTH_KEY".to_string(), "/k".to_string())],
      DoupBotConfigFile::ENV_PREFIX,
    );
    assert!(load_with_env::<DoupBotConfigFile>(Some(dir.path()), env).is_err());
  }

  #[test]
  fn env_overrides_are_not_saved_to_the_file() {
    let (dir, loaded) = load_with(&[("DOUPBOT_REPOS_0_ORIGIN", "https://github.com/c/d.git")]);
    assert!(loaded.merged.save().is_err());

    loaded.file.unwrap().save().unwrap();
    let saved = std::fs::read_to_string(dir.child("doupbot.json").path()).unwrap();
    assert!(saved.contains("https://github.com/a/b.git"));
    assert!(!saved.contains("c/d.git"));
  }

  #[test]
  fn loads_from_env_alone_when_no_file_exists() {
    let dir = TempDir::new().unwrap();
    let env = vars_to_value(
      [(
        "DOUPBOT_REPOS_0_ORIGIN".to_string(),
        "https://github.com/c/d.git".to_string(),
      )],
      DoupBotConfigFile::ENV_PREFIX,
    );
    let loaded = load_with_env::<DoupBotConfigFile>(Some(dir.path()), env).unwrap();
    assert!(loaded.file.is_none());
    assert_eq!(loaded.merged.repos[0].origin, "https://github.com/c/d.git");
  }
}
