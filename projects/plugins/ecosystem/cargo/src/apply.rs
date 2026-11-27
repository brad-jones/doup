//! Applies an update to the workspace: bumps the manifests, then re-resolves every affected
//! lockfile, fetching the index files the resolver asks for.

use crate::{
  Dependency, http,
  index::{self, IndexCache},
  lockfile::{Lockfile, PackageId},
  manifest, registry,
  resolve::{self, ResolveError, Target},
  workspace::{self, Workspace},
};
use futures::{StreamExt, stream};
use semver::Version;
use std::{
  collections::{BTreeMap, BTreeSet, HashMap},
  fs,
  path::Path,
};

/// How many index files are fetched at once.
const CONCURRENT_FETCHES: usize = 16;

/// Updates every occurrence of `dependency` beneath `root` to version `to`.
pub async fn apply(root: &Path, dependency: &Dependency, to: &str) -> Result<(), String> {
  let to = Version::parse(to).map_err(|err| format!("invalid version `{to}` for {}: {err}", dependency.name))?;
  let source = index::source_for_registry(dependency.registry.as_deref());
  let mut cache = IndexCache::default();

  // Before anything changes, re-resolve each lockfile as is. Any difference means it is out of
  // date (which cargo would also fix) or the resolver doesn't model the workspace exactly, so the
  // update will carry those differences too.
  let lock_paths: BTreeSet<&str> = dependency.occurrences.iter().filter_map(|o| o.lock_path.as_deref()).collect();
  let mut locks = Vec::new();
  for lock_path in lock_paths {
    let path = root.join(lock_path);
    let lock_dir = Path::new(lock_path).parent().unwrap_or(Path::new("")).to_path_buf();
    let content = fs::read_to_string(&path).map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    let lock = Lockfile::parse(&content).map_err(|err| format!("{}: {err}", path.display()))?;

    let workspace = load_workspace(root, &lock_dir, &lock).map_err(|err| format!("{}: {err}", path.display()))?;
    let baseline = resolve(&path, &lock, &workspace, None, &mut cache).await?;
    if baseline.to_string() != content {
      eprintln!(
        "warning: {} changes even without updating {} ({}). Either it is out of date or doup doesn't \
         resolve this workspace exactly as cargo would, the update includes these changes.",
        path.display(),
        dependency.name,
        describe_differences(&lock, &baseline)
      );
    }
    locks.push((path, lock_dir, content, lock));
  }

  manifest::update(root, dependency, &to)?;

  for (path, lock_dir, content, lock) in locks {
    let workspace = load_workspace(root, &lock_dir, &lock).map_err(|err| format!("{}: {err}", path.display()))?;
    let target = Target { name: &dependency.name, source: &source, to: &to };
    let updated = resolve(&path, &lock, &workspace, Some(&target), &mut cache).await?.to_string();
    if updated != content {
      fs::write(&path, updated).map_err(|err| format!("failed to write {}: {err}", path.display()))?;
    }
  }
  Ok(())
}

fn load_workspace(root: &Path, lock_dir: &Path, lock: &Lockfile) -> Result<Workspace, String> {
  let locked: HashMap<String, Version> =
    lock.packages.iter().filter(|p| p.id.source.is_none()).map(|p| (p.id.name.clone(), p.id.version.clone())).collect();
  workspace::load(root, lock_dir, &locked)
}

/// Resolves `lock`, fetching index files until the resolver has every one it needs.
async fn resolve(
  path: &Path,
  lock: &Lockfile,
  workspace: &Workspace,
  target: Option<&Target<'_>>,
  cache: &mut IndexCache,
) -> Result<Lockfile, String> {
  loop {
    match resolve::update(lock, workspace, target, cache) {
      Ok(resolved) => return Ok(resolved),
      Err(ResolveError::Failed(err)) => return Err(format!("{}: {err}", path.display())),
      Err(ResolveError::Missing(missing)) => {
        if let Some((_, name)) = missing.iter().find(|(source, name)| cache.get(source, name).is_some()) {
          return Err(format!("{}: the index file of {name} was requested twice", path.display()));
        }
        let fetched: Vec<_> = stream::iter(missing)
          .map(|(source, name)| async move {
            let body = fetch(&source, &name).await;
            (source, name, body)
          })
          .buffer_unordered(CONCURRENT_FETCHES)
          .collect()
          .await;
        for (source, name, body) in fetched {
          cache.insert(&source, &name, &body?)?;
        }
      }
    }
  }
}

async fn fetch(source: &str, name: &str) -> Result<String, String> {
  let base = index::index_base(source)?;
  let url = registry::index_url(base.as_deref(), name);
  http::get(&url, &[]).await?.into_success(&url)
}

/// Summarises how `resolved` differs from `lock`, eg: `+1 -2 ~3: foo@1.0.0, …`.
fn describe_differences(lock: &Lockfile, resolved: &Lockfile) -> String {
  let packages = |lock: &Lockfile| -> BTreeMap<PackageId, Vec<PackageId>> {
    lock.packages.iter().map(|p| (p.id.clone(), p.dependencies.clone())).collect()
  };
  let (before, after) = (packages(lock), packages(resolved));
  let added: Vec<_> = after.keys().filter(|id| !before.contains_key(id)).collect();
  let removed: Vec<_> = before.keys().filter(|id| !after.contains_key(id)).collect();
  let changed: Vec<_> =
    after.iter().filter(|(id, deps)| before.get(id).is_some_and(|b| b != *deps)).map(|(id, _)| id).collect();
  let examples: Vec<String> = added.iter().chain(&removed).chain(&changed).take(5).map(|id| id.to_string()).collect();
  format!("+{} -{} ~{}: {}", added.len(), removed.len(), changed.len(), examples.join(", "))
}
