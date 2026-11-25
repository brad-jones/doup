//! Works out where a dependency's sparse index lives and the path of a crate within it.
//!
//! This module only uses plain `std` / `serde` types (plus the WIT records) so that it can be
//! unit tested natively, without a wasm runtime.

use crate::Dependency;
use serde::Deserialize;
use std::{collections::BTreeMap, fs, path::Path};

const CRATES_IO_INDEX: &str = "https://index.crates.io/";

/// Returns the URL of the sparse index file describing `dependency`.
///
/// The default registry is crates.io. An alternate registry is an alias that is resolved through the
/// `registries.<alias>.index` key of a `.cargo/config.toml` (or `.cargo/config`) found by walking up from
/// the dependency's manifest to `root`, so the closest config wins as it does in cargo.
/// Only sparse (`sparse+https://…`) indexes are supported, there is no way to clone a git index from the sandbox.
pub fn index_url(root: &Path, dependency: &Dependency) -> Result<String, String> {
  let base = match &dependency.registry {
    None => CRATES_IO_INDEX.to_string(),
    Some(alias) => alternate_index(root, dependency, alias)?,
  };
  Ok(format!("{base}{}", index_path(&dependency.name)))
}

/// The path of a crate inside a cargo index, eg: `se/rd/serde`.
fn index_path(name: &str) -> String {
  let name = name.to_lowercase();
  match name.len() {
    1 | 2 => format!("{}/{name}", name.len()),
    3 => format!("3/{}/{name}", &name[..1]),
    _ => format!("{}/{}/{name}", &name[..2], &name[2..4]),
  }
}

fn alternate_index(root: &Path, dependency: &Dependency, alias: &str) -> Result<String, String> {
  let url = dependency
    .occurrences
    .iter()
    .find_map(|occurrence| {
      let manifest_dir = Path::new(&occurrence.manifest_path).parent().unwrap_or(Path::new(""));
      manifest_dir.ancestors().find_map(|dir| registry_index(&root.join(dir).join(".cargo"), alias).transpose())
    })
    .transpose()?
    .ok_or_else(|| format!("registry `{alias}` of {} is not defined in any .cargo/config.toml", dependency.name))?;

  let Some(url) = url.strip_prefix("sparse+") else {
    return Err(format!("registry `{alias}` uses `{url}`, only sparse+https:// indexes are supported"));
  };
  Ok(if url.ends_with('/') { url.to_string() } else { format!("{url}/") })
}

#[derive(Deserialize, Default)]
struct CargoConfig {
  #[serde(default)]
  registries: BTreeMap<String, Registry>,
}

#[derive(Deserialize)]
struct Registry {
  index: Option<String>,
}

/// Reads the index of `alias` from the cargo config inside `cargo_dir`, if there is one that defines it.
fn registry_index(cargo_dir: &Path, alias: &str) -> Result<Option<String>, String> {
  for file in ["config.toml", "config"] {
    let path = cargo_dir.join(file);
    let Ok(content) = fs::read_to_string(&path) else { continue };
    let config: CargoConfig =
      toml::from_str(&content).map_err(|err| format!("failed to parse {}: {err}", path.display()))?;
    if let Some(index) = config.registries.get(alias).and_then(|r| r.index.clone()) {
      return Ok(Some(index));
    }
  }
  Ok(None)
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::Occurrence;
  use tempfile::TempDir;

  fn dependency(name: &str, registry: Option<&str>, manifest_path: &str) -> Dependency {
    Dependency {
      name: name.to_string(),
      registry: registry.map(str::to_string),
      occurrences: vec![Occurrence {
        manifest_path: manifest_path.to_string(),
        lock_path: None,
        current_version: "1".to_string(),
        locked_version: None,
      }],
    }
  }

  fn write(root: &Path, path: &str, content: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
  }

  #[test]
  fn index_paths_follow_the_cargo_layout() {
    assert_eq!(index_path("a"), "1/a");
    assert_eq!(index_path("ab"), "2/ab");
    assert_eq!(index_path("syn"), "3/s/syn");
    assert_eq!(index_path("Serde"), "se/rd/serde");
  }

  #[test]
  fn default_registry_is_crates_io() {
    let dir = TempDir::new().unwrap();
    let url = index_url(dir.path(), &dependency("serde", None, "Cargo.toml")).unwrap();
    assert_eq!(url, "https://index.crates.io/se/rd/serde");
  }

  #[test]
  fn alias_is_resolved_by_the_closest_config() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), ".cargo/config.toml", "[registries.corp]\nindex = \"sparse+https://root.example.com/index\"\n");
    write(dir.path(), "tool/.cargo/config", "[registries.corp]\nindex = \"sparse+https://tool.example.com/index/\"\n");

    let root = index_url(dir.path(), &dependency("internal", Some("corp"), "Cargo.toml")).unwrap();
    assert_eq!(root, "https://root.example.com/index/in/te/internal");

    let nested = index_url(dir.path(), &dependency("internal", Some("corp"), "tool/Cargo.toml")).unwrap();
    assert_eq!(nested, "https://tool.example.com/index/in/te/internal");
  }

  #[test]
  fn undefined_alias_is_an_error() {
    let dir = TempDir::new().unwrap();
    let err = index_url(dir.path(), &dependency("internal", Some("corp"), "Cargo.toml")).unwrap_err();
    assert!(err.contains("corp"), "{err}");
  }

  #[test]
  fn git_indexes_are_not_supported() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), ".cargo/config.toml", "[registries.corp]\nindex = \"https://git.example.com/index.git\"\n");
    let err = index_url(dir.path(), &dependency("internal", Some("corp"), "Cargo.toml")).unwrap_err();
    assert!(err.contains("sparse"), "{err}");
  }
}
