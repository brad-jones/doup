//! Resolves a `Cargo.lock` the way cargo does, after one package has moved to a new version.
//!
//! A plugin runs inside a WASM sandbox, so it can't run `cargo update --precise`. Instead the
//! lockfile's dependency graph is rebuilt using the same rules cargo applies when it writes one:
//!
//! - Workspace members resolve with every feature & their dev-dependencies, as a lockfile must
//!   suit any build of the workspace. Their versions come from their manifests, so a stale
//!   lockfile is brought up to date as cargo would.
//! - Every other package (from a registry, or a path dependency outside the workspace) resolves
//!   with the union of the features its dependents request of it, expanded through its feature
//!   table. Dev-dependencies are skipped and every target is included, as a lockfile is target
//!   independent. A weak `dep?/feature` enables the optional dependency, as it does in cargo's
//!   dependency resolver.
//! - A requirement is met by, in order: the version its dependent was locked to, any locked
//!   version, an upgrade of the locked version in the same semver compatible range, or a new
//!   package. A new version is the highest that isn't yanked, doesn't clash on `links` and, with
//!   `incompatible-rust-versions = "fallback"`, supports the workspace's rust version if any does.
//! - Packages that are `[patch]`ed, from git or otherwise unresolvable keep the dependencies they
//!   were locked with. Their dependents get the optional dependencies they were locked with.
//! - Whatever the workspace members no longer reach is dropped.
//!
//! Upgrading a package part way through can leave features requested by its old version behind,
//! so resolution is repeated until a pass adds or upgrades nothing.
//!
//! The resolver is pure. When it needs an index file that hasn't been fetched it returns
//! [`ResolveError::Missing`], and the caller fetches the files and tries again.
//!
//! # Limitations
//!
//! - Unlike cargo, a conflict is never backtracked out of, it is an error.
//! - `[patch]` & `[replace]` in `.cargo/config.toml`, `min-publish-age` and git based registries
//!   aren't considered. `[replace]` in a manifest is an error.

use crate::index::{IndexCache, IndexEntry, is_registry, same_source};
use crate::lockfile::{Lockfile, Package, PackageId};
use crate::workspace::{LocalPackage, LocalSource, Workspace, parse_rust_version};
use semver::{Version, VersionReq};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

type Features = BTreeSet<String>;

/// Requested of a package by a dependent that can't be resolved, standing in for the features it
/// was locked with.
const LOCKED_FEATURES: &str = "?locked";
const MAX_PASSES: usize = 10;

/// The package to update.
pub struct Target<'a> {
  pub name: &'a str,
  /// The lockfile source of the package, see [`crate::index::source_for_registry`].
  pub source: &'a str,
  pub to: &'a Version,
}

#[derive(Debug, PartialEq)]
pub enum ResolveError {
  /// The index files of these `(source, name)` pairs are needed, fetch them and try again.
  Missing(BTreeSet<(String, String)>),
  Failed(String),
}

/// Resolves `lock` for `workspace`, first moving `target` (if any) to its new version.
/// Without a target, a lockfile that is up to date comes back unchanged.
pub fn update(
  lock: &Lockfile,
  workspace: &Workspace,
  target: Option<&Target>,
  index: &IndexCache,
) -> Result<Lockfile, ResolveError> {
  let previous: BTreeMap<PackageId, Package> = lock.packages.iter().map(|p| (p.id.clone(), p.clone())).collect();
  let mut state = State {
    candidates: previous
      .values()
      .filter(|p| p.id.source.as_deref().is_some_and(is_registry))
      .map(|p| (p.id.clone(), p.checksum.clone()))
      .collect(),
    renamed: HashMap::new(),
  };
  for id in previous.keys().filter(|id| id.source.is_none()) {
    if let Some(local) = workspace.package(&id.name)
      && local.version != id.version
    {
      state.renamed.insert(id.clone(), PackageId { version: local.version.clone(), ..id.clone() });
    }
  }
  if let Some(target) = target {
    state.apply(target, index, &previous)?;
  }

  for _ in 0..MAX_PASSES {
    let mut pass = Pass {
      index,
      workspace,
      previous: &previous,
      state: &mut state,
      features: HashMap::new(),
      processed: HashSet::new(),
      edges: HashMap::new(),
      queue: VecDeque::new(),
      missing: BTreeSet::new(),
      changed: false,
    };
    pass.run()?;
    if !pass.changed {
      return pass.finish(lock);
    }
  }
  Err(ResolveError::Failed(format!("the lockfile did not settle after {MAX_PASSES} passes")))
}

/// What carries over between passes: the registry package versions available to pick from.
struct State {
  candidates: BTreeMap<PackageId, Option<String>>,
  /// Where each replaced package went.
  renamed: HashMap<PackageId, PackageId>,
}

