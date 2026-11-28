//! Downloads plugins published as wasm packages, eg: `doup:ecosystem-plugin-cargo@0.1.0`, see [`PackageClient`].
//!
//! Packages are found with [wasm-pkg](https://github.com/bytecodealliance/wasm-pkg-tools), the library behind `wkg`,
//! so any registry `wkg` can pull from works: OCI registries (eg: `ghcr.io`) & wasm-pkg registries. Which registry a
//! namespace maps to comes from a plugin's `registry`, else doup's built-in `doup` namespace, the user's wkg config &
//! [`CONFIG_FILE_ENV`]. Registry credentials are found the same way `wkg` finds them, eg:
//! `WKG_REGISTRY_GHCR_IO_AUTH_BEARER` or docker's credential helpers.
//!
//! Downloads are cached by their content digest in [`CACHE_DIR_ENV`] (default `<the OS's cache dir>/doup`) & a
//! locked package that's already cached is used without touching the network at all.

mod cache;
mod progress;
mod registry;

pub use cache::CACHE_DIR_ENV;
pub use registry::CONFIG_FILE_ENV;
/// The wkg config a [`PackageClient`] resolves registries with, see [`PackageClient::with_config`].
pub use wasm_pkg_client::Config as WkgConfig;

use crate::error::{BoxError, PluginError};
use cache::PluginCache;
use doup_config::lock::LockedVersion;
use doup_config::repo::PackageSpec;
use futures_util::TryStreamExt;
use progress::{DownloadProgress, download_span};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::OnceCell;
use tracing::{Instrument, debug};
use wasm_pkg_client::{Client, ContentDigest, PackageRef, Release, Version, VersionInfo, VersionReq};

type Result<T, E = PluginError> = std::result::Result<T, E>;

/// Identifies a [`Client`]: the plugin's `registry` & the package's namespace, or `None` for the default config.
type ClientKey = Option<(String, String)>;

/// A cached package: its path & size in bytes.
type Cached = (PathBuf, u64);

/// Resolves & downloads wasm packages, caching what it downloads.
///
/// Cheap to clone, clones share the same registry clients & downloads, so the same package needed by many repos
/// at once is only downloaded once.
#[derive(Clone, Default)]
pub struct PackageClient {
  inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
  /// The config for packages without a registry of their own, loaded on first use.
  config: OnceCell<WkgConfig>,

  /// Where packages are cached, `None` for [`PluginCache::default_root`].
  cache_dir: Option<PathBuf>,

  clients: Mutex<HashMap<ClientKey, Client>>,

  /// Downloads by content digest, so concurrent requests for the same content share one.
  downloads: Mutex<HashMap<String, Arc<OnceCell<Cached>>>>,
}

impl std::fmt::Debug for PackageClient {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("PackageClient").field("cache_dir", &self.inner.cache_dir).finish_non_exhaustive()
  }
}

impl PackageClient {
  /// A client using the default config (see the [module docs](self)) & cache dir, both determined on first use.
  pub fn new() -> Self {
    Self::default()
  }

  /// A client resolving packages without a registry of their own with `config` alone & caching in `cache_dir`.
  pub fn with_config(config: WkgConfig, cache_dir: PathBuf) -> Self {
    Self { inner: Arc::new(Inner { config: OnceCell::from(config), cache_dir: Some(cache_dir), ..Default::default() }) }
  }

  /// Resolves `spec` to the highest published version matching its requirement & downloads it, so it's
  /// cached & its size is known. This is what `doup lock` records.
  ///
  /// # Errors
  ///
  /// [`PluginError::NoMatchingVersion`] if nothing matches, or any error resolving or downloading it.
  pub async fn lock(&self, spec: &PackageSpec<'_>) -> Result<LockedVersion> {
    let package = package_ref(spec)?;
    let client = self.client(spec, &package).await?;
    let resolve_err =
      |source: wasm_pkg_client::Error| PluginError::Resolve { plugin: spec.to_string(), source: source.into() };

    let versions = client.list_all_versions(&package).await.map_err(resolve_err)?;
    let version = select_version(&versions, &spec.requirement)
      .ok_or_else(|| PluginError::NoMatchingVersion { plugin: spec.to_string(), available: available(&versions) })?;
    debug!(package = spec.name, %version, "resolved");
    let release = client.get_release(&package, &version).await.map_err(resolve_err)?;
    let (_, size) = self.cached_or_download(spec, &package, &release, None).await?;
    Ok(LockedVersion {
      requirement: spec.requirement.clone(),
      version,
      digest: release.content_digest.to_string(),
      size,
    })
  }

