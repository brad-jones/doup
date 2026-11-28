//! Locks the package plugins of a repo's doup config into its `doup.lock`, see [`lock`].
//!
//! ```no_run
//! use doup_core::lock::{PackageClient, lock};
//!
//! # async fn run() -> Result<(), doup_core::lock::LockError> {
//! let report = lock(None, &PackageClient::new()).await?;
//! for plugin in report.plugins() {
//!   println!("{} {}", plugin.plugin(), plugin.version());
//! }
//! # Ok(())
//! # }
//! ```

use doup_config::ConfigError;
use doup_config::file::ConfigFile;
use doup_config::lock::{DoupLockFile, LOCK_FILE_NAME};
use doup_config::repo::{DoupRepoConfigFile, PackageSpec, PluginRef};
use doup_plugin_host::PluginError;
pub use doup_plugin_host::wkg::PackageClient;
use futures::future::try_join_all;
use semver::Version;
use std::path::{Path, PathBuf};
use tracing::info;

/// Why a repo's plugins could not be locked.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum LockError {
  #[error("no doup config file found in '{}' or its parents", dir.display())]
  NoConfig { dir: PathBuf },

  #[error(transparent)]
  Config(ConfigError),

  #[error("failed to lock plugin '{plugin}'")]
  Plugin { plugin: String, source: Box<PluginError> },
}

/// Whether a plugin's entry in the lock file is new.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LockStatus {
  /// It was already locked, & its content verified (downloading it if it wasn't cached).
  Kept,

  /// It was resolved to the highest matching version.
  Added,
}

/// A package plugin & the version it's locked to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedPlugin {
  plugin: String,
  version: Version,
  status: LockStatus,
}

impl LockedPlugin {
  /// The plugin's `use`, as written in the config.
  pub fn plugin(&self) -> &str {
    &self.plugin
  }

  pub fn version(&self) -> &Version {
    &self.version
  }

  pub fn status(&self) -> LockStatus {
    self.status
  }
}

/// What [`lock`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockReport {
  lock_file: PathBuf,
  plugins: Vec<LockedPlugin>,
  removed: Vec<String>,
  written: bool,
}

impl LockReport {
  /// Where the lock file is.
  pub fn lock_file(&self) -> &Path {
    &self.lock_file
  }

  /// Every package plugin in the config, in config order. Module paths are never locked.
  pub fn plugins(&self) -> &[LockedPlugin] {
    &self.plugins
  }

  /// Entries no plugin uses anymore, so were removed, as `name@version`.
  pub fn removed(&self) -> &[String] {
    &self.removed
  }

  /// Whether the lock file was written, it's left alone when nothing changed.
  pub fn written(&self) -> bool {
    self.written
  }
}

/// Locks every package plugin of the doup config found in `start_dir` (default: the current dir) or its parents,
/// like `cargo generate-lockfile`, writing `doup.lock` next to it.
///
/// Plugins already locked keep their version, others are resolved to the highest published version matching
/// their requirement & entries no plugin uses anymore are removed. Every plugin is downloaded (unless already
/// cached), verifying it & warming the cache. Env overrides are ignored, as the lock is for the file.
///
/// # Errors
///
/// [`LockError::NoConfig`] if there's no config, or any error loading it, its lock file or a plugin.
pub async fn lock(start_dir: Option<&Path>, packages: &PackageClient) -> Result<LockReport, LockError> {
  let config_file = DoupRepoConfigFile::locate_file(start_dir)
    .map_err(LockError::Config)?
    .ok_or_else(|| LockError::NoConfig { dir: start_dir.map_or_else(|| PathBuf::from("."), Path::to_path_buf) })?;
  let config = DoupRepoConfigFile::from_file(&config_file).map_err(LockError::Config)?;
  let lock_file = config.lock_path().unwrap_or_else(|| PathBuf::from(LOCK_FILE_NAME));
  let existing = DoupLockFile::load(&lock_file).map_err(LockError::Config)?;
  let previous = existing.clone().unwrap_or_default();

  let mut specs = Vec::new();
  for plugin in config.plugins() {
    let error = |source| LockError::Plugin { plugin: plugin.plugin.clone(), source: Box::new(source) };
    let reference = plugin.reference().map_err(|source| {
      error(PluginError::InvalidReference { plugin: plugin.plugin.clone(), source: source.into() })
    })?;
    if let PluginRef::Package(spec) = reference {
      specs.push((plugin.plugin.as_str(), spec));
    }
  }

  let resolved = try_join_all(specs.iter().map(|(plugin, spec)| async {
    let error = |source| LockError::Plugin { plugin: plugin.to_string(), source: Box::new(source) };
    match previous.find(spec) {
      Some(locked) => {
        packages.fetch(spec, locked).await.map_err(error)?;
        Ok((locked.clone(), LockStatus::Kept))
      }
      None => Ok((packages.lock(spec).await.map_err(error)?, LockStatus::Added)),
    }
  }))
  .await?;

  let mut lock = DoupLockFile::default();
  let mut plugins = Vec::new();
  for ((plugin, spec), (locked, status)) in specs.iter().zip(resolved) {
    plugins.push(LockedPlugin { plugin: plugin.to_string(), version: locked.version.clone(), status });
    lock.insert(spec, locked);
  }
  let removed = removed(&previous, &lock);

  let written = match &existing {
    Some(existing) => existing != &lock,
    None => !lock.is_empty(),
  };
  if written {
    lock.write(&lock_file).map_err(LockError::Config)?;
    info!(path = %lock_file.display(), "wrote lock file");
  }
  Ok(LockReport { lock_file, plugins, removed, written })
}