impl State {
  fn apply(
    &mut self,
    target: &Target,
    index: &IndexCache,
    previous: &BTreeMap<PackageId, Package>,
  ) -> Result<(), ResolveError> {
    let Some(entries) = index.get(target.source, target.name) else {
      return Err(ResolveError::Missing([(target.source.to_string(), target.name.to_string())].into()));
    };
    let entry = find_entry(entries, target.to)
      .ok_or_else(|| ResolveError::Failed(format!("{}@{} is not in the index", target.name, target.to)))?;
    let source = previous
      .keys()
      .filter_map(|id| id.source.as_deref())
      .find(|s| same_source(s, target.source))
      .unwrap_or(target.source);
    let new = PackageId { name: target.name.to_string(), version: target.to.clone(), source: Some(source.to_string()) };

    let olds: Vec<PackageId> = self
      .find(target.name, source)
      .filter(|old| **old != new && compatible(&old.version, target.to))
      .cloned()
      .collect();
    for old in olds {
      self.upgrade(&old, &new, &entry.cksum);
    }
    self.candidates.entry(new).or_insert_with(|| Some(entry.cksum.clone()));
    Ok(())
  }

  /// The candidates called `name` from `source`.
  fn find(&self, name: &str, source: &str) -> impl Iterator<Item = &PackageId> {
    self
      .candidates
      .keys()
      .filter(move |id| id.name == name && id.source.as_deref().is_some_and(|s| same_source(s, source)))
  }

  fn upgrade(&mut self, old: &PackageId, new: &PackageId, checksum: &str) {
    self.candidates.remove(old);
    self.candidates.insert(new.clone(), Some(checksum.to_string()));
    self.renamed.insert(old.clone(), new.clone());
  }

  /// Follows `id` through any replacements to the package that holds it now.
  fn current(&self, id: &PackageId) -> PackageId {
    let mut id = id;
    while let Some(next) = self.renamed.get(id) {
      id = next;
    }
    id.clone()
  }
}

/// One resolution of the whole graph, starting from the workspace's packages.
struct Pass<'a> {
  index: &'a IndexCache,
  workspace: &'a Workspace,
  previous: &'a BTreeMap<PackageId, Package>,
  state: &'a mut State,
  /// The features requested of each package so far.
  features: HashMap<PackageId, Features>,
  processed: HashSet<PackageId>,
  edges: HashMap<PackageId, BTreeSet<PackageId>>,
  queue: VecDeque<(PackageId, Features)>,
  missing: BTreeSet<(String, String)>,
  /// Whether a candidate was added or upgraded, which means another pass is needed.
  changed: bool,
}

impl<'a> Pass<'a> {
  fn run(&mut self) -> Result<(), ResolveError> {
    for id in self.previous.keys().filter(|id| id.source.is_none()) {
      self.queue.push_back((id.clone(), Features::new()));
    }
    while let Some((id, features)) = self.queue.pop_front() {
      let id = self.state.current(&id);
      self.process(id, features)?;
    }
    if self.missing.is_empty() { Ok(()) } else { Err(ResolveError::Missing(std::mem::take(&mut self.missing))) }
  }

  fn process(&mut self, id: PackageId, requested: Features) -> Result<(), ResolveError> {
    let (workspace, index) = (self.workspace, self.index);
    match id.source.clone() {
      None => match workspace.package(&id.name) {
        Some(package) => self.process_local(id, package, requested),
        None => self.process_locked(id),
      },
      Some(source) if is_registry(&source) => {
        let Some(entries) = index.get(&source, &id.name) else {
          self.missing.insert((source, id.name.clone()));
          return Ok(());
        };
        match find_entry(entries, &id.version) {
          Some(entry) => self.process_registry(id, &source, entries, entry, requested),
          None => self.process_locked(id),
        }
      }
      Some(_) => self.process_locked(id),
    }
  }

