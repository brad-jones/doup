//! Finds and parses `Cargo.toml` / `Cargo.lock` files to build the list of dependencies.
//!
//! This module only uses plain `std` / `serde` types (plus the WIT records) so that it can be
//! unit tested natively, without a wasm runtime.

use crate::{Dependency, Occurrence, error::CargoError, lookup, registry};
use ignore::WalkBuilder;
use semver::{Version, VersionReq};
use serde::Deserialize;
use std::{
  collections::BTreeMap,
  fs,
  path::{Path, PathBuf},
};

/// Discovers every registry dependency declared by the manifests found beneath `root`.
///
/// `.git` is always skipped and any `.gitignore` files are honoured.
pub fn discover(root: &Path) -> Result<Vec<Dependency>, CargoError> {
  let (manifests, lockfiles) = find_files(root)?;

  let mut locks = BTreeMap::new();
  for lock_dir in lockfiles {
    locks.insert(lock_dir.clone(), parse_lock(&root.join(&lock_dir).join("Cargo.lock"))?);
  }

  let mut by_key: BTreeMap<(String, Option<String>), Vec<Occurrence>> = BTreeMap::new();
  for manifest_dir in manifests {
    let manifest_file = manifest_dir.join("Cargo.toml");
    let declared = parse_manifest(&root.join(&manifest_file))?;
    let lock = nearest_lock(&manifest_dir, &locks);

    for dep in declared {
      let registry = match &dep.registry {
        Some(alias) => {
          Some(registry::resolve_index(root, &manifest_dir, alias).map_err(|err| CargoError::Declaration {
            manifest: manifest_file.clone(),
            dependency: dep.name.clone(),
            source: Box::new(err),
          })?)
        }
        None => None,
      };
      let occurrence = Occurrence {
        manifest_path: to_slash(&manifest_file),
        lock_path: lock.map(|(dir, _)| to_slash(&dir.join("Cargo.lock"))),
        locked_version: lock
          .and_then(|(_, packages)| locked_version(packages, &dep.name, dep.registry.is_some(), &dep.version)),
        current_version: dep.version,
      };
      let occurrences = by_key.entry((dep.name, registry)).or_default();
      if !occurrences.iter().any(|o| same_declaration(o, &occurrence)) {
        occurrences.push(occurrence);
      }
    }
  }

  by_key
    .into_iter()
    .map(|((name, registry), occurrences)| {
      let current_version = current_version(&name, &occurrences)?;
      Ok(Dependency { name, registry, current_version, occurrences })
    })
    .collect()
}

/// The canonical current version of a dependency: its lowest installed version, as a bare version number.
fn current_version(name: &str, occurrences: &[Occurrence]) -> Result<String, CargoError> {
  let requirements = lookup::requirements(name, occurrences)?;
  lookup::installed(occurrences, &requirements)?
    .map(|version| version.to_string())
    .ok_or_else(|| CargoError::NoOccurrences { name: name.to_string() })
}

fn same_declaration(a: &Occurrence, b: &Occurrence) -> bool {
  a.manifest_path == b.manifest_path && a.current_version == b.current_version
}

/// Returns the (sorted) directories, relative to `root`, that contain a `Cargo.toml`
/// and the ones that contain a `Cargo.lock`.
fn find_files(root: &Path) -> Result<(Vec<PathBuf>, Vec<PathBuf>), CargoError> {
  let walker = WalkBuilder::new(root)
    .hidden(false)
    .parents(false)
    .ignore(false)
    .git_global(false)
    .git_exclude(false)
    .require_git(false)
    .filter_entry(|entry| entry.file_name() != ".git")
    .build();

  let (mut manifests, mut lockfiles) = (Vec::new(), Vec::new());
  for entry in walker {
    let entry = entry.map_err(|source| CargoError::Walk { path: root.to_path_buf(), source })?;
    if !entry.file_type().is_some_and(|t| t.is_file()) {
      continue;
    }
    let target = match entry.file_name().to_str() {
      Some("Cargo.toml") => &mut manifests,
      Some("Cargo.lock") => &mut lockfiles,
      _ => continue,
    };
    let relative = entry.path().strip_prefix(root).map_err(|source| CargoError::OutsideRoot {
      path: entry.path().to_path_buf(),
      root: root.to_path_buf(),
      source,
    })?;
    target.push(relative.parent().unwrap_or(Path::new("")).to_path_buf());
  }
  manifests.sort();
  lockfiles.sort();
  Ok((manifests, lockfiles))
}

