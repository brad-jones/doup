//! The parts of a registry's sparse index that resolving a lockfile needs, and how lockfile
//! sources map onto index URLs.
//!
//! This module only uses plain `std` / `serde` types so that it can be unit tested natively.

use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};

/// How crates.io is written as a source in `Cargo.lock`, even when fetched over the sparse protocol.
pub const CRATES_IO_SOURCE: &str = "registry+https://github.com/rust-lang/crates.io-index";
pub const CRATES_IO_GIT_INDEX: &str = "https://github.com/rust-lang/crates.io-index";

/// One published version of a crate, see <https://doc.rust-lang.org/cargo/reference/registry-index.html>.
#[derive(Deserialize, Debug, Clone)]
pub struct IndexEntry {
  pub vers: String,
  #[serde(default)]
  pub deps: Vec<IndexDependency>,
  pub cksum: String,
  #[serde(default)]
  pub features: BTreeMap<String, Vec<String>>,
  #[serde(default)]
  pub features2: Option<BTreeMap<String, Vec<String>>>,
  #[serde(default)]
  pub yanked: bool,
  /// The native library the crate links to, only one crate in a graph may link to each.
  #[serde(default)]
  pub links: Option<String>,
  /// The minimum supported rust version, eg: `"1.70"`.
  #[serde(default)]
  pub rust_version: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct IndexDependency {
  /// The name the dependency is known by within the crate, which differs from the crate's real
  /// name when it is renamed.
  pub name: String,
  pub req: String,
  #[serde(default)]
  pub features: Vec<String>,
  #[serde(default)]
  pub optional: bool,
  #[serde(default = "default_true")]
  pub default_features: bool,
  pub kind: Option<String>,
  /// The index URL of the registry the dependency comes from, `None` is the same registry.
  pub registry: Option<String>,
  /// The real name of a renamed dependency.
  pub package: Option<String>,
}

fn default_true() -> bool {
  true
}

impl IndexEntry {
  /// Both feature maps merged, `features2` holds the ones using newer syntax.
  pub fn all_features(&self) -> BTreeMap<&str, &[String]> {
    self
      .features
      .iter()
      .chain(self.features2.iter().flatten())
      .map(|(name, values)| (name.as_str(), values.as_slice()))
      .collect()
  }
}

impl IndexDependency {
  pub fn real_name(&self) -> &str {
    self.package.as_deref().unwrap_or(&self.name)
  }

  pub fn is_dev(&self) -> bool {
    self.kind.as_deref() == Some("dev")
  }

  /// The lockfile source of this dependency, given the source of the crate that depends on it.
  pub fn source(&self, parent_source: &str) -> String {
    match self.registry.as_deref() {
      None => parent_source.to_string(),
      Some(CRATES_IO_GIT_INDEX) => CRATES_IO_SOURCE.to_string(),
      Some(url) if url.starts_with("sparse+") => url.to_string(),
      Some(url) => format!("registry+{url}"),
    }
  }
}

/// The lockfile source of a `Dependency::registry`, which is a sparse index URL or `None` for crates.io.
pub fn source_for_registry(registry: Option<&str>) -> String {
  match registry {
    None => CRATES_IO_SOURCE.to_string(),
    Some(url) => format!("sparse+{url}"),
  }
}

/// The base URL to fetch index files of `source` from, `None` being crates.io.
pub fn index_base(source: &str) -> Result<Option<String>, String> {
  if same_source(source, CRATES_IO_SOURCE) {
    return Ok(None);
  }
  match source.strip_prefix("sparse+") {
    Some(url) if url.ends_with('/') => Ok(Some(url.to_string())),
    Some(url) => Ok(Some(format!("{url}/"))),
    None => Err(format!("unsupported source `{source}`, only crates.io & sparse registries are supported")),
  }
}

/// Whether two lockfile sources are the same, ignoring a trailing slash.
pub fn same_source(a: &str, b: &str) -> bool {
  a.trim_end_matches('/') == b.trim_end_matches('/')
}

/// Whether `source` is a registry, whose packages can be looked up in an index.
pub fn is_registry(source: &str) -> bool {
  source.starts_with("registry+") || source.starts_with("sparse+")
}

/// The index files fetched so far, keyed by source & crate name.
#[derive(Default)]
pub struct IndexCache {
  entries: HashMap<(String, String), Vec<IndexEntry>>,
}

impl IndexCache {
  /// Parses & stores the newline delimited JSON index file of crate `name` from `source`.
  pub fn insert(&mut self, source: &str, name: &str, body: &str) -> Result<(), String> {
    let entries = body
      .lines()
      .filter(|line| !line.trim().is_empty())
      .map(|line| serde_json::from_str(line).map_err(|err| format!("invalid index entry for {name}: {err}")))
      .collect::<Result<_, _>>()?;
    self.entries.insert(Self::key(source, name), entries);
    Ok(())
  }

  pub fn get(&self, source: &str, name: &str) -> Option<&[IndexEntry]> {
    self.entries.get(&Self::key(source, name)).map(Vec::as_slice)
  }

  fn key(source: &str, name: &str) -> (String, String) {
    (source.trim_end_matches('/').to_string(), name.to_string())
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn sources_map_to_index_urls() {
    assert_eq!(index_base(CRATES_IO_SOURCE), Ok(None));
    assert_eq!(index_base("sparse+https://corp.example.com/index"), Ok(Some("https://corp.example.com/index/".into())));
    assert!(index_base("registry+https://git.example.com/index.git").is_err());
  }

  #[test]
  fn dependency_sources_follow_the_registry_field() {
    let dep = |registry: Option<&str>| IndexDependency {
      name: "x".into(),
      req: "1".into(),
      features: vec![],
      optional: false,
      default_features: true,
      kind: None,
      registry: registry.map(str::to_string),
      package: None,
    };
    assert_eq!(dep(None).source("sparse+https://a/"), "sparse+https://a/");
    assert_eq!(dep(Some(CRATES_IO_GIT_INDEX)).source("sparse+https://a/"), CRATES_IO_SOURCE);
    assert_eq!(dep(Some("sparse+https://b/")).source(CRATES_IO_SOURCE), "sparse+https://b/");
  }
}
