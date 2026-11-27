//! Reads what cargo needs to know about a workspace to resolve its lockfile: its local packages
//! (their dependencies & features), `[patch]`es and the rust version that versions are preferred for.
//!
//! This module only uses plain `std` / `serde` types so that it can be unit tested natively.

use crate::{discover, index, registry};
use semver::Version;
use std::{
  collections::{BTreeMap, HashMap, HashSet, VecDeque},
  fs,
  path::{Component, Path, PathBuf},
};
use toml::{Table, Value};

const KINDS: [&str; 5] =
  ["dependencies", "dev-dependencies", "dev_dependencies", "build-dependencies", "build_dependencies"];

#[derive(Debug, Default)]
pub struct Workspace {
  /// The packages read from manifests: the members, and the path dependencies they reach.
  pub packages: Vec<LocalPackage>,
  /// The `(lockfile source, crate name)` pairs replaced by `[patch]`.
  pub patches: HashSet<(String, String)>,
  /// The rust version to prefer dependency versions for, when `incompatible-rust-versions` is `fallback`.
  pub msrv: Option<Version>,
}

impl Workspace {
  pub fn package(&self, name: &str) -> Option<&LocalPackage> {
    self.packages.iter().find(|p| p.name == name)
  }

  pub fn is_patched(&self, source: &str, name: &str) -> bool {
    self.patches.iter().any(|(s, n)| n == name && index::same_source(s, source))
  }
}

/// A package whose manifest is in the repository.
#[derive(Debug)]
pub struct LocalPackage {
  pub name: String,
  pub version: Version,
  /// Members, the packages within the workspace's directory, resolve with every feature and
  /// their dev-dependencies. Other path dependencies resolve like any other dependency.
  pub member: bool,
  pub dependencies: Vec<LocalDependency>,
  pub features: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalDependency {
  /// The name the dependency is known by within the package.
  pub name: String,
  pub real_name: String,
  pub source: LocalSource,
  /// The version requirement, `None` meaning any version.
  pub req: Option<String>,
  pub features: Vec<String>,
  pub default_features: bool,
  pub optional: bool,
  pub dev: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LocalSource {
  /// The lockfile source of a registry.
  Registry(String),
  /// The directory of the dependency, relative to the repository. `None` when it is outside it.
  Path(Option<PathBuf>),
  Git(String),
}

/// Loads the workspace whose `Cargo.lock` is in `lock_dir` (relative to `root`).
///
/// `locked` holds the name & version of each package without a source in the lockfile. Manifests
/// of those packages are read first, so that unrelated manifests (eg: test fixtures) never get in
/// the way, then every path dependency they reach.
pub fn load(root: &Path, lock_dir: &Path, locked: &HashMap<String, Version>) -> Result<Workspace, String> {
  let root_manifest = read_table(&root.join(lock_dir).join("Cargo.toml"))?;
  if root_manifest.get("replace").and_then(Value::as_table).is_some_and(|t| !t.is_empty()) {
    return Err("[replace] is not supported, use [patch] instead".to_string());
  }

  // Seed with the manifests of locked packages, preferring the one with the locked version should
  // several share a name.
  let (manifests, locks) = discover::find_files(&root.join(lock_dir))?;
  let mut seeds: BTreeMap<String, (PathBuf, bool)> = BTreeMap::new();
  for dir in manifests {
    // A manifest beneath another lockfile belongs to a nested workspace.
    if locks.iter().any(|lock| !lock.as_os_str().is_empty() && dir.starts_with(lock)) {
      continue;
    }
    let manifest_dir = lock_dir.join(&dir);
    let table = read_table(&root.join(&manifest_dir).join("Cargo.toml"))?;
    let Some(package) = table.get("package").and_then(Value::as_table) else { continue };
    let Some(name) = package.get("name").and_then(Value::as_str) else { continue };
    let Some(locked_version) = locked.get(name) else { continue };
    let version = Context::find(root, &manifest_dir).version(package);
    let exact = version.as_ref() == Some(locked_version);
    if seeds.get(name).is_none_or(|(_, seen_exact)| exact && !seen_exact) {
      seeds.insert(name.to_string(), (manifest_dir, exact));
    }
  }

  let mut packages: Vec<LocalPackage> = Vec::new();
  let mut rust_versions = Vec::new();
  let mut queue: VecDeque<PathBuf> = seeds.into_values().map(|(dir, _)| dir).collect();
  let mut seen = HashSet::new();
  while let Some(dir) = queue.pop_front() {
    if !seen.insert(dir.clone()) {
      continue;
    }
    let Ok(table) = read_table(&root.join(&dir).join("Cargo.toml")) else { continue };
    let Some(package) = table.get("package").and_then(Value::as_table) else { continue };
    let Some(name) = package.get("name").and_then(Value::as_str) else { continue };
    if packages.iter().any(|p| p.name == name) {
      continue;
    }

    let context = Context::find(root, &dir);
    let member = dir.starts_with(lock_dir);
    let local = context.package(name, &dir, &table, member).map_err(|err| format!("{}: {err}", dir.display()))?;
    if member
      && let Some(rust_version) = context.inherited(package, "rust-version").and_then(|v| parse_rust_version(&v))
    {
      rust_versions.push(rust_version);
    }
    for dependency in &local.dependencies {
      if let LocalSource::Path(Some(path)) = &dependency.source {
        queue.push_back(path.clone());
      }
    }
    packages.push(local);
  }

  Ok(Workspace {
    packages,
    patches: patches(root, lock_dir, &root_manifest),
    msrv: msrv(root, lock_dir, &root_manifest, &Context::find(root, lock_dir), rust_versions),
  })
}

/// The workspace a package belongs to, which it may inherit fields & dependencies from.
struct Context<'a> {
  root: &'a Path,
  /// The directory of the workspace's root manifest, relative to `root`.
  dir: PathBuf,
  /// `[workspace.package]`
  package: Option<Table>,
  /// `[workspace.dependencies]`
  dependencies: Option<Table>,
}

impl<'a> Context<'a> {
  /// The workspace of the package in `dir`: the closest manifest with a `[workspace]` table.
  fn find(root: &'a Path, dir: &Path) -> Self {
    for ancestor in dir.ancestors() {
      let Ok(manifest) = read_table(&root.join(ancestor).join("Cargo.toml")) else { continue };
      if let Some(workspace) = manifest.get("workspace").and_then(Value::as_table) {
        return Self {
          root,
          dir: ancestor.to_path_buf(),
          package: workspace.get("package").and_then(Value::as_table).cloned(),
          dependencies: workspace.get("dependencies").and_then(Value::as_table).cloned(),
        };
      }
    }
    Self { root, dir: dir.to_path_buf(), package: None, dependencies: None }
  }