/// The entries of `previous` that are not in `current`, as `name@version`.
fn removed(previous: &DoupLockFile, current: &DoupLockFile) -> Vec<String> {
  previous
    .packages()
    .iter()
    .flat_map(|package| package.versions.iter().map(move |locked| (package, locked)))
    .filter(|(package, locked)| {
      let spec = PackageSpec {
        name: &package.name,
        requirement: locked.requirement.clone(),
        registry: package.registry.as_deref(),
      };
      current.find(&spec) != Some(locked)
    })
    .map(|(package, locked)| format!("{}@{}", package.name, locked.version))
    .collect()
}

#[cfg(test)]
mod tests {
  use super::*;
  use doup_plugin_host::wkg::WkgConfig;
  use tempfile::TempDir;

  /// A repo dir with a doup config, plus a local registry its `test` namespace resolves to.
  struct Fixture {
    repo: TempDir,
    registry: TempDir,
    packages: PackageClient,
    _cache: TempDir,
  }

  impl Fixture {
    fn new() -> Self {
      let (repo, registry, cache) = (TempDir::new().unwrap(), TempDir::new().unwrap(), TempDir::new().unwrap());
      // Stops the config search from leaving the repo.
      std::fs::create_dir(repo.path().join(".git")).unwrap();
      let config = WkgConfig::from_toml(&format!(
        "[namespace_registries]\ntest = \"local.test\"\n[registry.\"local.test\"]\ndefault = \"local\"\n[registry.\"local.test\".local]\nroot = {:?}\n",
        registry.path().display().to_string()
      ))
      .unwrap();
      let packages = PackageClient::with_config(config, cache.path().to_path_buf());
      Self { repo, registry, packages, _cache: cache }
    }

    fn publish(&self, name: &str, version: &str) {
      let dir = self.registry.path().join("test").join(name);
      std::fs::create_dir_all(&dir).unwrap();
      std::fs::write(dir.join(format!("{version}.wasm")), format!("{name}@{version}")).unwrap();
    }

    fn config(&self, platform: &str, ecosystems: &[&str]) {
      let ecosystems: String = ecosystems.iter().map(|use_| format!("  - use: {use_}\n")).collect();
      std::fs::write(
        self.repo.path().join("doup.yaml"),
        format!("platform:\n  use: {platform}\necosystems:\n{ecosystems}"),
      )
      .unwrap();
    }

    async fn lock(&self) -> LockReport {
      lock(Some(self.repo.path()), &self.packages).await.unwrap()
    }
  }

  fn summary(report: &LockReport) -> Vec<String> {
    report.plugins().iter().map(|p| format!("{} {} {:?}", p.plugin(), p.version(), p.status())).collect()
  }

  #[tokio::test]
  async fn locks_new_plugins_keeps_locked_ones_and_removes_unused_ones() {
    let fixture = Fixture::new();
    fixture.publish("github", "0.1.0");
    fixture.publish("cargo", "1.0.0");
    fixture.publish("npm", "2.0.0");
    fixture.config("test:github@0.1", &["test:cargo@1", "./local.wasm"]);

    let report = fixture.lock().await;
    assert_eq!(summary(&report), ["test:github@0.1 0.1.0 Added", "test:cargo@1 1.0.0 Added"]);
    assert!(report.written());
    assert_eq!(report.lock_file(), fixture.repo.path().join("doup.lock"));

    // A newer matching release isn't picked up by an existing entry.
    fixture.publish("cargo", "1.1.0");
    let report = fixture.lock().await;
    assert_eq!(summary(&report), ["test:github@0.1 0.1.0 Kept", "test:cargo@1 1.0.0 Kept"]);
    assert!(!report.written());

    fixture.config("test:github@0.1", &["test:npm"]);
    let report = fixture.lock().await;
    assert_eq!(summary(&report), ["test:github@0.1 0.1.0 Kept", "test:npm 2.0.0 Added"]);
    assert_eq!(report.removed(), ["test:cargo@1.0.0"]);
    assert!(report.written());

    let lock = DoupLockFile::load(report.lock_file()).unwrap().unwrap();
    let names: Vec<_> = lock.packages().iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["test:github", "test:npm"]);
  }

  #[tokio::test]
  async fn does_not_write_a_lock_without_package_plugins() {
    let fixture = Fixture::new();
    fixture.config("./platform.wasm", &["./eco.wasm"]);
    let report = fixture.lock().await;
    assert!(report.plugins().is_empty() && !report.written());
    assert!(!report.lock_file().exists());
  }

  #[tokio::test]
  async fn fails_without_a_config_or_a_matching_version() {
    let fixture = Fixture::new();
    let err = lock(Some(fixture.repo.path()), &fixture.packages).await.unwrap_err();
    assert!(matches!(err, LockError::NoConfig { .. }), "{err:?}");

    fixture.publish("github", "0.1.0");
    fixture.config("test:github@1", &["./eco.wasm"]);
    let err = lock(Some(fixture.repo.path()), &fixture.packages).await.unwrap_err();
    let LockError::Plugin { plugin, source } = &err else { panic!("{err:?}") };
    assert_eq!(plugin, "test:github@1");
    assert!(matches!(**source, PluginError::NoMatchingVersion { .. }), "{source:?}");
  }
}
