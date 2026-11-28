use super::ConfigError;
use super::file::ConfigFile;
use super::lock::LOCK_FILE_NAME;
use jsonschema::ValidationError;
use semver::VersionReq;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;
use std::path::{Path, PathBuf};

/// Embed the Json Schema, we will use this for runtime validation.
#[jsonschema::validator(path = "../../schemas/repo.schema.json")]
struct Schema;

/// Configuration found at the root of a git repo that is to be updated by the
/// doup dependency updater bot. This is the normalised internal representation
/// of what is described by the equivalent JSON schema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DoupRepoConfigFile {
  /// Ensure `$schema` round trips if the user has added it to their config.
  /// see: <https://json-schema.org/understanding-json-schema/reference/schema>
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

#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct DoupRepoPlugin {
  /// Either a plugin package reference in wkg format (namespace:package@version) or a path to an
  /// explicit wasm module (eg: ./plugin.wasm).
  #[serde(rename = "use")]
  pub plugin: String,

  /// Where a package reference is downloaded from, overriding the registry its namespace maps to.
  /// Either `host/prefix` (an OCI registry, eg: `ghcr.io/brad-jones` → `ghcr.io/brad-jones/<ns>/<pkg>`)
  /// or a bare `host` (a wasm-pkg registry, its metadata is discovered). Only valid with package references.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub registry: Option<String>,

  /// An optional untyped property bag, this will be passed as a JSON string over the WIT contract.
  /// The plugin is then free to deserialize, validate & do what it wishes with it's own config.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub with: Option<serde_json::Map<String, serde_json::Value>>,
}

/// What [`DoupRepoPlugin::plugin`] refers to, see [`DoupRepoPlugin::reference`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginRef<'a> {
  /// A path to a wasm module, relative paths are resolved by the plugin host.
  Module(&'a Path),

  /// A package to download from a registry.
  Package(PackageSpec<'a>),
}

/// A package reference parsed from `namespace:package[@requirement]`, with the plugin's registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageSpec<'a> {
  /// The package without its version, eg: `doup:ecosystem-plugin-cargo`.
  pub name: &'a str,

  /// The versions that may be used. Cargo semantics: `@0.1.0` means `^0.1.0`, no version means any.
  pub requirement: VersionReq,

  /// See [`DoupRepoPlugin::registry`].
  pub registry: Option<&'a str>,
}

impl fmt::Display for PackageSpec<'_> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "{}@{}", self.name, self.requirement)?;
    if let Some(registry) = self.registry {
      write!(f, " (from {registry})")?;
    }
    Ok(())
  }
}

impl DoupRepoPlugin {
  /// Parses [`Self::plugin`]: anything ending in `.wasm` is a module path, anything else a package reference.
  ///
  /// # Errors
  ///
  /// [`ConfigError::InvalidVersionRequirement`] if a package's version is not a valid semver requirement.
  pub fn reference(&self) -> Result<PluginRef<'_>, ConfigError> {
    if self.plugin.ends_with(".wasm") {
      return Ok(PluginRef::Module(Path::new(&self.plugin)));
    }
    let (name, requirement) = match self.plugin.split_once('@') {
      Some((name, requirement)) => (
        name,
        VersionReq::parse(requirement)
          .map_err(|source| ConfigError::InvalidVersionRequirement { plugin: self.plugin.clone(), source })?,
      ),
      None => (self.plugin.as_str(), VersionReq::STAR),
    };
    Ok(PluginRef::Package(PackageSpec { name, requirement, registry: self.registry.as_deref() }))
  }

  /// A short, version independent name for the plugin, derived from [`Self::plugin`]:
  ///
  /// - A wasm module path gives its file name minus the extension, eg: `./a/b.wasm` → `b`.
  /// - A wkg package reference drops the version, eg: `namespace:package@1.2.3` → `namespace:package`.
  pub fn name(&self) -> &str {
    match self.plugin.strip_suffix(".wasm") {
      // Split on both separators so Windows style paths work regardless of the host OS.
      Some(path) => path.rsplit(['/', '\\']).next().unwrap_or(path),
      None => self.plugin.split_once('@').map_or(self.plugin.as_str(), |(package, _version)| package),
    }
  }
}

impl DoupRepoConfigFile {
  /// The env prefix scoping overrides to the bot's repo at `index`, eg: `DOUP_REPO_0`.
  pub fn repo_env_prefix(index: usize) -> String {
    format!("{}_{index}", Self::ENV_PREFIX)
  }

  /// Where this config's [`DoupLockFile`](crate::lock::DoupLockFile) lives: next to the config file.
  /// `None` when the config was not loaded from a file.
  pub fn lock_path(&self) -> Option<PathBuf> {
    Some(self.source.as_deref()?.parent()?.join(LOCK_FILE_NAME))
  }