  /// A `[package]` string field, following `field.workspace = true`.
  fn inherited(&self, package: &Table, field: &str) -> Option<String> {
    match package.get(field)? {
      Value::String(value) => Some(value.clone()),
      Value::Table(t) if t.get("workspace").and_then(Value::as_bool) == Some(true) => {
        self.package.as_ref()?.get(field)?.as_str().map(str::to_string)
      }
      _ => None,
    }
  }

  /// The package's version, which is `0.0.0` when left out.
  fn version(&self, package: &Table) -> Option<Version> {
    Version::parse(&self.inherited(package, "version").unwrap_or_else(|| "0.0.0".to_string())).ok()
  }

  fn package(&self, name: &str, manifest_dir: &Path, manifest: &Table, member: bool) -> Result<LocalPackage, String> {
    let package = manifest.get("package").and_then(Value::as_table).ok_or("missing [package]")?;
    let version = self.version(package).ok_or("invalid version")?;

    let mut dependencies = Vec::new();
    let mut tables = vec![manifest];
    if let Some(targets) = manifest.get("target").and_then(Value::as_table) {
      tables.extend(targets.values().filter_map(Value::as_table));
    }
    for table in tables {
      for kind in KINDS {
        for (key, spec) in table.get(kind).and_then(Value::as_table).into_iter().flatten() {
          dependencies.push(self.dependency(key, spec, manifest_dir, kind.starts_with("dev"))?);
        }
      }
    }

    let features = manifest
      .get("features")
      .and_then(Value::as_table)
      .into_iter()
      .flatten()
      .map(|(feature, values)| (feature.clone(), strings(values)))
      .collect();
    Ok(LocalPackage { name: name.to_string(), version, member, dependencies, features })
  }

