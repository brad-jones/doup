//! Picks the versions to offer from a crate's sparse index entry.
//!
//! This module only uses plain `std` / `serde` types (plus the WIT records) so that it can be
//! unit tested natively, without a wasm runtime.

use crate::{Dependency, Occurrence, Versions};
use semver::{Version, VersionReq};
use serde::Deserialize;

#[derive(Deserialize)]
struct IndexEntry {
  vers: String,
  #[serde(default)]
  yanked: bool,
}

/// What is installed and what could be installed for a dependency, derived from its crate's index entry.
pub struct Analysis {
  /// The lowest locked version, or the lower bound of the requirement when there's no lock, as an
  /// occurrence that lags behind needs updating even if others are current. None without occurrences.
  pub installed: Option<Version>,
  /// The requirement of every occurrence.
  pub requirements: Vec<VersionReq>,
  /// Every version that may be offered: yanked versions are never included, and pre-releases
  /// only when the dependency already uses one.
  pub candidates: Vec<Version>,
}

/// Parses the declared version requirement of every occurrence of the dependency called `name`.
pub fn requirements(name: &str, occurrences: &[Occurrence]) -> Result<Vec<VersionReq>, String> {
  occurrences
    .iter()
    .map(|o| {
      VersionReq::parse(&o.current_version)
        .map_err(|err| format!("invalid version requirement `{}` for {name}: {err}", o.current_version))
    })
    .collect()
}

/// The lowest version installed across `occurrences`, whose `requirements` come from [`requirements`].
///
/// An occurrence's installed version is its locked version, or the lower bound of its requirement without a lock.
pub fn installed(occurrences: &[Occurrence], requirements: &[VersionReq]) -> Result<Option<Version>, String> {
  Ok(
    occurrences
      .iter()
      .zip(requirements)
      .map(|(o, requirement)| match &o.locked_version {
        Some(locked) => Version::parse(locked).map_err(|err| format!("invalid locked version `{locked}`: {err}")),
        None => Ok(lower_bound(requirement)),
      })
      .collect::<Result<Vec<_>, _>>()?
      .into_iter()
      .min(),
  )
}

/// Parses the newline delimited JSON `index` entry of the crate behind `dependency`.
pub fn analyse(dependency: &Dependency, index: &str) -> Result<Analysis, String> {
  let requirements = requirements(&dependency.name, &dependency.occurrences)?;
  let installed = installed(&dependency.occurrences, &requirements)?;

  let allow_prerelease = installed.iter().any(|v| !v.pre.is_empty())
    || requirements.iter().flat_map(|r| &r.comparators).any(|c| !c.pre.is_empty());

  let mut candidates = Vec::new();
  for line in index.lines().filter(|line| !line.trim().is_empty()) {
    let entry: IndexEntry =
      serde_json::from_str(line).map_err(|err| format!("invalid index entry for {}: {err}", dependency.name))?;
    if entry.yanked {
      continue;
    }
    match Version::parse(&entry.vers) {
      Ok(version) if allow_prerelease || version.pre.is_empty() => candidates.push(version),
      _ => {}
    }
  }

  Ok(Analysis { installed, requirements, candidates })
}

/// Chooses the versions to offer for `dependency` from the newline delimited JSON `index` entry of its crate.
///
/// - `next` is the highest candidate satisfying the requirement of every occurrence.
/// - `latest` is the highest candidate overall, if it is newer than `next`.
///
/// Returns `None` when nothing newer than the installed version exists.
pub fn select(dependency: &Dependency, index: &str) -> Result<Option<Versions>, String> {
  let Analysis { installed, requirements, candidates } = analyse(dependency, index)?;

  let next = candidates.iter().filter(|v| requirements.iter().all(|r| r.matches(v))).max();
  let (Some(next), Some(installed)) = (next, installed) else {
    return Ok(None);
  };
  let latest = candidates.iter().max().filter(|latest| *latest > next);

  if latest.unwrap_or(next) <= &installed {
    return Ok(None);
  }
  Ok(Some(Versions { next: next.to_string(), latest: latest.map(Version::to_string) }))
}

