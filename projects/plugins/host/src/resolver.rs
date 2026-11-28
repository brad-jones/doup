//! Finds the wasm module of a plugin, see [`ModuleResolver`].

use crate::error::PluginError;
use crate::wkg::PackageClient;
use doup_config::lock::{DoupLockFile, LOCK_FILE_NAME};
use doup_config::repo::{DoupRepoPlugin, PluginRef};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Finds the wasm module a [`DoupRepoPlugin`] refers to:
///
/// - A module path is used as is when absolute, a relative one is looked for in the current dir & then
///   each of the search dirs, in order.
/// - A package reference is looked up in the lock file & the exact release it pins is downloaded
///   (or taken from the cache) by the [`PackageClient`]. A package missing from the lock file is an error,
///   so what runs is always what was locked.
///
/// Cheap to clone.
#[derive(Debug, Clone)]
pub struct ModuleResolver {
  packages: PackageClient,
  search_dirs: Vec<PathBuf>,
  lock: Arc<DoupLockFile>,
  lock_file: PathBuf,
}

impl ModuleResolver {
  /// A resolver with an empty lock file, see [`Self::with_lock`].
  pub fn new(packages: PackageClient, search_dirs: Vec<PathBuf>) -> Self {
    Self { packages, search_dirs, lock: Arc::default(), lock_file: PathBuf::from(LOCK_FILE_NAME) }
  }

  /// Resolves package references with `lock`, which was loaded from `lock_file` (used in errors).
  pub fn with_lock(self, lock: Arc<DoupLockFile>, lock_file: PathBuf) -> Self {
    Self { lock, lock_file, ..self }
  }

  /// The path of `plugin`'s wasm module.
  ///
  /// # Errors
  ///
  /// [`PluginError::NotFound`] for a module that doesn't exist, [`PluginError::NotLocked`] for a package that
  /// isn't in the lock file, or any error from [`PackageClient::fetch`].
  pub async fn resolve(&self, plugin: &DoupRepoPlugin) -> Result<PathBuf, PluginError> {
    let reference = plugin
      .reference()
      .map_err(|source| PluginError::InvalidReference { plugin: plugin.plugin.clone(), source: source.into() })?;
    match reference {
      PluginRef::Module(path) => self.find_module(&plugin.plugin, path),
      PluginRef::Package(spec) => {
        let locked = self
          .lock
          .find(&spec)
          .ok_or_else(|| PluginError::NotLocked { plugin: plugin.plugin.clone(), lock_file: self.lock_file.clone() })?;
        self.packages.fetch(&spec, locked).await
      }
    }
  }

  fn find_module(&self, plugin: &str, path: &Path) -> Result<PathBuf, PluginError> {
    if path.is_absolute() {
      if path.is_file() {
        return Ok(path.to_path_buf());
      }
      return Err(PluginError::NotFound { plugin: plugin.to_string(), searched: vec![path.to_path_buf()] });
    }

    let mut searched = vec![std::env::current_dir().map_err(PluginError::CurrentDir)?.join(path)];
    for candidate in self.search_dirs.iter().map(|dir| dir.join(path)) {
      if !searched.contains(&candidate) {
        searched.push(candidate);
      }
    }
    match searched.iter().find(|candidate| candidate.is_file()) {
      Some(found) => Ok(found.clone()),
      None => Err(PluginError::NotFound { plugin: plugin.to_string(), searched }),
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::wkg::tests::{local_client, publish, spec};

  fn plugin(reference: &str) -> DoupRepoPlugin {
    DoupRepoPlugin { plugin: reference.to_string(), registry: None, with: None }
  }

  #[tokio::test]
  async fn missing_modules_report_everywhere_searched() {
    let dir = tempfile::tempdir().unwrap();
    let resolver = ModuleResolver::new(PackageClient::new(), vec![dir.path().to_path_buf()]);
    let err = resolver.resolve(&plugin("missing.wasm")).await.unwrap_err();
    let PluginError::NotFound { plugin, searched } = &err else { panic!("unexpected error: {err}") };
    assert_eq!(plugin, "missing.wasm");
    assert_eq!(searched.last(), Some(&dir.path().join("missing.wasm")));
  }

  #[tokio::test]
  async fn modules_are_found_in_the_search_dirs() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("found.wasm"), b"").unwrap();
    let resolver = ModuleResolver::new(PackageClient::new(), vec![dir.path().to_path_buf()]);
    assert_eq!(resolver.resolve(&plugin("found.wasm")).await.unwrap(), dir.path().join("found.wasm"));
  }

  #[tokio::test]
  async fn packages_must_be_locked() {
    let (registry, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    publish(registry.path(), "plugin", "0.1.0", b"content");
    let client = local_client(registry.path(), cache.path());

    let resolver = ModuleResolver::new(client.clone(), Vec::new());
    let err = resolver.resolve(&plugin("test:plugin@0.1.0")).await.unwrap_err();
    assert!(
      matches!(&err, PluginError::NotLocked { plugin, lock_file } if plugin == "test:plugin@0.1.0" && lock_file == Path::new("doup.lock")),
      "{err}"
    );

    let mut lock = DoupLockFile::default();
    lock.insert(&spec("test:plugin", "0.1.0"), client.lock(&spec("test:plugin", "0.1.0")).await.unwrap());
    let resolver = resolver.with_lock(Arc::new(lock), PathBuf::from("/repo/doup.lock"));
    let path = resolver.resolve(&plugin("test:plugin@0.1.0")).await.unwrap();
    assert_eq!(std::fs::read(path).unwrap(), b"content");

    // A different requirement is a different lock entry.
    let err = resolver.resolve(&plugin("test:plugin@0.2")).await.unwrap_err();
    assert!(
      matches!(&err, PluginError::NotLocked { lock_file, .. } if lock_file == Path::new("/repo/doup.lock")),
      "{err}"
    );
  }
}
