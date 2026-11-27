//! Resolves cargo registry aliases and works out where a crate lives within a sparse index.
//!
//! This module only uses plain `std` / `serde` types (plus the WIT records) so that it can be
//! unit tested natively, without a wasm runtime.

use crate::error::CargoError;
use serde::Deserialize;
use std::{collections::BTreeMap, fs, path::Path};

const CRATES_IO_INDEX: &str = "https://index.crates.io/";

/// Returns the URL of the sparse index file describing the crate `name`.
///
/// `registry` is the index URL produced by [`resolve_index`], `None` is crates.io.
pub fn index_url(registry: Option<&str>, name: &str) -> String {
  format!("{}{}", registry.unwrap_or(CRATES_IO_INDEX), index_path(name))
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

/// Resolves the registry `alias` used by the manifest at `manifest_dir` (relative to `root`) to its sparse index URL.
///
/// The alias is looked up through the `registries.<alias>.index` key of a `.cargo/config.toml` (or `.cargo/config`)
/// found by walking up from `manifest_dir` to `root`, so the closest config wins as it does in cargo.
/// Only sparse (`sparse+https://…`) indexes are supported, there is no way to clone a git index from the sandbox.
pub fn resolve_index(root: &Path, manifest_dir: &Path, alias: &str) -> Result<String, CargoError> {
  let url = manifest_dir
    .ancestors()
    .find_map(|dir| registry_index(&root.join(dir).join(".cargo"), alias).transpose())
    .transpose()?
    .ok_or_else(|| CargoError::UndefinedRegistry { alias: alias.to_string() })?;

  let Some(url) = url.strip_prefix("sparse+") else {
    return Err(CargoError::UnsupportedRegistry { alias: alias.to_string(), url });
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
fn registry_index(cargo_dir: &Path, alias: &str) -> Result<Option<String>, CargoError> {
  for file in ["config.toml", "config"] {
    let path = cargo_dir.join(file);
    let Ok(content) = fs::read_to_string(&path) else { continue };
    let config: CargoConfig =
      toml::from_str(&content).map_err(|source| CargoError::ParseToml { path: path.clone(), source })?;
    if let Some(index) = config.registries.get(alias).and_then(|r| r.index.clone()) {
      return Ok(Some(index));
    }
  }
  Ok(None)
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

  #[test]
  fn index_paths_follow_the_cargo_layout() {
    assert_eq!(index_path("a"), "1/a");
    assert_eq!(index_path("ab"), "2/ab");
    assert_eq!(index_path("syn"), "3/s/syn");
    assert_eq!(index_path("Serde"), "se/rd/serde");
  }

  #[test]
  fn default_registry_is_crates_io() {
    assert_eq!(index_url(None, "serde"), "https://index.crates.io/se/rd/serde");
  }

  #[test]
  fn resolved_registry_is_used_as_the_index_base() {
    assert_eq!(
      index_url(Some("https://corp.example.com/index/"), "internal"),
      "https://corp.example.com/index/in/te/internal"
    );
  }

  #[test]
  fn alias_is_resolved_by_the_closest_config() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), ".cargo/config.toml", "[registries.corp]\nindex = \"sparse+https://root.example.com/index\"\n");
    write(dir.path(), "tool/.cargo/config", "[registries.corp]\nindex = \"sparse+https://tool.example.com/index/\"\n");

    let root = resolve_index(dir.path(), Path::new(""), "corp").unwrap();
    assert_eq!(root, "https://root.example.com/index/");

    let nested = resolve_index(dir.path(), Path::new("tool"), "corp").unwrap();
    assert_eq!(nested, "https://tool.example.com/index/");
  }

  #[test]
  fn undefined_alias_is_an_error() {
    let dir = TempDir::new().unwrap();
    let err = resolve_index(dir.path(), Path::new(""), "corp").unwrap_err();
    assert!(matches!(&err, CargoError::UndefinedRegistry { alias } if alias == "corp"), "{err}");
  }

  #[test]
  fn git_indexes_are_not_supported() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), ".cargo/config.toml", "[registries.corp]\nindex = \"https://git.example.com/index.git\"\n");
    let err = resolve_index(dir.path(), Path::new(""), "corp").unwrap_err();
    assert!(matches!(err, CargoError::UnsupportedRegistry { .. }), "{err}");
  }
}