/// The oldest version a requirement can mean, eg: `1.2` is `1.2.0`.
fn lower_bound(requirement: &VersionReq) -> Version {
  requirement
    .comparators
    .first()
    .map_or(Version::new(0, 0, 0), |c| Version::new(c.major, c.minor.unwrap_or(0), c.patch.unwrap_or(0)))
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::Occurrence;

  fn occurrence(current: &str, locked: Option<&str>) -> Occurrence {
    Occurrence {
      manifest_path: "Cargo.toml".to_string(),
      lock_path: locked.map(|_| "Cargo.lock".to_string()),
      current_version: current.to_string(),
      locked_version: locked.map(str::to_string),
    }
  }

  fn dependency(occurrences: Vec<Occurrence>) -> Dependency {
    Dependency { name: "foo".to_string(), registry: None, current_version: String::new(), occurrences }
  }

  fn index(versions: &[(&str, bool)]) -> String {
    versions.iter().map(|(v, yanked)| format!("{{\"vers\":\"{v}\",\"yanked\":{yanked}}}\n")).collect()
  }

  fn run(dependency: &Dependency, versions: &[(&str, bool)]) -> Option<(String, Option<String>)> {
    select(dependency, &index(versions)).unwrap().map(|v| (v.next, v.latest))
  }

  #[test]
  fn next_respects_the_requirement_and_latest_ignores_it() {
    let dep = dependency(vec![occurrence("1.2", Some("1.2.3"))]);
    let found = run(&dep, &[("1.2.3", false), ("1.9.0", false), ("2.0.0", false), ("2.1.0", false)]);
    assert_eq!(found, Some(("1.9.0".to_string(), Some("2.1.0".to_string()))));
  }

  #[test]
  fn up_to_date_is_none() {
    let dep = dependency(vec![occurrence("1", Some("1.9.0"))]);
    assert_eq!(run(&dep, &[("1.8.0", false), ("1.9.0", false)]), None);
  }

  #[test]
  fn only_a_breaking_release_is_offered_as_latest() {
    let dep = dependency(vec![occurrence("1", Some("1.9.0"))]);
    let found = run(&dep, &[("1.9.0", false), ("2.0.0", false)]);
    assert_eq!(found, Some(("1.9.0".to_string(), Some("2.0.0".to_string()))));
  }

  #[test]
  fn yanked_and_prereleases_are_skipped() {
    let dep = dependency(vec![occurrence("1", Some("1.0.0"))]);
    let found = run(&dep, &[("1.0.0", false), ("1.1.0", true), ("1.2.0-beta.1", false), ("1.0.1", false)]);
    assert_eq!(found, Some(("1.0.1".to_string(), None)));
  }

  #[test]
  fn prereleases_are_offered_when_already_in_use() {
    let dep = dependency(vec![occurrence("1.0.0-beta.1", Some("1.0.0-beta.1"))]);
    let found = run(&dep, &[("1.0.0-beta.1", false), ("1.0.0-beta.2", false)]);
    assert_eq!(found, Some(("1.0.0-beta.2".to_string(), None)));
  }

  #[test]
  fn next_satisfies_every_occurrence() {
    let dep = dependency(vec![occurrence("1", Some("1.0.0")), occurrence("<1.5", Some("1.0.0"))]);
    let found = run(&dep, &[("1.0.0", false), ("1.4.0", false), ("1.6.0", false)]);
    assert_eq!(found, Some(("1.4.0".to_string(), Some("1.6.0".to_string()))));
  }

  #[test]
  fn a_lagging_occurrence_still_needs_updating() {
    let dep = dependency(vec![occurrence("1", Some("1.9.0")), occurrence("1", Some("1.2.0"))]);
    let found = run(&dep, &[("1.2.0", false), ("1.9.0", false)]);
    assert_eq!(found, Some(("1.9.0".to_string(), None)));
  }

  #[test]
  fn without_a_lockfile_the_requirement_lower_bound_is_installed() {
    let behind = dependency(vec![occurrence("1.0", None)]);
    assert_eq!(run(&behind, &[("1.0.0", false), ("1.0.5", false)]), Some(("1.0.5".to_string(), None)));

    let exact = dependency(vec![occurrence("=1.0.5", None)]);
    assert_eq!(run(&exact, &[("1.0.0", false), ("1.0.5", false)]), None);
  }

  #[test]
  fn invalid_requirement_is_an_error() {
    let dep = dependency(vec![occurrence("not-a-version", None)]);
    assert!(select(&dep, &index(&[("1.0.0", false)])).unwrap_err().contains("not-a-version"));
  }

  #[test]
  fn invalid_index_is_an_error() {
    let dep = dependency(vec![occurrence("1", None)]);
    assert!(select(&dep, "not json").is_err());
  }
}
