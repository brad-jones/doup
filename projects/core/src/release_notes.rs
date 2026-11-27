//! Picks & orders the release notes shown in a pull request.

use doup_plugin_host::ecosystem_plugin::ReleaseNote;
use semver::Version;
use std::cmp::Ordering;

/// Parses `version` as semver, leniently: a leading `v` is ignored & missing minor or patch numbers
/// are taken as `0`, eg: `v1.2` is `1.2.0`. Anything else that isn't semver gives `None`.
pub(crate) fn parse_version(version: &str) -> Option<Version> {
  let version = version.trim();
  let version = version.strip_prefix(['v', 'V']).unwrap_or(version);
  if let Ok(parsed) = Version::parse(version) {
    return Some(parsed);
  }

  let split = version.find(['-', '+']).unwrap_or(version.len());
  let (core, rest) = version.split_at(split);
  let parts: Vec<&str> = core.split('.').collect();
  let numeric = parts.iter().all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()));
  if !numeric || parts.len() >= 3 {
    return None;
  }
  let padding = ".0".repeat(3 - parts.len());
  Version::parse(&format!("{core}{padding}{rest}")).ok()
}

/// The notes worth showing for an update to `target`, newest first.
///
/// Notes for versions newer than `target` are dropped, which matters as notes are fetched once per
/// dependency, up to its newest target, then shared by its `next` & `latest` pull requests. Notes whose
/// version (or `target`) can't be parsed are kept, after the rest, in the order they were given.
pub(crate) fn for_target(notes: &[ReleaseNote], target: &str) -> Vec<ReleaseNote> {
  let target = parse_version(target);
  let mut notes: Vec<(Option<Version>, &ReleaseNote)> = notes
    .iter()
    .map(|note| (parse_version(&note.version), note))
    .filter(|(version, _)| match (version, &target) {
      (Some(version), Some(target)) => version <= target,
      _ => true,
    })
    .collect();
  notes.sort_by(|(a, _), (b, _)| match (a, b) {
    (Some(a), Some(b)) => b.cmp(a),
    (Some(_), None) => Ordering::Less,
    (None, Some(_)) => Ordering::Greater,
    (None, None) => Ordering::Equal,
  });
  notes.into_iter().map(|(_, note)| note.clone()).collect()
}

#[cfg(test)]
mod tests {
  use super::*;

  fn note(version: &str) -> ReleaseNote {
    ReleaseNote { version: version.to_string(), url: String::new(), body: String::new() }
  }

  fn versions(notes: &[ReleaseNote]) -> Vec<&str> {
    notes.iter().map(|note| note.version.as_str()).collect()
  }

  #[test]
  fn parses_versions_leniently() {
    assert_eq!(parse_version("1.2.3"), Some(Version::new(1, 2, 3)));
    assert_eq!(parse_version(" v1.2.3 "), Some(Version::new(1, 2, 3)));
    assert_eq!(parse_version("V2"), Some(Version::new(2, 0, 0)));
    assert_eq!(parse_version("1.2"), Some(Version::new(1, 2, 0)));
    assert_eq!(parse_version("1.2-beta.1"), Version::parse("1.2.0-beta.1").ok());
    for invalid in ["", "latest", "1.x", "1..2", "1.2.3.4"] {
      assert_eq!(parse_version(invalid), None, "{invalid}");
    }
  }

  #[test]
  fn drops_notes_newer_than_the_target_and_orders_newest_first() {
    let notes = [note("1.1.0"), note("2.0.0"), note("v1.2.0"), note("1.2.0-rc.1"), note("1.0.1")];
    assert_eq!(versions(&for_target(&notes, "1.2.0")), ["v1.2.0", "1.2.0-rc.1", "1.1.0", "1.0.1"]);
    assert_eq!(versions(&for_target(&notes, "2.0.0")), ["2.0.0", "v1.2.0", "1.2.0-rc.1", "1.1.0", "1.0.1"]);
  }

  #[test]
  fn keeps_notes_that_can_not_be_compared() {
    let notes = [note("nightly"), note("1.1.0"), note("stable"), note("3.0.0")];
    assert_eq!(versions(&for_target(&notes, "2.0.0")), ["1.1.0", "nightly", "stable"]);
    assert_eq!(versions(&for_target(&notes, "main")), ["3.0.0", "1.1.0", "nightly", "stable"]);
  }
}
