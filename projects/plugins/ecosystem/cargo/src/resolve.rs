//! Updates a single package within a `Cargo.lock`, a much smaller job than full resolution.
//!
//! A plugin runs inside a WASM sandbox so it can't shell out to `cargo update --precise`.
//! Instead the lockfile is edited much like cargo would do it:
//!
//! 1. The target package moves to its new version and checksum. A semver compatible old version
//!    is replaced everywhere it is referenced. An incompatible one (eg: a `latest` major bump) is
//!    only replaced in references from workspace members (packages without a `source`). Other
//!    registry packages keep depending on the old version.
//! 2. Every new or changed package's dependencies are recomputed from its index entry. Dev
//!    dependencies are skipped and every target is included, as a lockfile is target
//!    independent. Each requirement is met by the first that works of:
//!    - an already locked version that satisfies it,
//!    - the locked version in the same semver compatible range, upgraded in place to the highest
//!      version that satisfies it,
//!    - a new package at the highest non-yanked version that satisfies it.
//!
//!    Changed and added packages are then processed the same way.
//! 3. Packages no longer reachable from the workspace members are removed.
//!
//! Optional dependencies follow the features requested of a package. These come from the edges
//! of changed packages, the workspace manifests (for the target) and the old version's
//! features, which are inferred from the optional dependencies that were present. A package
//! whose version doesn't change only ever gains newly enabled optional dependencies.
//!
//! # Limitations
//!
//! - Feature unification is only approximated, so an optional dependency enabled indirectly by
//!   another registry package's feature may be missed. Cargo corrects it on the next build, but
//!   `--locked` builds would fail.
//! - A dependent's requirement is assumed to accept a semver compatible upgrade.
//! - `rust-version`, `links`, `[patch]`, `[replace]` & git based registries aren't considered.
//!
//! The resolver is pure. When it needs an index file that hasn't been fetched it returns
//! [`ResolveError::Missing`], and the caller fetches the files and tries again.

use crate::index::{IndexCache, IndexDependency, IndexEntry, same_source};
use crate::lockfile::{Lockfile, Package, PackageId};
use crate::manifest::Requested;
use semver::{Version, VersionReq};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

type Features = BTreeSet<String>;

/// The package to update.
pub struct Target<'a> {
  pub name: &'a str,
  /// The lockfile source of the package, see [`crate::index::source_for_registry`].
  pub source: &'a str,
  pub to: &'a Version,
  /// The versions the workspace's occurrences were locked to. Workspace references to these
  /// move to `to` even when it is a breaking change.
  pub replaces: &'a [Version],
  /// The features the workspace manifests request of the package.
  pub requested: &'a Requested,
}

#[derive(Debug, PartialEq)]
pub enum ResolveError {
  /// The index files of these `(source, name)` pairs are needed, fetch them and try again.
  Missing(BTreeSet<(String, String)>),
  Failed(String),
}

/// Returns `lock` with `target` updated, see the module docs.
pub fn update(lock: &Lockfile, target: &Target, index: &IndexCache) -> Result<Lockfile, ResolveError> {
  let mut resolver = Resolver::new(lock, index);
  let Some(entries) = index.get(target.source, target.name) else {
    return Err(ResolveError::Missing([(target.source.to_string(), target.name.to_string())].into()));
  };
  let entry = find_entry(entries, target.to)
    .ok_or_else(|| ResolveError::Failed(format!("{}@{} is not in the index", target.name, target.to)))?;

  let olds: Vec<PackageId> = resolver.find(target.name, target.source).cloned().collect();
  if olds.is_empty() {
    return Ok(lock.with_packages(lock.packages.clone()));
  }

  let new = PackageId {
    name: target.name.to_string(),
    version: target.to.clone(),
    source: Some(resolver.canonical_source(target.source)),
  };
  for old in olds.into_iter().filter(|old| *old != new) {
    if compatible(&old.version, &new.version) {
      resolver.replace(&old, &new, &entry.cksum);
    } else if target.replaces.contains(&old.version) {
      resolver.repoint_workspace(&old, &new);
    }
  }
  if !resolver.packages.contains_key(&new) {
    resolver.add(&new, &entry.cksum);
  }

  let mut features = target.requested.features.clone();
  if target.requested.default_features {
    features.insert("default".to_string());
  }
  resolver.queue.push_back((new, features));
  resolver.run()?;
  resolver.collect_garbage();
  Ok(lock.with_packages(resolver.packages.into_values().collect()))
}

