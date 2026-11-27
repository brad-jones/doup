//! Encodes the version a pull request updates to in its body.
//!
//! The version is kept in a hidden HTML comment, eg: `<!-- doup:version=1.2.3 -->`,
//! so it survives a maintainer editing the PR's title.

const PREFIX: &str = "<!--";
const SUFFIX: &str = "-->";
const KEY: &str = "doup:version=";

/// The marker to embed in a pull request body for `version`.
pub fn marker(version: &str) -> String {
  format!("{PREFIX} {KEY}{version} {SUFFIX}")
}

/// The version from the first marker in `body`, or an empty string if there isn't a valid one.
///
/// An empty version never equals a real one, so a closed PR without a marker
/// never stops a new PR being created, while an open one is still updated.
pub fn parse(body: Option<&str>) -> String {
  let mut rest = body.unwrap_or_default();
  while let Some(start) = rest.find(PREFIX) {
    rest = &rest[start + PREFIX.len()..];
    let Some(end) = rest.find(SUFFIX) else { break };
    let comment = rest[..end].trim();
    rest = &rest[end + SUFFIX.len()..];

    if let Some(version) = comment.strip_prefix(KEY).map(str::trim)
      && !version.is_empty()
      && !version.chars().any(char::is_whitespace)
    {
      return version.to_string();
    }
  }
  String::new()
}

#[cfg(test)]
mod tests {
  use super::{marker, parse};

  #[test]
  fn a_marker_round_trips() {
    assert_eq!(parse(Some(&marker("1.2.3"))), "1.2.3");
  }

  #[test]
  fn the_marker_is_found_amongst_other_content() {
    let body = "Bumps foo.\n\n<!-- some other comment -->\n<!--doup:version=2.0.0-rc.1-->\n<!-- doup:version=9.9.9 -->";
    assert_eq!(parse(Some(body)), "2.0.0-rc.1");
  }

  #[test]
  fn no_valid_marker_is_an_empty_version() {
    for body in [
      None,
      Some(""),
      Some("Bumps foo to 1.2.3"),
      Some("<!-- doup:version= -->"),
      Some("<!-- doup:version=1 2 -->"),
      Some("<!-- doup:version=1.2.3"),
    ] {
      assert_eq!(parse(body), "", "{body:?}");
    }
  }
}