  /// A package from a manifest. Members resolve once with every feature & their dev-dependencies,
  /// other path dependencies resolve again each time more features are requested of them.
  fn process_local(
    &mut self,
    id: PackageId,
    package: &LocalPackage,
    mut requested: Features,
  ) -> Result<(), ResolveError> {
    if package.member {
      requested = package.features.keys().cloned().collect();
      requested.extend(package.dependencies.iter().filter(|d| d.optional).map(|d| format!("dep:{}", d.name)));
    } else if requested.remove(LOCKED_FEATURES) {
      requested.insert("default".to_string());
    }
    let features = self.features.entry(id.clone()).or_default();
    let before = features.len();
    features.extend(requested);
    if !self.processed.insert(id.clone()) && features.len() == before {
      return Ok(());
    }

    let table = package.features.iter().map(|(name, values)| (name.as_str(), values.as_slice())).collect();
    let optional = package.dependencies.iter().filter(|d| d.optional).map(|d| d.name.as_str()).collect();
    let activation = activate_with(&table, &optional, &features.clone());
    let mut edges = BTreeSet::new();
    for dependency in &package.dependencies {
      if (dependency.dev && !package.member)
        || (dependency.optional && !activation.dependencies.contains(&dependency.name))
      {
        continue;
      }
      let selected = match &dependency.source {
        LocalSource::Registry(source) => {
          let requirement = parse_requirement(dependency.req.as_deref().unwrap_or("*"), &id, &dependency.real_name)?;
          let source = self.canonical_source(source);
          self.select(&id, &dependency.real_name, &source, &requirement)?
        }
        LocalSource::Path(_) => Some(match self.workspace.package(&dependency.real_name) {
          Some(local) => PackageId { name: local.name.clone(), version: local.version.clone(), source: None },
          None => self.locked(&id, &dependency.real_name, |source| source.is_none())?,
        }),
        LocalSource::Git(url) => {
          let url = url.trim_end_matches('/').trim_end_matches(".git");
          let git = |source: Option<&str>| source.is_some_and(|s| s.starts_with("git+"));
          let exact =
            self.locked(&id, &dependency.real_name, |source| git(source) && source.is_some_and(|s| s.contains(url)));
          Some(exact.or_else(|_| self.locked(&id, &dependency.real_name, git))?)
        }
      };
      let Some(selected) = selected else { continue };

      let mut features: Features = dependency.features.iter().cloned().collect();
      if dependency.default_features {
        features.insert("default".to_string());
      }
      features.extend(activation.dependency_features.get(&dependency.name).into_iter().flatten().cloned());
      edges.insert(selected.clone());
      self.queue.push_back((selected, features));
    }
    self.edges.insert(id, edges);
    Ok(())
  }

  /// A registry package resolves again each time more features are requested of it.
  fn process_registry(
    &mut self,
    id: PackageId,
    source: &str,
    entries: &[IndexEntry],
    entry: &IndexEntry,
    mut requested: Features,
  ) -> Result<(), ResolveError> {
    if requested.remove(LOCKED_FEATURES) {
      requested.extend(self.locked_features(&id, entries, entry));
    }
    let features = self.features.entry(id.clone()).or_default();
    let before = features.len();
    features.extend(requested);
    if !self.processed.insert(id.clone()) && features.len() == before {
      return Ok(());
    }

    let activation = activate(entry, &features.clone());
    let mut edges = BTreeSet::new();
    for dependency in &entry.deps {
      if dependency.is_dev() || (dependency.optional && !activation.dependencies.contains(&dependency.name)) {
        continue;
      }
      let dependency_source = self.canonical_source(&dependency.source(source));
      let requirement = parse_requirement(&dependency.req, &id, dependency.real_name())?;
      let Some(selected) = self.select(&id, dependency.real_name(), &dependency_source, &requirement)? else {
        continue;
      };

      let mut features: Features = dependency.features.iter().cloned().collect();
      if dependency.default_features {
        features.insert("default".to_string());
      }
      features.extend(activation.dependency_features.get(&dependency.name).into_iter().flatten().cloned());
      edges.insert(selected.clone());
      self.queue.push_back((selected, features));
    }
    self.edges.insert(id, edges);
    Ok(())
  }

  /// A package that can't be resolved (eg: from git) keeps the dependencies it was locked with.
  fn process_locked(&mut self, id: PackageId) -> Result<(), ResolveError> {
    if !self.processed.insert(id.clone()) {
      return Ok(());
    }
    let package = self.previous_package(&id).ok_or_else(|| ResolveError::Failed(format!("{id} can't be resolved")))?;
    let mut edges = BTreeSet::new();
    for dependency in &package.dependencies {
      let dependency = self.state.current(dependency);
      edges.insert(dependency.clone());
      self.queue.push_back((dependency, [LOCKED_FEATURES.to_string()].into()));
    }
    self.edges.insert(id, edges);
    Ok(())
  }

  /// The features a package was evidently locked with: the optional dependencies that were
  /// locked, and `default` unless it would enable an optional dependency that wasn't.
  fn locked_features(&self, id: &PackageId, entries: &[IndexEntry], entry: &IndexEntry) -> Features {
    let Some(previous) = self.previous_package(id) else { return ["default".to_string()].into() };
    let old_entry = find_entry(entries, &previous.id.version).unwrap_or(entry);
    let locked: HashSet<&str> = previous.dependencies.iter().map(|d| d.name.as_str()).collect();
    let enabled: HashSet<&str> =
      old_entry.deps.iter().filter(|d| d.optional && locked.contains(d.real_name())).map(|d| d.name.as_str()).collect();

    let mut features = Features::new();
    let default_needs = activate(old_entry, &["default".to_string()].into()).dependencies;
    if default_needs.iter().all(|d| enabled.contains(d.as_str())) {
      features.insert("default".to_string());
    }
    for dependency in entry.deps.iter().filter(|d| d.optional && enabled.contains(d.name.as_str())) {
      features.insert(format!("dep:{}", dependency.name));
    }
    features
  }