  fn dependency(&self, key: &str, spec: &Value, manifest_dir: &Path, dev: bool) -> Result<LocalDependency, String> {
    let empty = Table::new();
    let local = spec.as_table().unwrap_or(&empty);
    let inherits = local.get("workspace").and_then(Value::as_bool) == Some(true);

    let (spec, base_dir) = if inherits {
      let inherited = self
        .dependencies
        .as_ref()
        .and_then(|d| d.get(key))
        .ok_or_else(|| format!("`{key}` inherits from [workspace.dependencies] but isn't defined there"))?;
      (inherited, self.dir.as_path())
    } else {
      (spec, manifest_dir)
    };
    let shorthand;
    let table = match spec {
      Value::String(req) => {
        shorthand = Table::from_iter([("version".to_string(), Value::String(req.clone()))]);
        &shorthand
      }
      Value::Table(table) => table,
      _ => return Err(format!("dependency `{key}` is neither a version nor a table")),
    };

    let default_features =
      |t: &Table| t.get("default-features").or_else(|| t.get("default_features")).and_then(Value::as_bool);
    let mut features = table.get("features").map(strings).unwrap_or_default();
    if inherits {
      features.extend(local.get("features").map(strings).unwrap_or_default());
    }

    let real_name = table.get("package").and_then(Value::as_str).unwrap_or(key).to_string();
    let source = if let Some(path) = table.get("path").and_then(Value::as_str) {
      LocalSource::Path(normalize(&base_dir.join(path)))
    } else if let Some(url) = table.get("git").and_then(Value::as_str) {
      LocalSource::Git(url.to_string())
    } else if let Some(alias) = table.get("registry").and_then(Value::as_str) {
      let url = registry::resolve_index(self.root, base_dir, alias)?;
      LocalSource::Registry(index::source_for_registry(Some(&url)))
    } else if let Some(url) = table.get("registry-index").and_then(Value::as_str) {
      LocalSource::Registry(if url.starts_with("sparse+") { url.to_string() } else { format!("registry+{url}") })
    } else {
      LocalSource::Registry(index::CRATES_IO_SOURCE.to_string())
    };

    Ok(LocalDependency {
      name: key.to_string(),
      real_name,
      source,
      req: table.get("version").and_then(Value::as_str).map(str::to_string),
      features,
      default_features: default_features(local).or_else(|| default_features(table)).unwrap_or(true),
      optional: local.get("optional").and_then(Value::as_bool).unwrap_or(false),
      dev,
    })
  }
}

/// Resolves `.` & `..` in a path relative to the repository, `None` if it leads outside of it.
fn normalize(path: &Path) -> Option<PathBuf> {
  let mut normalized = PathBuf::new();
  for component in path.components() {
    match component {
      Component::CurDir => {}
      Component::ParentDir => {
        if !normalized.pop() {
          return None;
        }
      }
      Component::Normal(part) => normalized.push(part),
      _ => return None,
    }
  }
  Some(normalized)
}

fn strings(value: &Value) -> Vec<String> {
  value.as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect()
}

fn read_table(path: &Path) -> Result<Table, String> {
  let content = fs::read_to_string(path).map_err(|err| format!("failed to read {}: {err}", path.display()))?;
  toml::from_str(&content).map_err(|err| format!("failed to parse {}: {err}", path.display()))
}

/// The registry crates replaced by the root manifest's `[patch]` tables.
/// Patches of git sources are left out, as git packages are never resolved.
fn patches(root: &Path, lock_dir: &Path, manifest: &Table) -> HashSet<(String, String)> {
  let mut patches = HashSet::new();
  for (key, entries) in manifest.get("patch").and_then(Value::as_table).into_iter().flatten() {
    let source = match key.as_str() {
      "crates-io" | index::CRATES_IO_GIT_INDEX => Some(index::CRATES_IO_SOURCE.to_string()),
      url if url.starts_with("sparse+") => Some(url.to_string()),
      url if url.contains("://") => None,
      alias => registry::resolve_index(root, lock_dir, alias).ok().map(|url| index::source_for_registry(Some(&url))),
    };
    let Some(source) = source else { continue };
    for (name, spec) in entries.as_table().into_iter().flatten() {
      let real_name = spec.get("package").and_then(Value::as_str).unwrap_or(name);
      patches.insert((source.clone(), real_name.to_string()));
    }
  }
  patches
}

/// The rust version that cargo prefers dependency versions for, see
/// <https://doc.rust-lang.org/cargo/reference/config.html#resolverincompatible-rust-versions>.
///
/// Only with the `fallback` policy (the default of resolver 3) are versions filtered. It is then
/// the lowest `rust-version` of the members, or the toolchain's version when none set one.
fn msrv(
  root: &Path,
  lock_dir: &Path,
  manifest: &Table,
  context: &Context,
  rust_versions: Vec<Version>,
) -> Option<Version> {
  let package = manifest.get("package").and_then(Value::as_table);
  let resolver = manifest
    .get("workspace")
    .and_then(|w| w.get("resolver"))
    .or_else(|| package.and_then(|p| p.get("resolver")))
    .and_then(Value::as_str)
    .map(str::to_string)
    .or_else(|| match package.and_then(|p| context.inherited(p, "edition")).as_deref() {
      Some("2024") => Some("3".to_string()),
      _ => None,
    });

  let configured = lock_dir.ancestors().find_map(|dir| {
    ["config.toml", "config"].iter().find_map(|file| {
      let config = read_table(&root.join(dir).join(".cargo").join(file)).ok()?;
      config.get("resolver")?.get("incompatible-rust-versions")?.as_str().map(str::to_string)
    })
  });
  let fallback = match configured.as_deref() {
    Some(policy) => policy == "fallback",
    None => resolver.as_deref() == Some("3"),
  };
  if !fallback {
    return None;
  }
  rust_versions.into_iter().min().or_else(|| toolchain_version(root, lock_dir))
}

/// The version pinned by a `rust-toolchain.toml` / `rust-toolchain` file, if it pins one.
fn toolchain_version(root: &Path, lock_dir: &Path) -> Option<Version> {
  lock_dir.ancestors().find_map(|dir| {
    if let Ok(content) = fs::read_to_string(root.join(dir).join("rust-toolchain.toml")) {
      let table: Table = toml::from_str(&content).ok()?;
      return parse_rust_version(table.get("toolchain")?.get("channel")?.as_str()?);
    }
    let content = fs::read_to_string(root.join(dir).join("rust-toolchain")).ok()?;
    match toml::from_str::<Table>(&content) {
      Ok(table) => parse_rust_version(table.get("toolchain")?.get("channel")?.as_str()?),
      Err(_) => parse_rust_version(content.trim()),
    }
  })
}

/// Parses a rust version that may leave out the minor or patch, eg: `"1.70"`.
pub fn parse_rust_version(version: &str) -> Option<Version> {
  let mut parts = version.trim().split('.').map(str::parse::<u64>);
  let major = parts.next()?.ok()?;
  let minor = parts.next().transpose().ok()?.unwrap_or(0);
  let patch = parts.next().transpose().ok()?.unwrap_or(0);
  parts.next().is_none().then(|| Version::new(major, minor, patch))
}

#[cfg(test)]
mod tests {
  use super::*;
  use tempfile::TempDir;

