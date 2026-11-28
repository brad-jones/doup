//! A content addressed cache of downloaded packages, shared by every doup process on the machine.
//!
//! Packages are stored at `<root>/sha256/<hex>.wasm`. wasm-pkg's own `FileCache` isn't used as it writes in
//! place (a concurrent reader can see half a file), never re-checks what it has cached, names files
//! `sha256:<hex>` (not a valid Windows file name) & only hands content back as a stream, where wasmtime wants a path.

use std::io;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;
use tracing::warn;
use wasm_pkg_client::ContentDigest;

/// Names doup's cache dir, plugins are cached in its `plugins` subdir.
pub const CACHE_DIR_ENV: &str = "DOUP_CACHE_DIR";

#[derive(Debug, Clone)]
pub(super) struct PluginCache {
  root: PathBuf,
}

impl PluginCache {
  pub(super) fn new(root: PathBuf) -> Self {
    Self { root }
  }

  /// `$DOUP_CACHE_DIR/plugins`, else `<the OS's cache dir>/doup/plugins`, eg: `~/.cache/doup/plugins`.
  pub(super) fn default_root() -> Option<PathBuf> {
    use etcetera::BaseStrategy;
    let doup = match std::env::var_os(CACHE_DIR_ENV).filter(|dir| !dir.is_empty()) {
      Some(dir) => PathBuf::from(dir),
      None => etcetera::choose_base_strategy().ok()?.cache_dir().join("doup"),
    };
    Some(doup.join("plugins"))
  }

  fn path(&self, digest: &ContentDigest) -> PathBuf {
    let ContentDigest::Sha256 { hex } = digest;
    self.root.join("sha256").join(format!("{hex}.wasm"))
  }

  /// The path & size of the cached content with `digest`, if any. The content is re-hashed first, so a
  /// corrupted entry is never used, it's deleted instead so that it's downloaded again.
  pub(super) async fn get(&self, digest: &ContentDigest) -> io::Result<Option<(PathBuf, u64)>> {
    let path = self.path(digest);
    let content = match tokio::fs::read(&path).await {
      Ok(content) => content,
      Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
      Err(err) => return Err(err),
    };
    let actual = sha256(&content);
    if &actual != digest {
      warn!(path = %path.display(), expected = %digest, %actual, "discarding a corrupt cached plugin");
      tokio::fs::remove_file(&path).await?;
      return Ok(None);
    }
    Ok(Some((path, content.len() as u64)))
  }

  /// Starts writing the content with `digest`, which only appears in the cache once [`CacheWriter::finish`]ed.
  pub(super) async fn writer(&self, digest: &ContentDigest) -> io::Result<CacheWriter> {
    let path = self.path(digest);
    let dir = path.parent().unwrap_or(&self.root).to_path_buf();
    tokio::fs::create_dir_all(&dir).await?;
    // Created in the same dir, so it can be renamed into place atomically.
    let temp =
      tokio::task::spawn_blocking(move || tempfile::NamedTempFile::new_in(dir)).await.map_err(io::Error::other)??;
    let file = tokio::fs::File::from_std(temp.as_file().try_clone()?);
    Ok(CacheWriter { temp, file, path })
  }

  pub(super) fn root(&self) -> &Path {
    &self.root
  }
}

/// Writes content into the cache, see [`PluginCache::writer`]. Dropping it without finishing discards it.
pub(super) struct CacheWriter {
  temp: tempfile::NamedTempFile,
  file: tokio::fs::File,
  path: PathBuf,
}

impl CacheWriter {
  pub(super) async fn write(&mut self, chunk: &[u8]) -> io::Result<()> {
    self.file.write_all(chunk).await
  }

  /// Moves the content into place, returning its path in the cache. The caller must have verified its digest.
  pub(super) async fn finish(mut self) -> io::Result<PathBuf> {
    self.file.flush().await?;
    self.file.sync_all().await?;
    let Self { temp, path, .. } = self;
    tokio::task::spawn_blocking(move || match temp.persist(&path) {
      Ok(_) => Ok(path),
      // Another process cached the same content first, which is just as good.
      Err(_) if path.is_file() => Ok(path),
      Err(err) => Err(err.error),
    })
    .await
    .map_err(io::Error::other)?
  }
}

fn sha256(content: &[u8]) -> ContentDigest {
  use sha2::Digest;
  sha2::Sha256::new_with_prefix(content).into()
}

#[cfg(test)]
mod tests {
  use super::*;

  #[tokio::test]
  async fn caches_content_by_digest_and_discards_corrupt_entries() {
    let dir = tempfile::tempdir().unwrap();
    let cache = PluginCache::new(dir.path().to_path_buf());
    let digest = sha256(b"hello");
    assert_eq!(cache.get(&digest).await.unwrap(), None);

    let mut writer = cache.writer(&digest).await.unwrap();
    writer.write(b"hel").await.unwrap();
    writer.write(b"lo").await.unwrap();
    assert_eq!(cache.get(&digest).await.unwrap(), None, "unfinished content must not be visible");
    let path = writer.finish().await.unwrap();
    assert!(path.ends_with(format!("sha256/{}.wasm", digest.to_string().trim_start_matches("sha256:"))));
    assert_eq!(cache.get(&digest).await.unwrap(), Some((path.clone(), 5)));

    std::fs::write(&path, b"tampered").unwrap();
    assert_eq!(cache.get(&digest).await.unwrap(), None);
    assert!(!path.exists());
  }
}