  /// The locked package `id` is, or the one it replaced.
  fn previous_package(&self, id: &PackageId) -> Option<&'a Package> {
    let previous = self.previous;
    previous.get(id).or_else(|| {
      let old = self.state.renamed.keys().find(|old| self.state.current(old) == *id)?;
      previous.get(old)
    })
  }

  /// The locked package called `name` that isn't from a registry, for path, git & patched dependencies.
  fn locked(
    &self,
    parent: &PackageId,
    name: &str,
    source: impl Fn(Option<&str>) -> bool,
  ) -> Result<PackageId, ResolveError> {
    self
      .previous
      .keys()
      .find(|id| id.name == name && source(id.source.as_deref()))
      .cloned()
      .ok_or_else(|| ResolveError::Failed(format!("{parent} depends on {name}, which isn't in Cargo.lock")))
  }

  /// Picks the package that satisfies a dependency of `parent`, adding or upgrading one if needed.
  /// `None` when an index file is missing.
  fn select(
    &mut self,
    parent: &PackageId,
    name: &str,
    source: &str,
    requirement: &VersionReq,
  ) -> Result<Option<PackageId>, ResolveError> {
    let matches = |id: &PackageId| {
      id.name == name
        && id.source.as_deref().is_some_and(|s| same_source(s, source))
        && requirement.matches(&id.version)
    };

    if self.workspace.is_patched(source, name) {
      let patched = self.previous.keys().find(|id| {
        id.name == name && !id.source.as_deref().is_some_and(is_registry) && requirement.matches(&id.version)
      });
      return patched.cloned().map(Some).ok_or_else(|| {
        ResolveError::Failed(format!("{parent} depends on {name} `{requirement}`, but no patched version matches"))
      });
    }

    // Cargo keeps the version the dependent was locked to while it still matches.
    if let Some(package) = self.previous_package(parent) {
      for locked in &package.dependencies {
        let locked = self.state.current(locked);
        if matches(&locked) && self.state.candidates.contains_key(&locked) {
          return Ok(Some(locked));
        }
      }
    }
    if let Some(candidate) = self.state.find(name, source).filter(|id| matches(id)).max() {
      return Ok(Some(candidate.clone()));
    }

    let index = self.index;
    let Some(entries) = index.get(source, name) else {
      self.missing.insert((source.to_string(), name.to_string()));
      return Ok(None);
    };
    let (version, entry) = self.choose(name, source, entries, requirement).ok_or_else(|| {
      ResolveError::Failed(format!("no version of {name} matches `{requirement}`, as required by {parent}"))
    })?;

    let new = PackageId { name: name.to_string(), version, source: Some(source.to_string()) };
    let old = self.state.find(name, source).find(|id| compatible(&id.version, &new.version)).cloned();
    match old {
      Some(old) => {
        self.state.upgrade(&old, &new, &entry.cksum);
        self.processed.remove(&old);
        self.edges.remove(&old);
        if let Some(features) = self.features.remove(&old) {
          self.queue.push_back((new.clone(), features));
        }
      }
      None => {
        self.state.candidates.insert(new.clone(), Some(entry.cksum.clone()));
      }
    }
    self.changed = true;
    Ok(Some(new))
  }

  /// The highest version of a crate that satisfies `requirement`, isn't yanked & doesn't clash on
  /// `links`. Versions supporting the workspace's rust version are preferred.
  fn choose<'e>(
    &self,
    name: &str,
    source: &str,
    entries: &'e [IndexEntry],
    requirement: &VersionReq,
  ) -> Option<(Version, &'e IndexEntry)> {
    let links_in_use: HashMap<&str, &PackageId> =
      self.state.candidates.keys().filter_map(|id| Some((self.entry(id)?.links.as_deref()?, id))).collect();
    let clashes = |entry: &IndexEntry, version: &Version| {
      entry.links.as_deref().and_then(|links| links_in_use.get(links)).is_some_and(|other| {
        !(other.name == name
          && other.source.as_deref().is_some_and(|s| same_source(s, source))
          && compatible(&other.version, version))
      })
    };

    let versions: Vec<(Version, &IndexEntry)> = entries
      .iter()
      .filter(|e| !e.yanked)
      .filter_map(|e| Version::parse(&e.vers).ok().map(|v| (v, e)))
      .filter(|(v, e)| requirement.matches(v) && !clashes(e, v))
      .collect();
    let supported = |entry: &IndexEntry| match (&self.workspace.msrv, entry.rust_version.as_deref()) {
      (Some(msrv), Some(rust_version)) => parse_rust_version(rust_version).is_none_or(|v| v <= *msrv),
      _ => true,
    };
    let highest = |supported_only: bool| {
      versions.iter().filter(|(_, e)| !supported_only || supported(e)).max_by(|a, b| a.0.cmp(&b.0)).cloned()
    };
    highest(true).or_else(|| highest(false))
  }

  /// The index entry of a registry package, if its index file has been fetched.
  fn entry(&self, id: &PackageId) -> Option<&'a IndexEntry> {
    let index = self.index;
    find_entry(index.get(id.source.as_deref()?, &id.name)?, &id.version)
  }

  /// `source` as already written in the lockfile, if it is there, so a trailing slash is consistent.
  fn canonical_source(&self, source: &str) -> String {
    self
      .previous
      .keys()
      .chain(self.state.candidates.keys())
      .filter_map(|id| id.source.as_deref())
      .find(|s| same_source(s, source))
      .unwrap_or(source)
      .to_string()
  }

  /// Builds the lockfile from every package the workspace reaches.
  fn finish(self, lock: &Lockfile) -> Result<Lockfile, ResolveError> {
    let mut stack: Vec<PackageId> = self.previous.keys().filter(|id| id.source.is_none()).cloned().collect();
    let mut reachable = BTreeSet::new();
    while let Some(id) = stack.pop() {
      let id = self.state.current(&id);
      if reachable.insert(id.clone()) {
        stack.extend(self.edges.get(&id).into_iter().flatten().cloned());
      }
    }

    let mut links: HashMap<&str, &PackageId> = HashMap::new();
    for id in &reachable {
      if let Some(native) = self.entry(id).and_then(|e| e.links.as_deref())
        && let Some(other) = links.insert(native, id)
      {
        return Err(ResolveError::Failed(format!("{other} and {id} both link to the native library `{native}`")));
      }
    }

    let packages = reachable
      .iter()
      .map(|id| {
        let previous = self.previous.get(id);
        let mut dependencies: Vec<PackageId> =
          self.edges.get(id).into_iter().flatten().map(|d| self.state.current(d)).collect();
        dependencies.sort();
        dependencies.dedup();
        Package {
          id: id.clone(),
          checksum: self
            .state
            .candidates
            .get(id)
            .cloned()
            .flatten()
            .or_else(|| previous.and_then(|p| p.checksum.clone())),
          dependencies,
          replace: previous.and_then(|p| p.replace.clone()),
        }
      })
      .collect();
    Ok(lock.with_packages(packages))
  }
}