struct Resolver<'a> {
  index: &'a IndexCache,
  packages: BTreeMap<PackageId, Package>,
  /// Packages whose dependencies are recomputed from the index, as they are new or a different version.
  changed: HashSet<PackageId>,
  /// The package each changed package replaced, if any.
  previous: HashMap<PackageId, Package>,
  /// Where each replaced package went.
  renamed: HashMap<PackageId, PackageId>,
  /// The features applied to each package so far.
  applied: HashMap<PackageId, Features>,
  queue: VecDeque<(PackageId, Features)>,
  missing: BTreeSet<(String, String)>,
}

impl<'a> Resolver<'a> {
  fn new(lock: &Lockfile, index: &'a IndexCache) -> Self {
    Self {
      index,
      packages: lock.packages.iter().map(|p| (p.id.clone(), p.clone())).collect(),
      changed: HashSet::new(),
      previous: HashMap::new(),
      renamed: HashMap::new(),
      applied: HashMap::new(),
      queue: VecDeque::new(),
      missing: BTreeSet::new(),
    }
  }

  /// The locked packages called `name` from `source`.
  fn find(&self, name: &str, source: &str) -> impl Iterator<Item = &PackageId> {
    self
      .packages
      .keys()
      .filter(move |id| id.name == name && id.source.as_deref().is_some_and(|s| same_source(s, source)))
  }

  /// `source` as already written in the lockfile, if it is there, so a trailing slash is consistent.
  fn canonical_source(&self, source: &str) -> String {
    self
      .packages
      .keys()
      .filter_map(|id| id.source.as_deref())
      .find(|s| same_source(s, source))
      .unwrap_or(source)
      .to_string()
  }

  /// Follows `id` through any replacements to the package that holds it now.
  fn current(&self, id: &PackageId) -> PackageId {
    let mut id = id;
    while let Some(next) = self.renamed.get(id) {
      id = next;
    }
    id.clone()
  }

  fn add(&mut self, id: &PackageId, checksum: &str) {
    let package =
      Package { id: id.clone(), checksum: Some(checksum.to_string()), dependencies: Vec::new(), replace: None };
    self.packages.insert(id.clone(), package);
    self.changed.insert(id.clone());
  }

  /// Swaps `old` for `new` everywhere, keeping the old package around to infer its features from.
  fn replace(&mut self, old: &PackageId, new: &PackageId, checksum: &str) {
    let Some(previous) = self.packages.remove(old) else { return };
    for package in self.packages.values_mut() {
      for dependency in package.dependencies.iter_mut().filter(|d| *d == old) {
        *dependency = new.clone();
      }
    }
    if !self.packages.contains_key(new) {
      self.add(new, checksum);
      self.previous.insert(new.clone(), previous);
    }
    self.renamed.insert(old.clone(), new.clone());
    if let Some(features) = self.applied.remove(old) {
      self.queue.push_back((new.clone(), features));
    }
  }

  /// Swaps `old` for `new` in the dependencies of workspace members only.
  fn repoint_workspace(&mut self, old: &PackageId, new: &PackageId) {
    for package in self.packages.values_mut().filter(|p| p.id.source.is_none()) {
      for dependency in package.dependencies.iter_mut().filter(|d| *d == old) {
        *dependency = new.clone();
      }
    }
  }