  fn write(root: &Path, path: &str, content: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
  }

  fn locked(packages: &[(&str, &str)]) -> HashMap<String, Version> {
    packages.iter().map(|(n, v)| (n.to_string(), Version::parse(v).unwrap())).collect()
  }

  #[test]
  fn members_inherit_from_the_workspace() {
    let dir = TempDir::new().unwrap();
    write(
      dir.path(),
      "Cargo.toml",
      r#"
[workspace]
members = ["a"]
resolver = "3"
[workspace.package]
version = "0.2.0"
rust-version = "1.80"
[workspace.dependencies]
serde = { version = "1", default-features = false, features = ["std"] }
tokio = "1.40"
local = { path = "local" }
[patch.crates-io]
tokio = { git = "https://example.com/tokio" }
"#,
    );
    write(
      dir.path(),
      "a/Cargo.toml",
      r#"
[package]
name = "a"
version.workspace = true
rust-version = { workspace = true }
[dependencies]
serde = { workspace = true, features = ["derive"] }
tokio = { workspace = true, optional = true }
local = { workspace = true }
renamed = { package = "real", version = "2", default-features = false }
[target.'cfg(unix)'.dev-dependencies]
from-git = { git = "https://example.com/x" }
[features]
fast = ["tokio/rt"]
"#,
    );
    write(
      dir.path(),
      "fixture/Cargo.toml",
      "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\n[dependencies]\nx = { workspace = true }\n",
    );

    let workspace = load(dir.path(), Path::new(""), &locked(&[("a", "0.2.0")])).unwrap();
    let a = workspace.package("a").unwrap();
    assert!(a.member);
    assert_eq!(a.version, Version::new(0, 2, 0));
    let crates_io = || LocalSource::Registry(index::CRATES_IO_SOURCE.to_string());
    let dep =
      |name: &str, real: &str, source, req: Option<&str>, features: &[&str], default_features| LocalDependency {
        name: name.into(),
        real_name: real.into(),
        source,
        req: req.map(str::to_string),
        features: features.iter().map(|f| f.to_string()).collect(),
        default_features,
        optional: false,
        dev: false,
      };
    assert_eq!(
      a.dependencies,
      [
        dep("local", "local", LocalSource::Path(Some("local".into())), None, &[], true),
        dep("renamed", "real", crates_io(), Some("2"), &[], false),
        dep("serde", "serde", crates_io(), Some("1"), &["std", "derive"], false),
        LocalDependency { optional: true, ..dep("tokio", "tokio", crates_io(), Some("1.40"), &[], true) },
        LocalDependency {
          dev: true,
          ..dep("from-git", "from-git", LocalSource::Git("https://example.com/x".into()), None, &[], true)
        },
      ]
    );
    assert_eq!(a.features["fast"], ["tokio/rt"]);
    assert!(workspace.is_patched(index::CRATES_IO_SOURCE, "tokio"));
    assert_eq!(workspace.msrv, Some(Version::new(1, 80, 0)));
  }