/// Finds the lockfile closest to `manifest_dir`, searching upwards.
fn nearest_lock<'a>(
  manifest_dir: &Path,
  locks: &'a BTreeMap<PathBuf, Vec<LockPackage>>,
) -> Option<(&'a PathBuf, &'a Vec<LockPackage>)> {
  manifest_dir.ancestors().find_map(|dir| locks.get_key_value(dir))
}

fn to_slash(path: &Path) -> String {
  path.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
}

#[derive(Deserialize, Default)]
struct Manifest {
  #[serde(flatten)]
  tables: DependencyTables,
  #[serde(default)]
  target: BTreeMap<String, DependencyTables>,
  workspace: Option<Workspace>,
}

#[derive(Deserialize, Default)]
struct Workspace {
  #[serde(default)]
  dependencies: BTreeMap<String, Spec>,
}

#[derive(Deserialize, Default)]
struct DependencyTables {
  #[serde(default)]
  dependencies: BTreeMap<String, Spec>,
  #[serde(default, rename = "dev-dependencies", alias = "dev_dependencies")]
  dev_dependencies: BTreeMap<String, Spec>,
  #[serde(default, rename = "build-dependencies", alias = "build_dependencies")]
  build_dependencies: BTreeMap<String, Spec>,
}

impl DependencyTables {
  fn all(&self) -> impl Iterator<Item = (&String, &Spec)> {
    self.dependencies.iter().chain(&self.dev_dependencies).chain(&self.build_dependencies)
  }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Spec {
  Version(String),
  Detailed(DetailedSpec),
}

#[derive(Deserialize)]
struct DetailedSpec {
  version: Option<String>,
  package: Option<String>,
  registry: Option<String>,
  workspace: Option<bool>,
}

/// A dependency as declared in a manifest.
struct Declared {
  name: String,
  registry: Option<String>,
  version: String,
}

impl Spec {
  /// The real crate name, registry and declared version requirement, if this spec is one we can update.
  /// Path/git-only dependencies have no version and `workspace = true` ones inherit
  /// their version from `[workspace.dependencies]`, so neither are returned.
  fn resolve(&self, key: &str) -> Option<Declared> {
    match self {
      Spec::Version(version) => Some(Declared { name: key.to_string(), registry: None, version: version.clone() }),
      Spec::Detailed(spec) if spec.workspace == Some(true) => None,
      Spec::Detailed(spec) => Some(Declared {
        name: spec.package.clone().unwrap_or_else(|| key.to_string()),
        registry: spec.registry.clone(),
        version: spec.version.clone()?,
      }),
    }
  }
}

/// Returns the dependencies declared by a manifest.
fn parse_manifest(path: &Path) -> Result<Vec<Declared>, CargoError> {
  let manifest: Manifest = read_toml(path)?;

  let workspace = manifest.workspace.iter().flat_map(|w| &w.dependencies);
  let target = manifest.target.values().flat_map(DependencyTables::all);
  Ok(manifest.tables.all().chain(target).chain(workspace).filter_map(|(key, spec)| spec.resolve(key)).collect())
}

#[derive(Deserialize, Default)]
struct Lock {
  #[serde(default)]
  package: Vec<LockPackage>,
}

/// A package recorded in a `Cargo.lock`.
#[derive(Deserialize)]
pub struct LockPackage {
  pub name: String,
  pub version: String,
  pub source: Option<String>,
}

impl LockPackage {
  pub fn is_crates_io(&self) -> bool {
    self
      .source
      .as_deref()
      .is_some_and(|s| s.contains("github.com/rust-lang/crates.io-index") || s.contains("index.crates.io"))
  }