  /// The path of the package `locked` pins `spec` to, downloading it unless it's already cached.
  ///
  /// # Errors
  ///
  /// Any error downloading or caching it, including [`PluginError::Download`] if its content doesn't match the
  /// locked digest.
  pub async fn fetch(&self, spec: &PackageSpec<'_>, locked: &LockedVersion) -> Result<PathBuf> {
    let package = package_ref(spec)?;
    let content_digest: ContentDigest = locked.digest.parse().map_err(|source: wasm_pkg_client::Error| {
      PluginError::InvalidDigest { plugin: spec.to_string(), source: source.into() }
    })?;
    let release = Release { version: locked.version.clone(), content_digest };
    let (path, _) = self.cached_or_download(spec, &package, &release, Some(locked.size)).await?;
    Ok(path)
  }

  /// The release from the cache, else downloaded into it. Concurrent calls for the same content share one download.
  async fn cached_or_download(
    &self,
    spec: &PackageSpec<'_>,
    package: &PackageRef,
    release: &Release,
    size: Option<u64>,
  ) -> Result<Cached> {
    let digest = release.content_digest.to_string();
    let cell = self.inner.downloads.lock().unwrap_or_else(PoisonError::into_inner).entry(digest).or_default().clone();
    let cached = cell
      .get_or_try_init(|| async {
        let cache = self.cache()?;
        let cache_err = |source| PluginError::Cache { path: cache.root().to_path_buf(), source };
        if let Some(cached) = cache.get(&release.content_digest).await.map_err(cache_err)? {
          debug!(package = spec.name, version = %release.version, path = %cached.0.display(), "using cached package");
          return Ok(cached);
        }
        let client = self.client(spec, package).await?;
        let span = download_span!(spec.name, release.version);
        download(&cache, &client, spec, package, release, size).instrument(span).await
      })
      .await?;
    Ok(cached.clone())
  }

  fn cache(&self) -> Result<PluginCache> {
    let root = match &self.inner.cache_dir {
      Some(dir) => dir.clone(),
      None => PluginCache::default_root().ok_or(PluginError::NoCacheDir)?,
    };
    Ok(PluginCache::new(root))
  }

  /// The client for `spec`'s registry, created on first use.
  async fn client(&self, spec: &PackageSpec<'_>, package: &PackageRef) -> Result<Client> {
    let config_err =
      |source: wasm_pkg_client::Error| PluginError::RegistryConfig { plugin: spec.to_string(), source: source.into() };
    let base = self.inner.config.get_or_try_init(registry::default_config).await.map_err(config_err)?;
    let key: ClientKey = spec.registry.map(|registry| (registry.to_string(), package.namespace().to_string()));
    let mut clients = self.inner.clients.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(client) = clients.get(&key) {
      return Ok(client.clone());
    }
    let config = match spec.registry {
      Some(registry) => registry::explicit_config(base, package, registry).map_err(config_err)?,
      None => base.clone(),
    };
    let client = Client::new(config);
    clients.insert(key, client.clone());
    Ok(client)
  }
}

/// Downloads `release` into `cache`, run it within a [`download_span`].
async fn download(
  cache: &PluginCache,
  client: &Client,
  spec: &PackageSpec<'_>,
  package: &PackageRef,
  release: &Release,
  size: Option<u64>,
) -> Result<Cached> {
  let download_err = |source: BoxError| PluginError::Download { plugin: spec.to_string(), source };
  let cache_err = |source| PluginError::Cache { path: cache.root().to_path_buf(), source };

  let mut progress = DownloadProgress::start(tracing::Span::current(), size);
  // Validated against the release's digest by wasm-pkg, the stream fails at its end if it doesn't match.
  let mut stream = client.stream_content(package, release).await.map_err(|err| download_err(err.into()))?;
  let mut writer = cache.writer(&release.content_digest).await.map_err(cache_err)?;
  while let Some(chunk) = stream.try_next().await.map_err(|err| download_err(err.into()))? {
    progress.advance(chunk.len() as u64);
    if let Some(size) = size
      && progress.done() > size
    {
      return Err(download_err(format!("its content is larger than the {size} bytes it was locked with").into()));
    }
    writer.write(&chunk).await.map_err(cache_err)?;
  }
  let done = progress.done();
  let path = writer.finish().await.map_err(cache_err)?;
  progress.finish();
  Ok((path, done))
}

fn package_ref(spec: &PackageSpec<'_>) -> Result<PackageRef> {
  spec.name.parse().map_err(|source: wasm_pkg_client::Error| PluginError::InvalidReference {
    plugin: spec.to_string(),
    source: source.into(),
  })
}

/// The highest version that isn't yanked & matches `requirement`.
fn select_version(versions: &[VersionInfo], requirement: &VersionReq) -> Option<Version> {
  versions
    .iter()
    .filter(|info| !info.yanked && requirement.matches(&info.version))
    .map(|info| &info.version)
    .max()
    .cloned()
}