  fn run(&mut self) -> Result<(), ResolveError> {
    while let Some((id, features)) = self.queue.pop_front() {
      self.process(&self.current(&id), features)?;
    }
    if self.missing.is_empty() { Ok(()) } else { Err(ResolveError::Missing(std::mem::take(&mut self.missing))) }
  }

  /// Applies `requested` features to the package `id`, adding the dependencies they need.
  fn process(&mut self, id: &PackageId, requested: Features) -> Result<(), ResolveError> {
    let (Some(source), true) = (id.source.clone(), self.packages.contains_key(id)) else {
      return Ok(());
    };
    let Some(entries) = self.index.get(&source, &id.name) else {
      self.missing.insert((source, id.name.clone()));
      return Ok(());
    };
    let entry = find_entry(entries, &id.version)
      .ok_or_else(|| ResolveError::Failed(format!("{id} is locked but is not in the index")))?;

    let is_changed = self.changed.contains(id);
    let first_time = !self.applied.contains_key(id);
    let mut features = self.applied.get(id).cloned().unwrap_or_default();
    if first_time && is_changed {
      features.extend(self.carried_features(id, entries, entry));
    }
    let before = features.len();
    features.extend(requested);
    if !first_time && features.len() == before {
      return Ok(());
    }
    self.applied.insert(id.clone(), features.clone());

    let activation = activate(entry, &features);
    let mut dependencies = if is_changed { Vec::new() } else { self.packages[id].dependencies.clone() };
    for dependency in entry.deps.iter().filter(|d| !d.is_dev()) {
      if dependency.optional && !activation.dependencies.contains(&dependency.name) {
        continue;
      }
      let dependency_source = self.canonical_source(&dependency.source(&source));
      let already_present = || {
        dependencies.iter().any(|d| {
          d.name == dependency.real_name() && d.source.as_deref().is_some_and(|s| same_source(s, &dependency_source))
        })
      };
      // A package whose version is unchanged keeps its dependencies, it only gains new optional ones.
      if !is_changed && (!dependency.optional || already_present()) {
        continue;
      }
      let Some(selected) = self.select(id, dependency, &dependency_source)? else { continue };

      let mut dependency_features: Features = dependency.features.iter().cloned().collect();
      if dependency.default_features {
        dependency_features.insert("default".to_string());
      }
      dependency_features.extend(activation.dependency_features.get(&dependency.name).into_iter().flatten().cloned());
      dependencies.push(selected.clone());
      self.queue.push_back((selected, dependency_features));
    }

    let mut dependencies: Vec<PackageId> = dependencies.iter().map(|d| self.current(d)).collect();
    dependencies.sort();
    dependencies.dedup();
    if let Some(package) = self.packages.get_mut(id) {
      package.dependencies = dependencies;
    }
    Ok(())
  }

  /// Picks the package that satisfies `dependency` of `parent`, adding or upgrading one if needed.
  /// `None` when an index file is missing.
  fn select(
    &mut self,
    parent: &PackageId,
    dependency: &IndexDependency,
    source: &str,
  ) -> Result<Option<PackageId>, ResolveError> {
    let name = dependency.real_name();
    let requirement = VersionReq::parse(&dependency.req).map_err(|err| {
      ResolveError::Failed(format!("{parent} has an invalid requirement `{}` on {name}: {err}", dependency.req))
    })?;

    if let Some(locked) = self.find(name, source).filter(|id| requirement.matches(&id.version)).max() {
      return Ok(Some(locked.clone()));
    }

    let Some(entries) = self.index.get(source, name) else {
      self.missing.insert((source.to_string(), name.to_string()));
      return Ok(None);
    };
    let (version, entry) = entries
      .iter()
      .filter(|e| !e.yanked)
      .filter_map(|e| Version::parse(&e.vers).ok().map(|v| (v, e)))
      .filter(|(v, _)| requirement.matches(v))
      .max_by(|a, b| a.0.cmp(&b.0))
      .ok_or_else(|| {
        ResolveError::Failed(format!("no version of {name} matches `{}`, as required by {parent}", dependency.req))
      })?;

    let new = PackageId { name: name.to_string(), version, source: Some(source.to_string()) };
    let checksum = entry.cksum.clone();
    let existing = self.find(name, source).find(|id| compatible(&id.version, &new.version)).cloned();
    match existing {
      Some(old) => self.replace(&old, &new, &checksum),
      None => self.add(&new, &checksum),
    }
    Ok(Some(new))
  }

