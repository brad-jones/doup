//! Applies an update to the workspace: bumps the manifests, then updates every affected lockfile,
//! fetching the index files the resolver asks for.

use crate::{
  Dependency, http,
  index::{self, IndexCache},
  lockfile::Lockfile,
  manifest, registry,
  resolve::{self, ResolveError, Target},
};
use semver::Version;
use std::{collections::BTreeSet, fs, path::Path};

/// Updates every occurrence of `dependency` beneath `root` to version `to`.
pub async fn apply(root: &Path, dependency: &Dependency, to: &str) -> Result<(), String> {
  let to = Version::parse(to).map_err(|err| format!("invalid version `{to}` for {}: {err}", dependency.name))?;
  let requested = manifest::update(root, dependency, &to)?;
  let source = index::source_for_registry(dependency.registry.as_deref());

  let mut cache = IndexCache::default();
  let lock_paths: BTreeSet<&str> = dependency.occurrences.iter().filter_map(|o| o.lock_path.as_deref()).collect();
  for lock_path in lock_paths {
    let path = root.join(lock_path);
    let replaces = dependency
      .occurrences
      .iter()
      .filter(|o| o.lock_path.as_deref() == Some(lock_path))
      .filter_map(|o| o.locked_version.as_deref())
      .map(|v| Version::parse(v).map_err(|err| format!("invalid locked version `{v}`: {err}")))
      .collect::<Result<Vec<_>, _>>()?;
    let target =
      Target { name: &dependency.name, source: &source, to: &to, replaces: &replaces, requested: &requested };

    let content = fs::read_to_string(&path).map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    let lock = Lockfile::parse(&content).map_err(|err| format!("{}: {err}", path.display()))?;
    let updated = loop {
      match resolve::update(&lock, &target, &cache) {
        Ok(updated) => break updated.to_string(),
        Err(ResolveError::Missing(missing)) => {
          for (source, name) in missing {
            if cache.get(&source, &name).is_some() {
              return Err(format!("{}: index file of {name} was requested twice", path.display()));
            }
            fetch(&mut cache, &source, &name).await?;
          }
        }
        Err(ResolveError::Failed(err)) => return Err(format!("{}: {err}", path.display())),
      }
    };

    if updated != content {
      fs::write(&path, updated).map_err(|err| format!("failed to write {}: {err}", path.display()))?;
    }
  }
  Ok(())
}

async fn fetch(cache: &mut IndexCache, source: &str, name: &str) -> Result<(), String> {
  let base = index::index_base(source)?;
  let url = registry::index_url(base.as_deref(), name);
  let body = http::get(&url, &[]).await?.into_success(&url)?;
  cache.insert(source, name, &body)
}