/// Every version that isn't yanked, in ascending order.
fn available(versions: &[VersionInfo]) -> Vec<String> {
  let mut versions: Vec<_> = versions.iter().filter(|info| !info.yanked).map(|info| &info.version).collect();
  versions.sort();
  versions.into_iter().map(Version::to_string).collect()
}

#[cfg(test)]
pub(crate) mod tests {
  use super::*;
  use std::path::Path;

  /// A client whose `test` namespace resolves to a local dir registry, at `<registry>/test/<name>/<version>.wasm`.
  pub(crate) fn local_client(registry: &Path, cache: &Path) -> PackageClient {
    let config = WkgConfig::from_toml(&format!(
      r#"
      [namespace_registries]
      test = "local.test"

      [registry."local.test"]
      default = "local"

      [registry."local.test".local]
      root = {:?}
      "#,
      registry.display().to_string()
    ))
    .unwrap();
    PackageClient::with_config(config, cache.to_path_buf())
  }

  /// Publishes `content` as `test:<name>@<version>` to the local registry at `registry`.
  pub(crate) fn publish(registry: &Path, name: &str, version: &str, content: &[u8]) {
    let dir = registry.join("test").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{version}.wasm")), content).unwrap();
  }

  pub(crate) fn spec<'a>(name: &'a str, requirement: &str) -> PackageSpec<'a> {
    PackageSpec { name, requirement: VersionReq::parse(requirement).unwrap(), registry: None }
  }

  fn info(version: &str, yanked: bool) -> VersionInfo {
    VersionInfo { version: Version::parse(version).unwrap(), yanked }
  }

  #[test]
  fn selects_the_highest_matching_version_that_is_not_yanked() {
    let versions = [info("0.1.0", false), info("0.1.2", true), info("0.1.1", false), info("0.2.0", false)];
    let select =
      |requirement| select_version(&versions, &VersionReq::parse(requirement).unwrap()).map(|v| v.to_string());
    assert_eq!(select("0.1.0").as_deref(), Some("0.1.1"));
    assert_eq!(select("*").as_deref(), Some("0.2.0"));
    assert_eq!(select("=0.1.2"), None);
    assert_eq!(select("1"), None);
  }

  #[tokio::test]
  async fn locks_then_fetches_from_the_cache() {
    let (registry, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    publish(registry.path(), "plugin", "0.1.0", b"old");
    publish(registry.path(), "plugin", "0.1.1", b"newer");
    publish(registry.path(), "plugin", "0.2.0", b"breaking");
    let client = local_client(registry.path(), cache.path());

    let locked = client.lock(&spec("test:plugin", "0.1.0")).await.unwrap();
    assert_eq!((locked.version.to_string(), locked.size), ("0.1.1".to_string(), 5));

    // Removing the registry proves the locked package now comes from the cache alone.
    drop(registry);
    let path = client.fetch(&spec("test:plugin", "0.1.0"), &locked).await.unwrap();
    assert!(path.starts_with(cache.path()));
    assert_eq!(std::fs::read(&path).unwrap(), b"newer");

    // As does a fresh client, with nothing in memory.
    let fresh = local_client(Path::new("/does/not/exist"), cache.path());
    assert_eq!(fresh.fetch(&spec("test:plugin", "0.1.0"), &locked).await.unwrap(), path);
  }

  #[tokio::test]
  async fn rejects_content_that_does_not_match_the_lock() {
    let (registry, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    publish(registry.path(), "plugin", "1.0.0", b"original");
    let mut locked = local_client(registry.path(), cache.path()).lock(&spec("test:plugin", "1")).await.unwrap();

    // Republished with different content, as a mutable OCI tag could be.
    publish(registry.path(), "plugin", "1.0.0", b"tampered");
    let other_cache = tempfile::tempdir().unwrap();
    let client = local_client(registry.path(), other_cache.path());
    let err = client.fetch(&spec("test:plugin", "1"), &locked).await.unwrap_err();
    assert!(matches!(err, PluginError::Download { .. }), "{err:?}");

    locked.digest = "not-a-digest".into();
    let err = client.fetch(&spec("test:plugin", "1"), &locked).await.unwrap_err();
    assert!(matches!(err, PluginError::InvalidDigest { .. }), "{err:?}");
  }

  #[tokio::test]
  async fn reports_the_available_versions_when_none_match() {
    let (registry, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    publish(registry.path(), "plugin", "0.1.0", b"a");
    publish(registry.path(), "plugin", "0.2.0", b"b");
    let err = local_client(registry.path(), cache.path()).lock(&spec("test:plugin", "1")).await.unwrap_err();
    let PluginError::NoMatchingVersion { available, .. } = &err else { panic!("{err:?}") };
    assert_eq!(available, &["0.1.0", "0.2.0"]);
  }
}
