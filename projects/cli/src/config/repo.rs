use super::file::ConfigFile;
use anyhow::Result;
use jsonschema::ValidationError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Embed the Json Schema, we will use this for runtime validation.
#[jsonschema::validator(path = "../../schemas/repo.schema.json")]
struct SCHEMA;

/// Configuration found at the root of a git repo that is to be updated by the
/// doup dependency updater bot. This is the normalised internal representation
/// of what is described by the equivalent JSON schema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DoupRepoConfigFile {
  /// Ensure `$schema` round trips if the user has added it to their config.
  /// see: https://json-schema.org/understanding-json-schema/reference/schema
  #[serde(rename = "$schema", default, skip_serializing_if = "Option::is_none")]
  schema_uri: Option<String>,

  /// The platform that doup will operate against, submitting pull requests with
  /// it's dependency updates. Think github, gitlab, bitbucket, etc. ONE MUST BE GIVEN.
  pub platform: DoupRepoPlugin,

  /// The ecosystems of dependencies that doup will work to keep up to date.
  /// Think npm, nuget, go modules, rust crates, etc. AT LEAST ONE MUST BE GIVEN.
  pub ecosystems: Vec<DoupRepoPlugin>,

  /// Where this config was loaded from; `None` if built in memory.
  #[serde(skip)]
  source: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DoupRepoPlugin {
  /// A plugin package reference in wkg format: namespace:package@version.
  #[serde(rename = "use")]
  pub plugin: String,

  /// An optional untyped property bag, this will be passed as a JSON string over the WIT contract.
  /// The plugin is then free to deserialize, validate & do what it wishes with it's own config.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub with: Option<serde_json::Map<String, serde_json::Value>>,
}

impl ConfigFile for DoupRepoConfigFile {
  const DEFAULT_NAMES: &'static [&'static str] = &["doup.json", "doup.yaml", "doup.yml"];

  const ENV_PREFIX: &'static str = "DOUP";

  const VALIDATE: for<'i> fn(&'i Value) -> Result<(), ValidationError<'i>> = SCHEMA::validate;

  fn source(&self) -> Option<&Path> {
    self.source.as_deref()
  }

  fn set_source(&mut self, path: PathBuf) {
    self.source = Some(path)
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::config::env::vars_to_value;
  use crate::config::file::load_with_env;
  use assert_fs::{TempDir, prelude::*};
  use serde_json::json;

  const YAML: &str = "
$schema: ./path/to/repo.schema.json
platform:
  use: doup:github@^1
  with:
    optional: values
ecosystems:
  - use: doup:doup@^0.1.0
  - use: doup:npm@1.2.3
    with:
      optional: values
";

  fn plugin(plugin: &str, with: Option<Value>) -> DoupRepoPlugin {
    DoupRepoPlugin {
      plugin: plugin.into(),
      with: with.map(|v| v.as_object().unwrap().clone()),
    }
  }

  #[test]
  fn parses_use_and_with() {
    let config = DoupRepoConfigFile::from_yaml(YAML).unwrap();
    assert_eq!(
      config.platform,
      plugin("doup:github@^1", Some(json!({"optional": "values"})))
    );
    assert_eq!(
      config.ecosystems,
      vec![
        plugin("doup:doup@^0.1.0", None),
        plugin("doup:npm@1.2.3", Some(json!({"optional": "values"}))),
      ]
    );
  }

  #[test]
  fn round_trips() {
    let config = DoupRepoConfigFile::from_yaml(YAML).unwrap();
    assert_eq!(
      DoupRepoConfigFile::from_yaml(&config.to_yaml().unwrap()).unwrap(),
      config
    );
    assert_eq!(
      DoupRepoConfigFile::from_json(&config.to_json().unwrap()).unwrap(),
      config
    );
  }

  #[test]
  fn schema_rejects_invalid_plugins() {
    for platform in [
      json!("doup:github"),
      json!({ "doup:github": { "a": "b" } }),
      json!({ "with": { "a": "b" } }),
      json!({ "use": "doup:github", "uses": "typo" }),
      json!({ "use": "not a ref" }),
      json!({ "use": "doup:github", "with": "nope" }),
    ] {
      let instance = json!({ "platform": platform, "ecosystems": [{ "use": "doup:doup" }] });
      assert!(SCHEMA::validate(&instance).is_err(), "{instance}");
    }
  }

  /// Writes a config file, then loads it with the given env vars applied.
  fn load_with(
    vars: &[(&str, &str)],
  ) -> (TempDir, crate::config::file::Loaded<DoupRepoConfigFile>) {
    let dir = TempDir::new().unwrap();
    dir
      .child("doup.json")
      .write_str(
        &json!({
          "platform": { "use": "doup:github@^1" },
          "ecosystems": [{ "use": "doup:doup@^0.1.0" }]
        })
        .to_string(),
      )
      .unwrap();
    let env = vars_to_value(
      vars.iter().map(|(k, v)| (k.to_string(), v.to_string())),
      DoupRepoConfigFile::ENV_PREFIX,
    );
    let loaded = load_with_env::<DoupRepoConfigFile>(Some(dir.path()), env).unwrap();
    (dir, loaded)
  }

  #[test]
  fn env_overrides_plugin_use() {
    let (_dir, loaded) = load_with(&[
      ("DOUP_PLATFORM_USE", "doup:gitlab@^2"),
      ("DOUP_ECOSYSTEMS_0_USE", "doup:npm@1.2.3"),
    ]);
    assert_eq!(loaded.merged.platform.plugin, "doup:gitlab@^2");
    assert_eq!(loaded.merged.ecosystems[0].plugin, "doup:npm@1.2.3");
    assert_eq!(loaded.file.unwrap().platform.plugin, "doup:github@^1");
  }

  #[test]
  fn env_supplies_plugin_with() {
    let (_dir, loaded) = load_with(&[
      ("DOUP_PLATFORM_WITH_FOO", "bar"),
      ("DOUP_ECOSYSTEMS__0__WITH__SOME_KEY", "baz"),
    ]);
    assert_eq!(
      loaded.merged.platform.with,
      json!({"foo": "bar"}).as_object().cloned()
    );
    assert_eq!(
      loaded.merged.ecosystems[0].with,
      json!({"some_key": "baz"}).as_object().cloned()
    );
    assert_eq!(loaded.file.unwrap().platform.with, None);
  }

  #[test]
  fn loads_from_env_alone_when_no_file_exists() {
    let dir = TempDir::new().unwrap();
    let env = vars_to_value(
      [
        (
          "DOUP_PLATFORM_USE".to_string(),
          "doup:github@^1".to_string(),
        ),
        (
          "DOUP_ECOSYSTEMS_0_USE".to_string(),
          "doup:npm@1.2.3".to_string(),
        ),
      ],
      DoupRepoConfigFile::ENV_PREFIX,
    );
    let loaded = load_with_env::<DoupRepoConfigFile>(Some(dir.path()), env).unwrap();
    assert!(loaded.file.is_none());
    assert_eq!(loaded.merged.platform.plugin, "doup:github@^1");
  }

  #[test]
  fn no_file_and_incomplete_env_is_rejected() {
    let dir = TempDir::new().unwrap();
    let env = vars_to_value([], DoupRepoConfigFile::ENV_PREFIX);
    assert!(load_with_env::<DoupRepoConfigFile>(Some(dir.path()), env).is_err());
  }
}