fn parse_requirement(requirement: &str, parent: &PackageId, name: &str) -> Result<VersionReq, ResolveError> {
  VersionReq::parse(requirement).map_err(|err| {
    ResolveError::Failed(format!("{parent} has an invalid requirement `{requirement}` on {name}: {err}"))
  })
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
///
/// Like cargo's dependency resolver, which writes the lockfile, a weak `name?/feature` enables
/// the optional dependency too. Only the feature resolver used when building honours the `?`.
fn activate(entry: &IndexEntry, requested: &Features) -> Activation {
  let optional = entry.deps.iter().filter(|d| d.optional).map(|d| d.name.as_str()).collect();
  activate_with(&entry.all_features(), &optional, requested)
}

/// [`activate`] given a feature table & the names of the optional dependencies.
fn activate_with(table: &BTreeMap<&str, &[String]>, optional: &HashSet<&str>, requested: &Features) -> Activation {
  let explicit: HashSet<&str> = table.values().flat_map(|v| v.iter()).filter_map(|v| v.strip_prefix("dep:")).collect();
  let implicit = |name: &str| optional.contains(name) && !explicit.contains(name);

  let mut activation = Activation::default();
  let mut seen = HashSet::new();
  let mut stack: Vec<String> = requested.iter().cloned().collect();
  while let Some(feature) = stack.pop() {
    if !seen.insert(feature.clone()) {
      continue;
    }
    if let Some(name) = feature.strip_prefix("dep:") {
      activation.dependencies.insert(name.to_string());
    } else if let Some((name, dependency_feature)) = feature.split_once('/') {
      let (name, weak) = match name.strip_suffix('?') {
        Some(name) => (name, true),
        None => (name, false),
      };
      if optional.contains(name) {
        activation.dependencies.insert(name.to_string());
        if !weak && implicit(name) {
          stack.push(name.to_string());
        }
      }
      activation.dependency_features.entry(name.to_string()).or_default().insert(dependency_feature.to_string());
    } else if let Some(values) = table.get(feature.as_str()) {
      stack.extend(values.iter().cloned());
    } else if implicit(&feature) {
      activation.dependencies.insert(feature);
    }
  }
  activation
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::index::CRATES_IO_SOURCE;
  use crate::workspace::LocalDependency;
  use serde_json::{Value, json};

  const HEADER: &str =
    "# This file is automatically @generated by Cargo.\n# It is not intended for manual editing.\nversion = 4\n";
  const REGISTRY: Option<&str> = Some(CRATES_IO_SOURCE);
  const GIT: Option<&str> = Some("git+https://example.com/repo#abc");

  /// Builds a lockfile from `(name, version, source, dependencies)`.
  fn lock(packages: &[(&str, &str, Option<&str>, &[&str])]) -> Lockfile {
    let mut text = HEADER.to_string();
    for (name, version, source, dependencies) in packages {
      text.push_str(&format!("\n[[package]]\nname = \"{name}\"\nversion = \"{version}\"\n"));
      if let Some(source) = source {
        text.push_str(&format!("source = \"{source}\"\n"));
        if is_registry(source) {
          text.push_str(&format!("checksum = \"{name}-{version}\"\n"));
        }
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

  /// A workspace with the single member `app`.
  fn workspace(dependencies: Vec<LocalDependency>) -> Workspace {
    let app = LocalPackage {
      name: "app".into(),
      version: Version::new(0, 1, 0),
      member: true,
      dependencies,
      features: BTreeMap::new(),
    };
    Workspace { packages: vec![app], ..Workspace::default() }
  }

  fn needs(name: &str, req: &str, features: &[&str]) -> LocalDependency {
    LocalDependency {
      name: name.into(),
      real_name: name.into(),
      source: LocalSource::Registry(CRATES_IO_SOURCE.into()),
      req: Some(req.into()),
      features: features.iter().map(|f| f.to_string()).collect(),
      default_features: true,
      optional: false,
      dev: false,
    }
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

  fn plain(name: &str, vers: &str) -> String {
    version(name, vers, vec![], json!({}))
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
    workspace: &Workspace,
    index: &IndexCache,
    target: Option<(&str, &str)>,
  ) -> Result<Lockfile, ResolveError> {
    let to = target.map(|(_, to)| Version::parse(to).unwrap());
    let target = target.zip(to.as_ref()).map(|((name, _), to)| Target { name, source: CRATES_IO_SOURCE, to });
    update(lock, workspace, target.as_ref(), index)
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

  fn missing(names: &[&str]) -> ResolveError {
    ResolveError::Missing(names.iter().map(|n| (CRATES_IO_SOURCE.to_string(), n.to_string())).collect())
  }

  #[test]
  fn missing_index_files_are_requested() {
    let lock = lock(&[("app", "0.1.0", None, &["foo"]), ("foo", "1.0.0", REGISTRY, &[])]);
    let workspace = workspace(vec![needs("foo", "1", &[])]);
    assert_eq!(run(&lock, &workspace, &IndexCache::default(), None).unwrap_err(), missing(&["foo"]));
    assert_eq!(run(&lock, &workspace, &IndexCache::default(), Some(("foo", "1.1.0"))).unwrap_err(), missing(&["foo"]));

    let index = index(&[("foo", vec![version("foo", "1.1.0", vec![dep("bar", "^1")], json!({}))])]);
    assert_eq!(run(&lock, &workspace, &index, Some(("foo", "1.1.0"))).unwrap_err(), missing(&["bar"]));
  }

  #[test]
  fn an_up_to_date_lockfile_is_reproduced_exactly() {
    let lock = lock(&[
      ("app", "0.1.0", None, &["foo"]),
      ("foo", "1.0.0", REGISTRY, &["rand 0.7.3", "serde"]),
      ("rand", "0.7.3", REGISTRY, &[]),
      ("rand", "0.8.5", REGISTRY, &[]),
      ("serde", "1.0.0", REGISTRY, &["rand 0.8.5"]),
    ]);
    let workspace = workspace(vec![needs("foo", "1", &["serde"])]);
    let index = index(&[
      ("foo", vec![version("foo", "1.0.0", vec![dep("rand", ">=0.7, <0.9"), optional("serde", "^1")], json!({}))]),
      ("serde", vec![version("serde", "1.0.0", vec![dep("rand", "^0.8")], json!({}))]),
      ("rand", vec![plain("rand", "0.7.3"), plain("rand", "0.8.5")]),
    ]);
    assert_eq!(run(&lock, &workspace, &index, None).unwrap().to_string(), lock.to_string());
  }

  #[test]
  fn compatible_update_replaces_everywhere_and_resolves_new_dependencies() {
    let lock = lock(&[
      ("app", "0.1.0", None, &["foo", "other"]),
      ("foo", "1.0.0", REGISTRY, &["old-dep", "shared"]),
      ("old-dep", "1.0.0", REGISTRY, &[]),
      ("other", "1.0.0", REGISTRY, &["foo"]),
      ("shared", "1.0.0", REGISTRY, &[]),
    ]);
    let workspace = workspace(vec![needs("foo", "1", &[]), needs("other", "1", &[])]);
    let test_only = json!({ "name": "test-only", "req": "^1", "kind": "dev" });
    let index = index(&[
      ("foo", vec![version("foo", "1.1.0", vec![dep("shared", "^1.2"), dep("new-dep", "^2"), test_only], json!({}))]),
      ("other", vec![version("other", "1.0.0", vec![dep("foo", "^1")], json!({}))]),
      ("shared", vec![plain("shared", "1.0.0"), plain("shared", "1.3.0"), plain("shared", "2.0.0")]),
      ("new-dep", vec![plain("new-dep", "2.0.0"), plain("new-dep", "2.1.0")]),
    ]);

    let updated = run(&lock, &workspace, &index, Some(("foo", "1.1.0"))).unwrap();
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
  fn breaking_update_only_moves_dependents_whose_requirement_moved() {
    let lock = lock(&[
      ("app", "0.1.0", None, &["foo", "other"]),
      ("foo", "1.0.0", REGISTRY, &[]),
      ("other", "1.0.0", REGISTRY, &["foo"]),
    ]);
    // The manifest has already been bumped to the new major version.
    let workspace = workspace(vec![needs("foo", "2.0", &[]), needs("other", "1", &[])]);
    let index = index(&[
      ("foo", vec![plain("foo", "1.0.0"), plain("foo", "2.0.0")]),
      ("other", vec![version("other", "1.0.0", vec![dep("foo", "^1")], json!({}))]),
    ]);

    let updated = run(&lock, &workspace, &index, Some(("foo", "2.0.0"))).unwrap();
    assert_eq!(
      summary(&updated),
      ["app 0.1.0 -> [foo 2.0.0, other 1.0.0]", "foo 1.0.0 -> []", "foo 2.0.0 -> []", "other 1.0.0 -> [foo 1.0.0]"]
    );
    assert!(updated.to_string().contains(" \"foo 2.0.0\",\n"), "{updated}");
  }

  #[test]
  fn features_are_unified_across_every_dependent() {
    let lock = lock(&[
      ("app", "0.1.0", None, &["bar", "foo"]),
      ("bar", "1.0.0", REGISTRY, &["foo"]),
      ("foo", "1.0.0", REGISTRY, &["serde"]),
      ("serde", "1.0.0", REGISTRY, &[]),
    ]);
    let workspace = workspace(vec![needs("foo", "1", &["serde"]), needs("bar", "1", &[])]);
    let foo_features = json!({ "default": ["std"], "std": [], "extra": ["dep:extra", "serde?/derive"] });
    let wants_extra = json!({ "name": "foo", "req": "^1", "features": ["extra"], "kind": "normal" });
    let index = index(&[
      (
        "foo",
        vec![
          version("foo", "1.0.0", vec![optional("serde", "^1"), optional("unused", "^1")], json!({})),
          version(
            "foo",
            "1.1.0",
            vec![optional("serde", "^1"), optional("unused", "^1"), optional("extra", "^1")],
            foo_features,
          ),
        ],
      ),
      ("bar", vec![version("bar", "1.0.0", vec![wants_extra], json!({}))]),
      ("serde", vec![version("serde", "1.0.0", vec![optional("md", "^1")], json!({ "derive": ["dep:md"] }))]),
      ("extra", vec![plain("extra", "1.0.0")]),
      ("md", vec![plain("md", "1.0.0")]),
    ]);

    let updated = run(&lock, &workspace, &index, Some(("foo", "1.1.0"))).unwrap();
    assert_eq!(
      summary(&updated),
      [
        "app 0.1.0 -> [bar 1.0.0, foo 1.1.0]",
        "bar 1.0.0 -> [foo 1.1.0]",
        "extra 1.0.0 -> []",
        "foo 1.1.0 -> [extra 1.0.0, serde 1.0.0]",
        "md 1.0.0 -> []",
        "serde 1.0.0 -> [md 1.0.0]",
      ]
    );
  }

  #[test]
  fn optional_dependencies_no_longer_requested_are_dropped() {
    let lock = lock(&[
      ("app", "0.1.0", None, &["foo"]),
      ("bar", "1.0.0", REGISTRY, &["simd"]),
      ("foo", "1.0.0", REGISTRY, &["bar"]),
      ("simd", "1.0.0", REGISTRY, &[]),
    ]);
    let workspace = workspace(vec![needs("foo", "1", &[])]);
    let fast_bar = json!({ "name": "bar", "req": "^1", "features": ["fast"], "kind": "normal" });
    let index = index(&[
      (
        "foo",
        vec![
          version("foo", "1.0.0", vec![fast_bar], json!({})),
          version("foo", "1.1.0", vec![dep("bar", "^1")], json!({})),
        ],
      ),
      ("bar", vec![version("bar", "1.0.0", vec![optional("simd", "^1")], json!({ "fast": ["simd"] }))]),
      ("simd", vec![plain("simd", "1.0.0")]),
    ]);

    assert_eq!(run(&lock, &workspace, &index, None).unwrap().to_string(), lock.to_string());
    let updated = run(&lock, &workspace, &index, Some(("foo", "1.1.0"))).unwrap();
    assert_eq!(summary(&updated), ["app 0.1.0 -> [foo 1.1.0]", "bar 1.0.0 -> []", "foo 1.1.0 -> [bar 1.0.0]"]);
  }

  #[test]
  fn new_versions_prefer_the_workspace_rust_version() {
    let lock = lock(&[("app", "0.1.0", None, &["foo"]), ("foo", "1.0.0", REGISTRY, &[])]);
    let mut workspace = workspace(vec![needs("foo", "1", &[])]);
    workspace.msrv = Some(Version::new(1, 70, 0));
    let with_msrv = |name: &str, vers: &str, rust_version: &str| {
      json!({ "name": name, "vers": vers, "deps": [], "cksum": "x", "rust_version": rust_version }).to_string()
    };
    let foo = version("foo", "1.1.0", vec![dep("bar", "^1")], json!({}));
    let index =
      index(&[("foo", vec![foo]), ("bar", vec![with_msrv("bar", "1.0.0", "1.60"), with_msrv("bar", "1.5.0", "1.90")])]);
    let updated = run(&lock, &workspace, &index, Some(("foo", "1.1.0"))).unwrap();
    assert!(summary(&updated).contains(&"bar 1.0.0 -> []".to_string()), "{updated}");

    workspace.msrv = Some(Version::new(1, 50, 0));
    let updated = run(&lock, &workspace, &index, Some(("foo", "1.1.0"))).unwrap();
    assert!(summary(&updated).contains(&"bar 1.5.0 -> []".to_string()), "{updated}");
  }

  #[test]
  fn new_versions_avoid_links_clashes() {
    let lock = lock(&[
      ("app", "0.1.0", None, &["foo", "other-sys"]),
      ("foo", "1.0.0", REGISTRY, &[]),
      ("other-sys", "1.0.0", REGISTRY, &[]),
    ]);
    let workspace = workspace(vec![needs("foo", "1", &[]), needs("other-sys", "1", &[])]);
    let linking = |name: &str, vers: &str| {
      json!({ "name": name, "vers": vers, "deps": [], "cksum": "x", "links": "z" }).to_string()
    };
    let index = index(&[
      ("foo", vec![version("foo", "1.1.0", vec![dep("sys", ">=1")], json!({}))]),
      ("other-sys", vec![linking("other-sys", "1.0.0")]),
      ("sys", vec![plain("sys", "1.0.0"), linking("sys", "2.0.0")]),
    ]);
    let updated = run(&lock, &workspace, &index, Some(("foo", "1.1.0"))).unwrap();
    assert!(summary(&updated).contains(&"sys 1.0.0 -> []".to_string()), "{updated}");
  }

  #[test]
  fn git_and_patched_packages_keep_their_locked_dependencies() {
    let lock = lock(&[
      ("app", "0.1.0", None, &["from-git", "patched"]),
      ("from-git", "0.1.0", GIT, &["serde"]),
      ("patched", "1.0.0", GIT, &[]),
      ("serde", "1.0.0", REGISTRY, &["derive"]),
      ("derive", "1.0.0", REGISTRY, &[]),
    ]);
    let mut workspace = workspace(vec![
      LocalDependency {
        source: LocalSource::Git("https://example.com/repo".into()),
        req: None,
        ..needs("from-git", "*", &[])
      },
      needs("patched", "1", &[]),
    ]);
    workspace.patches.insert((CRATES_IO_SOURCE.into(), "patched".into()));
    let index = index(&[
      ("serde", vec![version("serde", "1.0.0", vec![optional("derive", "^1")], json!({}))]),
      ("derive", vec![plain("derive", "1.0.0")]),
    ]);
    assert_eq!(run(&lock, &workspace, &index, None).unwrap().to_string(), lock.to_string());
  }

  #[test]
  fn path_dependencies_outside_the_workspace_resolve_from_their_manifest() {
    // `lib` has moved on since it was locked: its version & requirements changed.
    let lock =
      lock(&[("app", "0.1.0", None, &["lib"]), ("lib", "0.4.16", None, &["regex"]), ("regex", "1.0.0", REGISTRY, &[])]);
    let mut workspace = workspace(vec![LocalDependency { source: LocalSource::Path(None), ..needs("lib", "*", &[]) }]);
    workspace.packages.push(LocalPackage {
      name: "lib".into(),
      version: Version::new(0, 4, 22),
      member: false,
      dependencies: vec![
        needs("regex", "1.5", &[]),
        LocalDependency { optional: true, ..needs("memchr", "2", &[]) },
        LocalDependency { dev: true, ..needs("criterion", "1", &[]) },
      ],
      features: BTreeMap::new(),
    });
    let index = index(&[("regex", vec![plain("regex", "1.0.0"), plain("regex", "1.5.0"), plain("regex", "1.6.0")])]);

    let resolved = run(&lock, &workspace, &index, None).unwrap();
    assert_eq!(summary(&resolved), ["app 0.1.0 -> [lib 0.4.22]", "lib 0.4.22 -> [regex 1.6.0]", "regex 1.6.0 -> []"]);
  }

  #[test]
  fn weak_features_enable_optional_dependencies_in_the_lockfile() {
    let entry: IndexEntry = serde_json::from_str(&version(
      "winnow",
      "1.0.0",
      vec![optional("memchr", "^2")],
      json!({ "default": ["std"], "std": ["memchr?/std"] }),
    ))
    .unwrap();
    let activation = activate(&entry, &["default".to_string()].into());
    assert!(activation.dependencies.contains("memchr"));
    assert_eq!(activation.dependency_features["memchr"], ["std".to_string()].into());
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