  pub fn is_alternate_registry(&self) -> bool {
    self
      .source
      .as_deref()
      .is_some_and(|s| (s.starts_with("registry+") || s.starts_with("sparse+")) && !self.is_crates_io())
  }
}

/// Reads the packages recorded in the `Cargo.lock` at `path`.
pub fn parse_lock(path: &Path) -> Result<Vec<LockPackage>, CargoError> {
  let lock: Lock = read_toml(path)?;
  Ok(lock.package)
}

/// Reads & deserializes the TOML file at `path`.
fn read_toml<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, CargoError> {
  let content = fs::read_to_string(path).map_err(|source| CargoError::Read { path: path.to_path_buf(), source })?;
  toml::from_str(&content).map_err(|source| CargoError::ParseToml { path: path.to_path_buf(), source })
}

/// Picks the locked version of `name`. A lockfile can hold several versions of one crate, in which
/// case the highest one that satisfies the declared requirement is used.
///
/// A lock entry only records the registry's URL, not the alias used in the manifest, so
/// `alternate_registry` can only tell crates.io apart from any other registry.
fn locked_version(packages: &[LockPackage], name: &str, alternate_registry: bool, requirement: &str) -> Option<String> {
  let mut candidates = packages
    .iter()
    .filter(|p| p.name == name && if alternate_registry { p.is_alternate_registry() } else { p.is_crates_io() });
  let first = candidates.next()?;
  let Some(second) = candidates.next() else {
    return Some(first.version.clone());
  };

  let requirement = VersionReq::parse(requirement).ok()?;
  [first, second]
    .into_iter()
    .chain(candidates)
    .filter_map(|p| Version::parse(&p.version).ok())
    .filter(|v| requirement.matches(v))
    .max()
    .map(|v| v.to_string())
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

  fn find<'a>(deps: &'a [Dependency], name: &str) -> &'a Dependency {
    deps.iter().find(|d| d.name == name).unwrap_or_else(|| panic!("{name} not found"))
  }

  const CRATES_IO: &str = "registry+https://github.com/rust-lang/crates.io-index";
  const CORP: &str = "sparse+https://corp.example.com/index/";

  fn lock(packages: &[(&str, &str)]) -> String {
    lock_from(&packages.iter().map(|(n, v)| (*n, *v, CRATES_IO)).collect::<Vec<_>>())
  }

  fn lock_from(packages: &[(&str, &str, &str)]) -> String {
    packages
      .iter()
      .map(|(n, v, s)| format!("[[package]]\nname = \"{n}\"\nversion = \"{v}\"\nsource = \"{s}\"\n"))
      .collect::<Vec<_>>()
      .join("\n")
  }

  #[test]
  fn reads_all_dependency_tables_and_lock() {
    let dir = TempDir::new().unwrap();
    write(
      dir.path(),
      "Cargo.toml",
      r#"
[package]
name = "app"

[dependencies]
serde = "1.0"
tokio = { version = "1", features = ["full"] }
renamed = { package = "real-name", version = "2" }
local = { path = "../local" }
from_git = { git = "https://example.com/x.git" }

[dev-dependencies]
tempfile = "3"

[build-dependencies]
cc = "1.2"

[target.'cfg(unix)'.dependencies]
libc = "0.2"

[target.x86_64-pc-windows-msvc.dev-dependencies]
winapi = "0.3"
"#,
    );
    write(
      dir.path(),
      "Cargo.lock",
      &lock(&[
        ("serde", "1.0.200"),
        ("tokio", "1.40.0"),
        ("real-name", "2.1.0"),
        ("tempfile", "3.10.0"),
        ("cc", "1.2.1"),
        ("libc", "0.2.150"),
        ("winapi", "0.3.9"),
      ]),
    );

    let deps = discover(dir.path()).unwrap();
    let names: Vec<_> = deps.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, ["cc", "libc", "real-name", "serde", "tempfile", "tokio", "winapi"]);

    let serde = &find(&deps, "serde").occurrences[0];
    assert_eq!(serde.manifest_path, "Cargo.toml");
    assert_eq!(serde.lock_path.as_deref(), Some("Cargo.lock"));
    assert_eq!(serde.current_version, "1.0");
    assert_eq!(serde.locked_version.as_deref(), Some("1.0.200"));
    assert_eq!(find(&deps, "serde").current_version, "1.0.200");
    assert_eq!(find(&deps, "real-name").occurrences[0].locked_version.as_deref(), Some("2.1.0"));
  }

  #[test]
  fn workspace_members_inherit_and_share_the_root_lock() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "Cargo.toml", "[workspace]\nmembers = [\"crates/a\"]\n[workspace.dependencies]\nserde = \"1\"\n");
    write(dir.path(), "Cargo.lock", &lock(&[("serde", "1.0.1"), ("anyhow", "1.0.9")]));
    write(
      dir.path(),
      "crates/a/Cargo.toml",
      "[package]\nname = \"a\"\n[dependencies]\nserde = { workspace = true }\nanyhow = \"1\"\n",
    );

    let deps = discover(dir.path()).unwrap();

    let serde = find(&deps, "serde");
    assert_eq!(serde.occurrences.len(), 1);
    assert_eq!(serde.occurrences[0].manifest_path, "Cargo.toml");

    let anyhow = &find(&deps, "anyhow").occurrences[0];
    assert_eq!(anyhow.manifest_path, "crates/a/Cargo.toml");
    assert_eq!(anyhow.lock_path.as_deref(), Some("Cargo.lock"));
    assert_eq!(anyhow.locked_version.as_deref(), Some("1.0.9"));
  }

  #[test]
  fn nested_crate_prefers_its_own_lock() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "Cargo.toml", "[dependencies]\nserde = \"1\"\n");
    write(dir.path(), "Cargo.lock", &lock(&[("serde", "1.0.1")]));
    write(dir.path(), "tool/Cargo.toml", "[dependencies]\nserde = \"1\"\n");
    write(dir.path(), "tool/Cargo.lock", &lock(&[("serde", "1.0.2")]));

    let deps = discover(dir.path()).unwrap();
    let occurrences = &find(&deps, "serde").occurrences;
    assert_eq!(occurrences.len(), 2);
    assert_eq!(occurrences[1].manifest_path, "tool/Cargo.toml");
    assert_eq!(occurrences[1].lock_path.as_deref(), Some("tool/Cargo.lock"));
    assert_eq!(occurrences[1].locked_version.as_deref(), Some("1.0.2"));
    assert_eq!(find(&deps, "serde").current_version, "1.0.1");
  }

  #[test]
  fn no_lockfile_means_no_locked_version() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "Cargo.toml", "[dependencies]\nserde = \"1\"\n");

    let deps = discover(dir.path()).unwrap();
    let serde = &find(&deps, "serde").occurrences[0];
    assert_eq!(serde.lock_path, None);
    assert_eq!(serde.locked_version, None);
    assert_eq!(find(&deps, "serde").current_version, "1.0.0");
  }

  #[test]
  fn multiple_locked_versions_pick_the_one_matching_the_requirement() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "Cargo.toml", "[dependencies]\nrand = \"0.8\"\n");
    write(
      dir.path(),
      "Cargo.lock",
      &lock(&[("rand", "0.7.3"), ("rand", "0.8.4"), ("rand", "0.8.5"), ("rand", "0.9.0")]),
    );

    let deps = discover(dir.path()).unwrap();
    assert_eq!(find(&deps, "rand").occurrences[0].locked_version.as_deref(), Some("0.8.5"));
  }

  #[test]
  fn same_name_from_different_registries_are_separate_dependencies() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), ".cargo/config.toml", "[registries.corp]\nindex = \"sparse+https://corp.example.com/index\"\n");
    write(
      dir.path(),
      "Cargo.toml",
      "[dependencies]\nfoo = \"1\"\ninternal = { version = \"3\", registry = \"corp\" }\n\
       [dev-dependencies]\nfoo-corp = { package = \"foo\", version = \"1\", registry = \"corp\" }\n",
    );
    write(
      dir.path(),
      "Cargo.lock",
      &lock_from(&[("foo", "1.0.1", CRATES_IO), ("foo", "1.4.0", CORP), ("internal", "3.1.0", CORP)]),
    );

    let deps = discover(dir.path()).unwrap();
    let summary: Vec<_> = deps
      .iter()
      .map(|d| {
        let o = &d.occurrences;
        assert_eq!(o.len(), 1);
        (d.name.as_str(), d.registry.as_deref(), o[0].locked_version.as_deref())
      })
      .collect();
    let corp = Some("https://corp.example.com/index/");
    assert_eq!(
      summary,
      [("foo", None, Some("1.0.1")), ("foo", corp, Some("1.4.0")), ("internal", corp, Some("3.1.0"))]
    );
  }

  #[test]
  fn an_alias_is_resolved_by_the_config_closest_to_each_manifest() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), ".cargo/config.toml", "[registries.corp]\nindex = \"sparse+https://a.example.com/\"\n");
    write(dir.path(), "tool/.cargo/config.toml", "[registries.corp]\nindex = \"sparse+https://b.example.com/\"\n");
    write(dir.path(), "Cargo.toml", "[dependencies]\ninternal = { version = \"1\", registry = \"corp\" }\n");
    write(dir.path(), "tool/Cargo.toml", "[dependencies]\ninternal = { version = \"1\", registry = \"corp\" }\n");

    let deps = discover(dir.path()).unwrap();
    let registries: Vec<_> = deps.iter().map(|d| d.registry.as_deref()).collect();
    assert_eq!(registries, [Some("https://a.example.com/"), Some("https://b.example.com/")]);
  }

  #[test]
  fn an_undefined_alias_is_an_error() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "Cargo.toml", "[dependencies]\ninternal = { version = \"1\", registry = \"corp\" }\n");

    let err = discover(dir.path()).unwrap_err();
    let CargoError::Declaration { dependency, source, .. } = &err else { panic!("unexpected error: {err}") };
    assert_eq!(dependency, "internal");
    assert!(matches!(&**source, CargoError::UndefinedRegistry { alias } if alias == "corp"), "{source}");
  }

  #[test]
  fn skips_dot_git_and_gitignored_paths() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), ".gitignore", "target/\nvendor/*\n");
    write(dir.path(), "Cargo.toml", "[dependencies]\nserde = \"1\"\n");
    write(dir.path(), ".git/Cargo.toml", "[dependencies]\ngit_dir = \"1\"\n");
    write(dir.path(), "target/Cargo.toml", "[dependencies]\ntarget_dir = \"1\"\n");
    write(dir.path(), "vendor/x/Cargo.toml", "[dependencies]\nvendored = \"1\"\n");
    write(dir.path(), "kept/Cargo.toml", "[dependencies]\nkept = \"1\"\n");

    let deps = discover(dir.path()).unwrap();
    let names: Vec<_> = deps.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, ["kept", "serde"]);
  }

  #[test]
  fn invalid_manifest_reports_the_file() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "Cargo.toml", "[dependencies\n");

    let err = discover(dir.path()).unwrap_err();
    assert!(matches!(&err, CargoError::ParseToml { path, .. } if path.ends_with("Cargo.toml")), "{err}");
  }
}