  /// The platform followed by every ecosystem, in config order.
  pub fn plugins(&self) -> impl Iterator<Item = &DoupRepoPlugin> {
    std::iter::once(&self.platform).chain(&self.ecosystems)
  }
}

impl ConfigFile for DoupRepoConfigFile {
  const DEFAULT_NAMES: &'static [&'static str] = &["doup.json", "doup.yaml", "doup.yml"];

  const ENV_PREFIX: &'static str = "DOUP_REPO";

  const STOP_AT_GIT_ROOT: bool = true;

  const VALIDATE: for<'i> fn(&'i Value) -> Result<(), ValidationError<'i>> = Schema::validate;

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
  use crate::env::{layered_vars_to_value, vars_to_value};
  use crate::file::load_with_env;
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
    DoupRepoPlugin { plugin: plugin.into(), registry: None, with: with.map(|v| v.as_object().unwrap().clone()) }
  }

  #[test]
  fn reference_parses_modules_and_packages() {
    let req = |r: &str| VersionReq::parse(r).unwrap();
    let mut registered = plugin("doup:cargo@=1.2.3", None);
    registered.registry = Some("ghcr.io/acme".into());
    for (plugin, expected) in [
      (plugin("./a/b.wasm", None), PluginRef::Module(Path::new("./a/b.wasm"))),
      (plugin("C:\\a\\b.wasm", None), PluginRef::Module(Path::new("C:\\a\\b.wasm"))),
      (
        plugin("doup:cargo@0.1.0", None),
        PluginRef::Package(PackageSpec { name: "doup:cargo", requirement: req("^0.1.0"), registry: None }),
      ),
      (
        plugin("doup:cargo", None),
        PluginRef::Package(PackageSpec { name: "doup:cargo", requirement: VersionReq::STAR, registry: None }),
      ),
      (
        registered.clone(),
        PluginRef::Package(PackageSpec {
          name: "doup:cargo",
          requirement: req("=1.2.3"),
          registry: Some("ghcr.io/acme"),
        }),
      ),
    ] {
      assert_eq!(plugin.reference().unwrap(), expected, "{}", plugin.plugin);
    }
  }

  #[test]
  fn reference_rejects_invalid_requirements() {
    let err = plugin("doup:cargo@not-semver", None).reference().unwrap_err();
    assert!(
      matches!(err, ConfigError::InvalidVersionRequirement { ref plugin, .. } if plugin == "doup:cargo@not-semver")
    );
  }

  #[test]
  fn schema_accepts_registries_for_packages_only() {
    for registry in ["ghcr.io", "ghcr.io/brad-jones", "localhost:5000/a/b"] {
      let package = json!({ "use": "doup:github@1", "registry": registry });
      let instance = json!({ "platform": package, "ecosystems": [package] });
      assert!(Schema::validate(&instance).is_ok(), "{instance}");
    }
    for platform in [
      json!({ "use": "./a.wasm", "registry": "ghcr.io" }),
      json!({ "use": "doup:github", "registry": "https://ghcr.io" }),
      json!({ "use": "doup:github", "registry": "" }),
    ] {
      let instance = json!({ "platform": platform, "ecosystems": [{ "use": "doup:doup" }] });
      assert!(Schema::validate(&instance).is_err(), "{instance}");
    }
  }

  #[test]
  fn lock_path_is_next_to_the_config_file() {
    let dir = TempDir::new().unwrap();
    let file = dir.child("doup.yaml");
    file.write_str("platform: { use: doup:github }\necosystems: [{ use: doup:cargo }]\n").unwrap();
    let config = DoupRepoConfigFile::from_file(file.path()).unwrap();
    assert_eq!(config.lock_path(), Some(dir.path().join("doup.lock")));
    assert_eq!(DoupRepoConfigFile::from_yaml(YAML).unwrap().lock_path(), None);
  }

  #[test]
  fn parses_use_and_with() {
    let config = DoupRepoConfigFile::from_yaml(YAML).unwrap();
    assert_eq!(config.platform, plugin("doup:github@^1", Some(json!({"optional": "values"}))));
    assert_eq!(
      config.ecosystems,
      vec![plugin("doup:doup@^0.1.0", None), plugin("doup:npm@1.2.3", Some(json!({"optional": "values"}))),]
    );
  }

  #[test]
  fn round_trips() {
    let config = DoupRepoConfigFile::from_yaml(YAML).unwrap();
    assert_eq!(DoupRepoConfigFile::from_yaml(&config.to_yaml().unwrap()).unwrap(), config);
    assert_eq!(DoupRepoConfigFile::from_json(&config.to_json().unwrap()).unwrap(), config);
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
      assert!(Schema::validate(&instance).is_err(), "{instance}");
    }
  }

  #[test]
  fn schema_requires_at_least_one_ecosystem() {
    let instance = json!({ "platform": { "use": "doup:github" }, "ecosystems": [] });
    assert!(Schema::validate(&instance).is_err());
  }

  #[test]
  fn schema_rejects_unknown_keys_outside_with() {
    let plugin = json!({ "use": "doup:github", "with": { "anything": "goes" } });
    assert!(Schema::validate(&json!({ "$schema": "./s.json", "platform": plugin, "ecosystems": [plugin] })).is_ok());
    assert!(Schema::validate(&json!({ "log": "x", "platform": plugin, "ecosystems": [plugin] })).is_err());
  }

  #[test]
  fn schema_accepts_wasm_paths() {
    for plugin in ["./a.wasm", "../b/c.wasm", "/abs/d.wasm", "target/e.wasm"] {
      let instance = json!({ "platform": { "use": plugin }, "ecosystems": [{ "use": plugin }] });
      assert!(Schema::validate(&instance).is_ok(), "{instance}");
    }
  }

  #[test]
  fn name_is_the_wasm_file_stem_or_the_unversioned_package() {
    for (plugin_ref, name) in [
      ("./target/wasm32-wasip3/release/doup_ecosystem_plugin_cargo.wasm", "doup_ecosystem_plugin_cargo"),
      ("C:\\plugins\\npm.wasm", "npm"),
      ("cargo.wasm", "cargo"),
      ("doup:cargo@^1.2.3", "doup:cargo"),
      ("doup:cargo", "doup:cargo"),
    ] {
      assert_eq!(plugin(plugin_ref, None).name(), name, "{plugin_ref}");
    }
  }

  /// Writes a config file, then loads it with the given env vars applied under an optional scope.
  fn load_with(vars: &[(&str, &str)], scope: Option<&str>) -> (TempDir, crate::file::Loaded<DoupRepoConfigFile>) {
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
    let env = layered_vars_to_value(
      vars.iter().map(|(k, v)| (k.to_string(), v.to_string())),
      DoupRepoConfigFile::ENV_PREFIX,
      scope,
    )
    .unwrap();
    let loaded = load_with_env::<DoupRepoConfigFile>(Some(dir.path()), env).unwrap();
    (dir, loaded)
  }

  #[test]
  fn repo_env_prefix_includes_the_index() {
    assert_eq!(DoupRepoConfigFile::repo_env_prefix(3), "DOUP_REPO_3");
  }

  #[test]
  fn locate_file_stops_at_the_git_root() {
    let dir = TempDir::new().unwrap();
    dir.child("doup.json").touch().unwrap();
    let repo = dir.child("repo");
    repo.child(".git").create_dir_all().unwrap();
    let sub = repo.child("sub");
    sub.create_dir_all().unwrap();
    assert_eq!(DoupRepoConfigFile::locate_file(Some(sub.path())).unwrap(), None);

    let file = repo.child("doup.yaml");
    file.touch().unwrap();
    assert_eq!(DoupRepoConfigFile::locate_file(Some(sub.path())).unwrap(), Some(file.path().to_path_buf()));
  }

  #[test]
  fn repo_scoped_env_overrides_global_env() {
    let scope = DoupRepoConfigFile::repo_env_prefix(0);
    let (_dir, loaded) = load_with(
      &[
        ("DOUP_REPO_PLATFORM__USE", "doup:gitlab@^2"),
        ("DOUP_REPO_0_PLATFORM__USE", "doup:bitbucket@^3"),
        ("DOUP_REPO_0_ECOSYSTEMS__0__WITH__SOME_KEY", "baz"),
      ],
      Some(&scope),
    );
    assert_eq!(loaded.merged.platform.plugin, "doup:bitbucket@^3");
    assert_eq!(loaded.merged.ecosystems[0].with, json!({"some_key": "baz"}).as_object().cloned());
  }

  #[test]
  fn other_repos_scoped_env_does_not_apply() {
    let scope = DoupRepoConfigFile::repo_env_prefix(1);
    let (_dir, loaded) = load_with(
      &[("DOUP_REPO_PLATFORM__USE", "doup:gitlab@^2"), ("DOUP_REPO_0_PLATFORM__USE", "doup:bitbucket@^3")],
      Some(&scope),
    );
    assert_eq!(loaded.merged.platform.plugin, "doup:gitlab@^2");
  }

  #[test]
  fn other_repos_scoped_env_alone_leaves_the_file_intact() {
    let scope = DoupRepoConfigFile::repo_env_prefix(1);
    let (_dir, loaded) = load_with(&[("DOUP_REPO_0_PLATFORM__USE", "doup:bitbucket@^3")], Some(&scope));
    let file = loaded.file.unwrap();
    assert_eq!(loaded.merged.platform, file.platform);
    assert_eq!(loaded.merged.ecosystems, file.ecosystems);
  }

  #[test]
  fn loads_from_scoped_env_alone_when_no_file_exists() {
    let dir = TempDir::new().unwrap();
    let env = layered_vars_to_value(
      [
        ("DOUP_REPO_0_PLATFORM__USE".to_string(), "doup:github@^1".to_string()),
        ("DOUP_REPO_0_ECOSYSTEMS__0__USE".to_string(), "doup:npm@1.2.3".to_string()),
      ],
      DoupRepoConfigFile::ENV_PREFIX,
      Some(&DoupRepoConfigFile::repo_env_prefix(0)),
    )
    .unwrap();
    let loaded = load_with_env::<DoupRepoConfigFile>(Some(dir.path()), env).unwrap();
    assert!(loaded.file.is_none());
    assert_eq!(loaded.merged.platform.plugin, "doup:github@^1");
    assert_eq!(loaded.merged.ecosystems[0].plugin, "doup:npm@1.2.3");
  }

  #[test]
  fn env_overrides_plugin_use() {
    let (_dir, loaded) = load_with(
      &[("DOUP_REPO_PLATFORM__USE", "doup:gitlab@^2"), ("DOUP_REPO_ECOSYSTEMS__0__USE", "doup:npm@1.2.3")],
      None,
    );
    assert_eq!(loaded.merged.platform.plugin, "doup:gitlab@^2");
    assert_eq!(loaded.merged.ecosystems[0].plugin, "doup:npm@1.2.3");
    assert_eq!(loaded.file.unwrap().platform.plugin, "doup:github@^1");
  }

  #[test]
  fn env_supplies_plugin_with() {
    let (_dir, loaded) =
      load_with(&[("DOUP_REPO_PLATFORM__WITH__FOO", "bar"), ("DOUP_REPO_ECOSYSTEMS__0__WITH__SOME_KEY", "baz")], None);
    assert_eq!(loaded.merged.platform.with, json!({"foo": "bar"}).as_object().cloned());
    assert_eq!(loaded.merged.ecosystems[0].with, json!({"some_key": "baz"}).as_object().cloned());
    assert_eq!(loaded.file.unwrap().platform.with, None);
  }

  #[test]
  fn loads_from_env_alone_when_no_file_exists() {
    let dir = TempDir::new().unwrap();
    let env = vars_to_value(
      [
        ("DOUP_REPO_PLATFORM__USE".to_string(), "doup:github@^1".to_string()),
        ("DOUP_REPO_ECOSYSTEMS__0__USE".to_string(), "doup:npm@1.2.3".to_string()),
      ],
      DoupRepoConfigFile::ENV_PREFIX,
    )
    .unwrap();
    let loaded = load_with_env::<DoupRepoConfigFile>(Some(dir.path()), env).unwrap();
    assert!(loaded.file.is_none());
    assert_eq!(loaded.merged.platform.plugin, "doup:github@^1");
  }

  #[test]
  fn no_file_and_incomplete_env_is_rejected() {
    let dir = TempDir::new().unwrap();
    let env = vars_to_value([], DoupRepoConfigFile::ENV_PREFIX).unwrap();
    let err = load_with_env::<DoupRepoConfigFile>(Some(dir.path()), env).unwrap_err();
    assert!(matches!(err, crate::ConfigError::Schema(_)), "{err:?}");
  }

  #[test]
  fn invalid_file_errors_name_the_file() {
    let dir = TempDir::new().unwrap();
    for (name, contents) in [("doup.yaml", "platform: [unclosed"), ("doup.json", r#"{ "platform": {} }"#)] {
      let file = dir.child(name);
      file.write_str(contents).unwrap();
      let err = DoupRepoConfigFile::from_file(file.path()).unwrap_err();
      let crate::ConfigError::File { path, source } = &err else { panic!("{err:?}") };
      assert_eq!(path, file.path());
      assert!(
        matches!(
          (name, &**source),
          ("doup.yaml", crate::ConfigError::ParseYaml(_)) | ("doup.json", crate::ConfigError::Schema(_))
        ),
        "{err:?}"
      );
    }
  }

  #[test]
  fn unsupported_extension_is_rejected() {
    let dir = TempDir::new().unwrap();
    let file = dir.child("doup.toml");
    file.write_str("").unwrap();
    let err = DoupRepoConfigFile::from_file(file.path()).unwrap_err();
    assert!(matches!(err, crate::ConfigError::UnsupportedExtension { ref path } if path == file.path()), "{err:?}");
  }
}