  /// The features a changed package keeps from the version it replaced.
  ///
  /// The old version's optional dependencies that were locked stay enabled. `default` stays on
  /// unless it would have enabled an optional dependency that wasn't locked. Other features can't
  /// be told apart from the lockfile alone. Guessing them over-enables features of dependencies,
  /// and the dependencies of unchanged packages are kept anyway.
  fn carried_features(&self, id: &PackageId, entries: &[IndexEntry], entry: &IndexEntry) -> Features {
    let Some(previous) = self.previous.get(id) else { return Features::new() };
    let Some(old_entry) = find_entry(entries, &previous.id.version) else { return Features::new() };

    let locked: HashSet<&str> = previous.dependencies.iter().map(|d| d.name.as_str()).collect();
    let enabled: HashSet<&str> =
      old_entry.deps.iter().filter(|d| d.optional && locked.contains(d.real_name())).map(|d| d.name.as_str()).collect();

    let mut carried = Features::new();
    let default_needs = activate(old_entry, &["default".to_string()].into()).dependencies;
    if default_needs.iter().all(|d| enabled.contains(d.as_str())) {
      carried.insert("default".to_string());
    }
    for dependency in entry.deps.iter().filter(|d| d.optional && enabled.contains(d.name.as_str())) {
      carried.insert(format!("dep:{}", dependency.name));
    }
    carried
  }

  /// Removes every package that the workspace members no longer depend on.
  fn collect_garbage(&mut self) {
    let mut stack: Vec<PackageId> =
      self.packages.values().filter(|p| p.id.source.is_none() || p.replace.is_some()).map(|p| p.id.clone()).collect();
    if stack.is_empty() {
      return;
    }
    let mut reachable = HashSet::new();
    while let Some(id) = stack.pop() {
      if let Some(package) = self.packages.get(&id)
        && reachable.insert(id)
      {
        stack.extend(package.dependencies.iter().cloned());
      }
    }
    self.packages.retain(|id, _| reachable.contains(id));
  }
}

fn find_entry<'e>(entries: &'e [IndexEntry], version: &Version) -> Option<&'e IndexEntry> {
  entries.iter().find(|e| Version::parse(&e.vers).is_ok_and(|v| v == *version))
}

/// Whether two versions are semver compatible, ie: cargo allows only one of them per source.
fn compatible(a: &Version, b: &Version) -> bool {
  match (a.major, a.minor) {
    _ if a.major != b.major => false,
    (1.., _) => true,
    _ if a.minor != b.minor => false,
    (0, 1..) => true,
    _ => a.patch == b.patch,
  }
}

/// What a set of features turns on within a package.
#[derive(Debug, Default)]
struct Activation {
  /// The (in crate) names of the optional dependencies enabled.
  dependencies: HashSet<String>,
  /// The features requested of each dependency, by its in crate name.
  dependency_features: HashMap<String, Features>,
}

