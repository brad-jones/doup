//! Rewrites a declared version requirement so that it points at a new version.
//!
//! This module only uses plain `std` / `semver` types so that it can be unit tested natively.

use semver::{Op, Version, VersionReq};

/// Bumps the requirement `original` to `to`, keeping its style.
///
/// - A single `^`, `~`, `=` or bare comparator keeps its operator and precision, eg: with `to` of
///   `1.9.4`, `"1.0.100"` becomes `"1.9.4"`, `"^1.2"` becomes `"^1.9"` and `"1"` stays `"1"`.
///   A pre-release `to` is always written in full.
/// - Wildcards keep their shape, eg: `"1.*"` becomes `"2.*"`, and `"*"` never changes.
/// - Anything else (eg: `">=1, <2"`) is left alone if it already allows `to`, otherwise it is
///   replaced with `to` as a bare (caret) requirement.
pub fn bump(original: &str, to: &Version) -> String {
  let Ok(requirement) = VersionReq::parse(original) else {
    return format_version(to, 3);
  };

  let bumped = match requirement.comparators.as_slice() {
    [] => return original.to_string(),
    [single] => match single.op {
      Op::Caret | Op::Tilde | Op::Exact => {
        let precision = if to.pre.is_empty() { precision(single) } else { 3 };
        Some(format!("{}{}", operator_prefix(original), format_version(to, precision)))
      }
      Op::Wildcard => Some(match single.minor {
        Some(_) => format!("{}{}.{}.*", operator_prefix(original), to.major, to.minor),
        None => format!("{}{}.*", operator_prefix(original), to.major),
      }),
      _ => None,
    },
    _ => None,
  };

  match bumped {
    Some(bumped) if VersionReq::parse(&bumped).is_ok_and(|r| r.matches(to)) => bumped,
    _ if requirement.matches(to) => original.to_string(),
    _ => format_version(to, 3),
  }
}

/// How many of major, minor & patch the comparator spells out.
fn precision(comparator: &semver::Comparator) -> usize {
  match (comparator.minor, comparator.patch) {
    (None, _) => 1,
    (Some(_), None) => 2,
    (Some(_), Some(_)) => 3,
  }
}

/// Everything before the version number, eg: `"^"`, `"= "` or `""`.
fn operator_prefix(original: &str) -> &str {
  let trimmed = original.trim_start();
  let end = trimmed.find(|c: char| c.is_ascii_digit() || c == '*').unwrap_or(0);
  &trimmed[..end]
}

/// Writes `version` to `precision` components. Build metadata is never written, as cargo ignores
/// it in requirements & warns about it.
fn format_version(version: &Version, precision: usize) -> String {
  match precision {
    1 => version.major.to_string(),
    2 => format!("{}.{}", version.major, version.minor),
    _ => Version { build: semver::BuildMetadata::EMPTY, ..version.clone() }.to_string(),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn bumped(original: &str, to: &str) -> String {
    bump(original, &Version::parse(to).unwrap())
  }

  #[test]
  fn keeps_operator_and_precision() {
    assert_eq!(bumped("1.0.100", "1.0.228"), "1.0.228");
    assert_eq!(bumped("1.0", "1.0.228"), "1.0");
    assert_eq!(bumped("1", "1.9.0"), "1");
    assert_eq!(bumped("^1.2", "1.9.4"), "^1.9");
    assert_eq!(bumped("~0.3.1", "0.3.7"), "~0.3.7");
    assert_eq!(bumped("=1.2.3", "2.0.0"), "=2.0.0");
    assert_eq!(bumped("= 1.2.3", "1.2.4"), "= 1.2.4");
    assert_eq!(bumped("1.2.2", "1.2.3+spec-1.1.0"), "1.2.3");
  }

  #[test]
  fn breaking_bumps_raise_the_requirement() {
    assert_eq!(bumped("1.0", "2.1.0"), "2.1");
    assert_eq!(bumped("0.3", "0.4.1"), "0.4");
    assert_eq!(bumped("1", "2.0.0"), "2");
  }

  #[test]
  fn prereleases_are_written_in_full() {
    assert_eq!(bumped("1.0.0-beta.1", "1.0.0-beta.2"), "1.0.0-beta.2");
    assert_eq!(bumped("^1", "2.0.0-rc.1"), "^2.0.0-rc.1");
  }

  #[test]
  fn wildcards_keep_their_shape() {
    assert_eq!(bumped("*", "9.9.9"), "*");
    assert_eq!(bumped("1.*", "2.3.4"), "2.*");
    assert_eq!(bumped("1.2.*", "1.3.0"), "1.3.*");
  }

  #[test]
  fn ranges_are_kept_when_satisfied_otherwise_replaced() {
    assert_eq!(bumped(">=1.2, <2", "1.9.0"), ">=1.2, <2");
    assert_eq!(bumped(">=1.2, <2", "2.1.0"), "2.1.0");
    assert_eq!(bumped(">=1.2", "5.0.0"), ">=1.2");
    assert_eq!(bumped("<1.5", "1.6.0"), "1.6.0");
  }
}
