//! Rewrites the declarations of a dependency in its `Cargo.toml` manifests.
//!
//! Edits are made with `toml_edit` so that formatting and comments are kept. This module only
//! uses plain `std` / `toml_edit` types (plus the WIT records) so that it can be unit tested natively.

use crate::{Dependency, registry, requirement};
use semver::Version;
use std::{collections::BTreeSet, fs, path::Path};
use toml_edit::{DocumentMut, Item, TableLike, Value};

const KINDS: [&str; 5] =
  ["dependencies", "dev-dependencies", "dev_dependencies", "build-dependencies", "build_dependencies"];

/// The features the workspace asks of a dependency, across all of its declarations.
#[derive(Debug, Default, PartialEq)]
pub struct Requested {
  pub features: BTreeSet<String>,
  /// True when any declaration leaves the dependency's default features on.
  pub default_features: bool,
}

/// Bumps every declaration of `dependency`, in each manifest it occurs in beneath `root`, to `to`.
/// Returns the features those declarations request.
pub fn update(root: &Path, dependency: &Dependency, to: &Version) -> Result<Requested, String> {
  let manifests: BTreeSet<&str> = dependency.occurrences.iter().map(|o| o.manifest_path.as_str()).collect();

  let mut requested = Requested::default();
  for manifest in manifests {
    let path = root.join(manifest);
    let manifest_dir = Path::new(manifest).parent().unwrap_or(Path::new(""));
    let content = fs::read_to_string(&path).map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    let mut doc: DocumentMut = content.parse().map_err(|err| format!("failed to parse {}: {err}", path.display()))?;

    let is_same_registry = |alias: Option<&str>| match (alias, dependency.registry.as_deref()) {
      (None, None) => Ok(true),
      (Some(alias), Some(registry)) => Ok(registry::resolve_index(root, manifest_dir, alias)? == registry),
      _ => Ok(false),
    };
    for_each_declaration(&mut doc, &mut |key, item| {
      update_declaration(key, item, dependency, to, &is_same_registry, &mut requested)
    })
    .map_err(|err| format!("{}: {err}", path.display()))?;

    let updated = doc.to_string();
    if updated != content {
      fs::write(&path, updated).map_err(|err| format!("failed to write {}: {err}", path.display()))?;
    }
  }
  Ok(requested)
}

/// Calls `f` with every entry of every dependency table that `discover` reads.
fn for_each_declaration(
  doc: &mut DocumentMut,
  f: &mut dyn FnMut(&str, &mut Item) -> Result<(), String>,
) -> Result<(), String> {
  let root = doc.as_table_mut();
  visit_kinds(root, f)?;
  if let Some(targets) = root.get_mut("target").and_then(Item::as_table_like_mut) {
    for (_, target) in targets.iter_mut() {
      if let Some(target) = target.as_table_like_mut() {
        visit_kinds(target, f)?;
      }
    }
  }
  if let Some(workspace) = root.get_mut("workspace").and_then(Item::as_table_like_mut)
    && let Some(dependencies) = workspace.get_mut("dependencies").and_then(Item::as_table_like_mut)
  {
    visit(dependencies, f)?;
  }
  Ok(())
}

fn visit_kinds(
  table: &mut dyn TableLike,
  f: &mut dyn FnMut(&str, &mut Item) -> Result<(), String>,
) -> Result<(), String> {
  for kind in KINDS {
    if let Some(dependencies) = table.get_mut(kind).and_then(Item::as_table_like_mut) {
      visit(dependencies, f)?;
    }
  }
  Ok(())
}

fn visit(table: &mut dyn TableLike, f: &mut dyn FnMut(&str, &mut Item) -> Result<(), String>) -> Result<(), String> {
  for (key, item) in table.iter_mut() {
    f(key.get(), item)?;
  }
  Ok(())
}