/// Expands `requested` through the feature table of `entry`, following cargo's feature syntax:
/// `feature`, `dep:name`, `name/feature` & `name?/feature`. An optional dependency not referenced
/// with `dep:` anywhere also has an implicit feature of the same name.
fn activate(entry: &IndexEntry, requested: &Features) -> Activation {
  let table = entry.all_features();
  let explicit: HashSet<&str> = table.values().flat_map(|v| v.iter()).filter_map(|v| v.strip_prefix("dep:")).collect();
  let optional: HashSet<&str> = entry.deps.iter().filter(|d| d.optional).map(|d| d.name.as_str()).collect();
  let implicit = |name: &str| optional.contains(name) && !explicit.contains(name);

  let mut activation = Activation::default();
  let mut weak = Vec::new();
  let mut seen = HashSet::new();
  let mut stack: Vec<String> = requested.iter().cloned().collect();
  while let Some(feature) = stack.pop() {
    if !seen.insert(feature.clone()) {
      continue;
    }
    if let Some(name) = feature.strip_prefix("dep:") {
      activation.dependencies.insert(name.to_string());
    } else if let Some((name, dependency_feature)) = feature.split_once('/') {
      if let Some(name) = name.strip_suffix('?') {
        weak.push((name.to_string(), dependency_feature.to_string()));
        continue;
      }
      if optional.contains(name) {
        activation.dependencies.insert(name.to_string());
      }
      activation.dependency_features.entry(name.to_string()).or_default().insert(dependency_feature.to_string());
    } else if let Some(values) = table.get(feature.as_str()) {
      stack.extend(values.iter().cloned());
    } else if implicit(&feature) {
      activation.dependencies.insert(feature);
    }
  }
  for (name, feature) in weak {
    if !optional.contains(name.as_str()) || activation.dependencies.contains(&name) {
      activation.dependency_features.entry(name).or_default().insert(feature);
    }
  }
  activation
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::index::CRATES_IO_SOURCE;
  use serde_json::{Value, json};

  const HEADER: &str =
    "# This file is automatically @generated by Cargo.\n# It is not intended for manual editing.\nversion = 4\n";

  /// Builds a lockfile from `(name, version, is_registry, dependencies)`.
  fn lock(packages: &[(&str, &str, bool, &[&str])]) -> Lockfile {
    let mut text = HEADER.to_string();
    for (name, version, registry, dependencies) in packages {
      text.push_str(&format!("\n[[package]]\nname = \"{name}\"\nversion = \"{version}\"\n"));
      if *registry {
        text.push_str(&format!("source = \"{CRATES_IO_SOURCE}\"\nchecksum = \"{name}-{version}\"\n"));
      }
      if !dependencies.is_empty() {
        text.push_str("dependencies = [\n");
        for d in *dependencies {
          text.push_str(&format!(" \"{d}\",\n"));
        }
        text.push_str("]\n");
      }
    }
    Lockfile::parse(&text).unwrap()
  }

  fn dep(name: &str, req: &str) -> Value {
    json!({ "name": name, "req": req, "features": [], "optional": false, "default_features": true, "kind": "normal" })
  }

  fn optional(name: &str, req: &str) -> Value {
    json!({ "name": name, "req": req, "features": [], "optional": true, "default_features": true, "kind": "normal" })
  }

  fn version(name: &str, vers: &str, deps: Vec<Value>, features: Value) -> String {
    json!({ "name": name, "vers": vers, "deps": deps, "cksum": format!("{name}-{vers}"), "features": features, "yanked": false })
      .to_string()
  }

  fn index(crates: &[(&str, Vec<String>)]) -> IndexCache {
    let mut cache = IndexCache::default();
    for (name, versions) in crates {
      cache.insert(CRATES_IO_SOURCE, name, &versions.join("\n")).unwrap();
    }
    cache
  }

  fn run(
    lock: &Lockfile,
    index: &IndexCache,
    name: &str,
    to: &str,
    replaces: &[&str],
    requested: Requested,
  ) -> Result<Lockfile, ResolveError> {
    let to = Version::parse(to).unwrap();
    let replaces: Vec<Version> = replaces.iter().map(|v| Version::parse(v).unwrap()).collect();
    let target = Target { name, source: CRATES_IO_SOURCE, to: &to, replaces: &replaces, requested: &requested };
    update(lock, &target, index)
  }

  fn defaults() -> Requested {
    Requested { features: Features::new(), default_features: true }
  }

  /// `name version -> [dependencies]` for every package, to compare against compactly.
  fn summary(lock: &Lockfile) -> Vec<String> {
    let mut packages: Vec<_> = lock.packages.iter().collect();
    packages.sort_by(|a, b| a.id.cmp(&b.id));
    packages
      .iter()
      .map(|p| {
        let deps: Vec<String> = p.dependencies.iter().map(|d| format!("{} {}", d.name, d.version)).collect();
        format!("{} {} -> [{}]", p.id.name, p.id.version, deps.join(", "))
      })
      .collect()
  }

  #[test]
  fn missing_index_files_are_requested() {
    let lock = lock(&[("app", "0.1.0", false, &["foo"]), ("foo", "1.0.0", true, &[])]);
    let err = run(&lock, &IndexCache::default(), "foo", "1.1.0", &["1.0.0"], defaults()).unwrap_err();
    assert_eq!(err, ResolveError::Missing([(CRATES_IO_SOURCE.to_string(), "foo".to_string())].into()));

    let index = index(&[("foo", vec![version("foo", "1.1.0", vec![dep("bar", "^1")], json!({}))])]);
    let err = run(&lock, &index, "foo", "1.1.0", &["1.0.0"], defaults()).unwrap_err();
    assert_eq!(err, ResolveError::Missing([(CRATES_IO_SOURCE.to_string(), "bar".to_string())].into()));
  }

  #[test]
  fn compatible_update_replaces_everywhere_and_resolves_new_dependencies() {
    let lock = lock(&[
      ("app", "0.1.0", false, &["foo", "other"]),
      ("foo", "1.0.0", true, &["old-dep"]),
      ("old-dep", "1.0.0", true, &[]),
      ("other", "1.0.0", true, &["foo"]),
      ("shared", "1.0.0", true, &[]),
    ]);
    let index = index(&[
      (
        "foo",
        vec![
          version("foo", "1.0.0", vec![dep("old-dep", "^1")], json!({})),
          version(
            "foo",
            "1.1.0",
            vec![
              dep("shared", "^1.2"),
              dep("new-dep", "^2"),
              json!({ "name": "test-only", "req": "^1", "kind": "dev" }),
            ],
            json!({}),
          ),
        ],
      ),
      (
        "shared",
        vec![
          version("shared", "1.0.0", vec![], json!({})),
          version("shared", "1.2.0", vec![], json!({})),
          version("shared", "1.3.0", vec![], json!({})),
          version("shared", "2.0.0", vec![], json!({})),
        ],
      ),
      ("new-dep", vec![version("new-dep", "2.0.0", vec![], json!({})), version("new-dep", "2.1.0", vec![], json!({}))]),
    ]);

    let updated = run(&lock, &index, "foo", "1.1.0", &["1.0.0"], defaults()).unwrap();
    assert_eq!(
      summary(&updated),
      [
        "app 0.1.0 -> [foo 1.1.0, other 1.0.0]",
        "foo 1.1.0 -> [new-dep 2.1.0, shared 1.3.0]",
        "new-dep 2.1.0 -> []",
        "other 1.0.0 -> [foo 1.1.0]",
        "shared 1.3.0 -> []",
      ]
    );
    let foo = updated.packages.iter().find(|p| p.id.name == "foo").unwrap();
    assert_eq!(foo.checksum.as_deref(), Some("foo-1.1.0"));
  }

  #[test]
  fn breaking_update_only_moves_the_workspace() {
    let lock = lock(&[
      ("app", "0.1.0", false, &["foo", "other"]),
      ("foo", "1.0.0", true, &[]),
      ("other", "1.0.0", true, &["foo"]),
    ]);
    let index = index(&[("foo", vec![version("foo", "2.0.0", vec![], json!({}))])]);

    let updated = run(&lock, &index, "foo", "2.0.0", &["1.0.0"], defaults()).unwrap();
    assert_eq!(
      summary(&updated),
      ["app 0.1.0 -> [foo 2.0.0, other 1.0.0]", "foo 1.0.0 -> []", "foo 2.0.0 -> []", "other 1.0.0 -> [foo 1.0.0]"]
    );
    assert!(updated.to_string().contains(" \"foo 2.0.0\",\n"), "{updated}");
  }

  #[test]
  fn optional_dependencies_follow_features() {
    let lock =
      lock(&[("app", "0.1.0", false, &["foo"]), ("foo", "1.0.0", true, &["serde"]), ("serde", "1.0.0", true, &[])]);
    // `markdown` only enables locked optional deps, yet it must not be assumed on as it would add `md`.
    let features = json!({
      "default": ["std"], "std": [], "extra": ["dep:extra", "serde?/derive"], "markdown": ["serde/markdown"]
    });
    let index = index(&[
      (
        "foo",
        vec![
          version("foo", "1.0.0", vec![optional("serde", "^1"), optional("unused", "^1")], features.clone()),
          version(
            "foo",
            "1.1.0",
            vec![optional("serde", "^1"), optional("unused", "^1"), optional("extra", "^1")],
            features,
          ),
        ],
      ),
      ("serde", vec![version("serde", "1.0.0", vec![optional("md", "^1")], json!({ "markdown": ["dep:md"] }))]),
      ("extra", vec![version("extra", "1.0.0", vec![], json!({}))]),
    ]);

    let plain = run(&lock, &index, "foo", "1.1.0", &["1.0.0"], defaults()).unwrap();
    assert_eq!(summary(&plain), ["app 0.1.0 -> [foo 1.1.0]", "foo 1.1.0 -> [serde 1.0.0]", "serde 1.0.0 -> []"]);

    let requested = Requested { features: ["extra".to_string()].into(), default_features: true };
    let extra = run(&lock, &index, "foo", "1.1.0", &["1.0.0"], requested).unwrap();
    assert_eq!(
      summary(&extra),
      ["app 0.1.0 -> [foo 1.1.0]", "extra 1.0.0 -> []", "foo 1.1.0 -> [extra 1.0.0, serde 1.0.0]", "serde 1.0.0 -> []"]
    );
  }

  #[test]
  fn unchanged_packages_gain_newly_requested_optional_dependencies() {
    let lock =
      lock(&[("app", "0.1.0", false, &["foo"]), ("foo", "1.0.0", true, &["bar"]), ("bar", "1.0.0", true, &[])]);
    let bar_dep = json!({ "name": "bar", "req": "^1", "features": ["fast"], "optional": false, "default_features": false, "kind": "normal" });
    let index = index(&[
      ("foo", vec![version("foo", "1.1.0", vec![bar_dep], json!({}))]),
      ("bar", vec![version("bar", "1.0.0", vec![optional("simd", "^1")], json!({ "fast": ["simd"] }))]),
      ("simd", vec![version("simd", "1.0.0", vec![], json!({}))]),
    ]);

    let updated = run(&lock, &index, "foo", "1.1.0", &["1.0.0"], defaults()).unwrap();
    assert_eq!(
      summary(&updated),
      ["app 0.1.0 -> [foo 1.1.0]", "bar 1.0.0 -> [simd 1.0.0]", "foo 1.1.0 -> [bar 1.0.0]", "simd 1.0.0 -> []"]
    );
  }

  #[test]
  fn compatibility_follows_cargo() {
    let v = |s: &str| Version::parse(s).unwrap();
    assert!(compatible(&v("1.0.0"), &v("1.9.0")));
    assert!(!compatible(&v("1.0.0"), &v("2.0.0")));
    assert!(compatible(&v("0.3.0"), &v("0.3.9")));
    assert!(!compatible(&v("0.3.0"), &v("0.4.0")));
    assert!(!compatible(&v("0.0.1"), &v("0.0.2")));
  }
}
