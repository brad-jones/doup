use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

/// Resolves a plugin's `use` value to a wasm module on disk.
///
/// Relative paths are looked up in `repo_dir` first, then the current working directory.
pub fn resolve_plugin_path(plugin: &str, repo_dir: Option<&Path>) -> Result<PathBuf> {
  let path = Path::new(plugin);

  if path.is_absolute() {
    if path.is_file() {
      return Ok(path.to_path_buf());
    }
    bail!("plugin not found: {}", path.display());
  }

  let mut searched = Vec::new();
  if let Some(repo_dir) = repo_dir {
    searched.push(repo_dir.join(path));
  }
  searched.push(std::env::current_dir()?.join(path));

  if let Some(found) = searched.iter().find(|candidate| candidate.is_file()) {
    return Ok(found.clone());
  }

  if is_wkg_reference(plugin) {
    bail!("wkg package references are not supported yet: {plugin}");
  }

  let searched = searched
    .iter()
    .map(|p| p.display().to_string())
    .collect::<Vec<_>>()
    .join(", ");
  bail!("plugin not found: {plugin} (searched: {searched})")
}

/// A wkg reference (`namespace:package@version`) has a `:` but no path separators.
fn is_wkg_reference(plugin: &str) -> bool {
  plugin.contains(':') && !plugin.contains(['/', '\\'])
}

#[cfg(test)]
mod tests {
  use super::*;
  use assert_fs::{TempDir, prelude::*};

  #[test]
  fn resolves_absolute_path() {
    let dir = TempDir::new().unwrap();
    let wasm = dir.child("a.wasm");
    wasm.touch().unwrap();
    let resolved = resolve_plugin_path(wasm.path().to_str().unwrap(), None).unwrap();
    assert_eq!(resolved, wasm.path());
  }

  #[test]
  fn resolves_relative_to_repo_dir() {
    let dir = TempDir::new().unwrap();
    dir.child("plugins/b.wasm").touch().unwrap();
    let resolved = resolve_plugin_path("./plugins/b.wasm", Some(dir.path())).unwrap();
    assert!(resolved.is_file());
    assert!(resolved.starts_with(dir.path()));
  }

  #[test]
  fn resolves_relative_to_cwd() {
    // Cargo runs unit tests with the crate root as the cwd.
    let resolved = resolve_plugin_path("Cargo.toml", None).unwrap();
    assert!(resolved.is_file());
  }

  #[test]
  fn prefers_repo_dir_over_cwd() {
    let dir = TempDir::new().unwrap();
    dir.child("Cargo.toml").touch().unwrap();
    let resolved = resolve_plugin_path("Cargo.toml", Some(dir.path())).unwrap();
    assert!(resolved.starts_with(dir.path()));
  }

  #[test]
  fn rejects_wkg_references() {
    let err = resolve_plugin_path("doup:github@^1", None).unwrap_err();
    assert!(err.to_string().contains("not supported yet"), "{err}");
  }

  #[test]
  fn errors_when_missing() {
    let err = resolve_plugin_path("./missing.wasm", None).unwrap_err();
    assert!(err.to_string().contains("plugin not found"), "{err}");
  }
}