/// Bumps a single declaration if it is one of `dependency`, recording the features it requests.
fn update_declaration(
  key: &str,
  item: &mut Item,
  dependency: &Dependency,
  to: &Version,
  is_same_registry: &dyn Fn(Option<&str>) -> Result<bool, String>,
  requested: &mut Requested,
) -> Result<(), String> {
  if let Some(value) = item.as_value_mut().filter(|v| v.is_str()) {
    if key == dependency.name && is_same_registry(None)? {
      bump_value(value, to);
      requested.default_features = true;
    }
    return Ok(());
  }

  let Some(table) = item.as_table_like_mut() else {
    return Ok(());
  };
  if table.get("workspace").and_then(Item::as_bool) == Some(true) {
    return Ok(());
  }
  let name = table.get("package").and_then(Item::as_str).unwrap_or(key);
  let alias = table.get("registry").and_then(Item::as_str);
  if name != dependency.name || !is_same_registry(alias)? {
    return Ok(());
  }
  let Some(version) = table.get_mut("version").and_then(Item::as_value_mut).filter(|v| v.is_str()) else {
    return Ok(());
  };
  bump_value(version, to);

  let default_features = table.get("default-features").or_else(|| table.get("default_features"));
  requested.default_features |= default_features.and_then(Item::as_bool).unwrap_or(true);
  if let Some(features) = table.get("features").and_then(Item::as_array) {
    requested.features.extend(features.iter().filter_map(Value::as_str).map(str::to_string));
  }
  Ok(())
}

/// Replaces the requirement held by the string `value`, keeping its surrounding whitespace and comments.
fn bump_value(value: &mut Value, to: &Version) {
  let Some(original) = value.as_str() else { return };
  let bumped = requirement::bump(original, to);
  if bumped != original {
    let decor = value.decor().clone();
    *value = Value::from(bumped);
    *value.decor_mut() = decor;
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::Occurrence;
  use tempfile::TempDir;

  fn write(root: &Path, path: &str, content: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
  }

  fn dependency(name: &str, registry: Option<&str>, manifests: &[&str]) -> Dependency {
    Dependency {
      name: name.to_string(),
      registry: registry.map(str::to_string),
      current_version: String::new(),
      occurrences: manifests
        .iter()
        .map(|m| Occurrence {
          manifest_path: m.to_string(),
          lock_path: None,
          current_version: String::new(),
          locked_version: None,
        })
        .collect(),
    }
  }

  #[test]
  fn bumps_every_table_and_keeps_formatting() {
    let dir = TempDir::new().unwrap();
    write(
      dir.path(),
      "Cargo.toml",
      r#"[package]
name = "app" # the app

[dependencies]
serde = "1.0.100"   # keep me
other = "1"
renamed = { package = "serde", version = "^1.0", features = ["derive"] }

[dev-dependencies.serde]
version = "1.0.100"
default-features = false
features = ["rc"]

[target.'cfg(unix)'.build-dependencies]
serde = '1.0.100'

[workspace.dependencies]
serde = { version = "1.0.100" }
inherited = { workspace = true }
"#,
    );

    let requested =
      update(dir.path(), &dependency("serde", None, &["Cargo.toml"]), &Version::parse("1.0.228").unwrap()).unwrap();

    assert_eq!(
      fs::read_to_string(dir.path().join("Cargo.toml")).unwrap(),
      r#"[package]
name = "app" # the app

[dependencies]
serde = "1.0.228"   # keep me
other = "1"
renamed = { package = "serde", version = "^1.0", features = ["derive"] }

[dev-dependencies.serde]
version = "1.0.228"
default-features = false
features = ["rc"]

[target.'cfg(unix)'.build-dependencies]
serde = "1.0.228"

[workspace.dependencies]
serde = { version = "1.0.228" }
inherited = { workspace = true }
"#
    );
    assert_eq!(
      requested,
      Requested { features: ["derive".to_string(), "rc".to_string()].into(), default_features: true }
    );
  }

  #[test]
  fn only_the_matching_registry_is_bumped() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), ".cargo/config.toml", "[registries.corp]\nindex = \"sparse+https://corp.example.com/index\"\n");
    write(
      dir.path(),
      "crates/a/Cargo.toml",
      "[dependencies]\nfoo = \"1.0\"\nfoo-corp = { package = \"foo\", version = \"1.0\", registry = \"corp\" }\n",
    );

    let dep = dependency("foo", Some("https://corp.example.com/index/"), &["crates/a/Cargo.toml"]);
    update(dir.path(), &dep, &Version::parse("2.0.0").unwrap()).unwrap();

    assert_eq!(
      fs::read_to_string(dir.path().join("crates/a/Cargo.toml")).unwrap(),
      "[dependencies]\nfoo = \"1.0\"\nfoo-corp = { package = \"foo\", version = \"2.0\", registry = \"corp\" }\n",
    );
  }

  #[test]
  fn default_features_off_everywhere_is_reported() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "Cargo.toml", "[dependencies]\nfoo = { version = \"1\", default-features = false }\n");

    let requested =
      update(dir.path(), &dependency("foo", None, &["Cargo.toml"]), &Version::parse("1.1.0").unwrap()).unwrap();
    assert!(!requested.default_features);
  }
}