  #[test]
  fn path_dependencies_outside_the_workspace_are_followed() {
    let dir = TempDir::new().unwrap();
    write(
      dir.path(),
      "crates/lib/Cargo.toml",
      "[package]\nname = \"lib\"\nversion.workspace = true\n[dependencies]\nmemchr = { version = \"2\", optional = true }\n",
    );
    write(
      dir.path(),
      "Cargo.toml",
      "[workspace]\nmembers = [\"crates/lib\"]\n[workspace.package]\nversion = \"0.4.22\"\n",
    );
    write(
      dir.path(),
      "fuzz/Cargo.toml",
      "[package]\nname = \"fuzz\"\nversion = \"0.0.1\"\n[dependencies]\nlib = { path = \"../crates/lib\" }\n[workspace]\n",
    );

    // The lockfile is stale, `lib` has moved on to 0.4.22 since it was locked.
    let workspace = load(dir.path(), Path::new("fuzz"), &locked(&[("fuzz", "0.0.1"), ("lib", "0.4.16")])).unwrap();
    assert!(workspace.package("fuzz").unwrap().member);
    let lib = workspace.package("lib").unwrap();
    assert!(!lib.member);
    assert_eq!(lib.version, Version::new(0, 4, 22));
    assert!(lib.dependencies[0].optional);
  }

  #[test]
  fn msrv_falls_back_to_the_toolchain_and_respects_config() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "Cargo.toml", "[package]\nname = \"a\"\nversion = \"0.1.0\"\nedition = \"2024\"\n");
    write(dir.path(), "rust-toolchain.toml", "[toolchain]\nchannel = \"1.100.0\"\n");
    let locked = locked(&[("a", "0.1.0")]);
    assert_eq!(load(dir.path(), Path::new(""), &locked).unwrap().msrv, Some(Version::new(1, 100, 0)));

    write(dir.path(), ".cargo/config.toml", "[resolver]\nincompatible-rust-versions = \"allow\"\n");
    assert_eq!(load(dir.path(), Path::new(""), &locked).unwrap().msrv, None);
  }

  #[test]
  fn replace_is_rejected() {
    let dir = TempDir::new().unwrap();
    write(
      dir.path(),
      "Cargo.toml",
      "[package]\nname = \"a\"\nversion = \"0.1.0\"\n[replace]\n\"foo:1.0.0\" = { path = \"foo\" }\n",
    );
    assert!(load(dir.path(), Path::new(""), &HashMap::new()).unwrap_err().contains("[replace]"));
  }

  #[test]
  fn rust_versions_may_be_partial() {
    assert_eq!(parse_rust_version("1.70"), Some(Version::new(1, 70, 0)));
    assert_eq!(parse_rust_version("1.70.1"), Some(Version::new(1, 70, 1)));
    assert_eq!(parse_rust_version("stable"), None);
  }
}
