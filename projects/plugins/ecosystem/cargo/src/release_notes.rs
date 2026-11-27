//! Works out which release notes are needed for a crate and turns GitHub releases and
//! `CHANGELOG.md` files into them.
//!
//! This module only uses plain `std` / `serde` types (plus the WIT records) so that it can be
//! unit tested natively, without a wasm runtime.

use crate::{Dependency, ReleaseNote, error::CargoError, lookup};
use semver::Version;
use serde::Deserialize;
use std::collections::HashMap;

/// A repository hosted on github.com.
#[derive(Debug, PartialEq, Eq)]
pub struct GithubRepo {
  pub owner: String,
  pub name: String,
}

/// Returns the versions that a release note is wanted for, newest first.
///
/// That is every version the index offers after the lowest installed version, up to and including `to`.
pub fn versions_between(dependency: &Dependency, index: &str, to: &str) -> Result<Vec<Version>, CargoError> {
  let to = Version::parse(to).map_err(|source| CargoError::InvalidVersion {
    name: dependency.name.clone(),
    version: to.to_string(),
    source,
  })?;
  let lookup::Analysis { installed, candidates, .. } = lookup::analyse(dependency, index)?;
  let Some(installed) = installed else {
    return Ok(Vec::new());
  };

  let mut versions: Vec<_> = candidates.into_iter().filter(|v| *v > installed && *v <= to).collect();
  versions.sort_unstable_by(|a, b| b.cmp(a));
  versions.dedup();
  Ok(versions)
}

#[derive(Deserialize)]
struct CrateResponse {
  #[serde(rename = "crate")]
  krate: CrateInfo,
}

#[derive(Deserialize)]
struct CrateInfo {
  repository: Option<String>,
}

/// Reads the repository URL out of a crates.io API response for a crate.
pub fn repository_url(crate_json: &str) -> Result<Option<String>, CargoError> {
  let response: CrateResponse =
    serde_json::from_str(crate_json).map_err(|source| CargoError::InvalidResponse { what: "crates.io", source })?;
  Ok(response.krate.repository)
}

/// Parses `url` as a github.com repository, eg: `https://github.com/serde-rs/serde`.
///
/// Anything below the repository (eg: `/tree/main/crate`) and a `.git` suffix are ignored.
pub fn github_repo(url: &str) -> Option<GithubRepo> {
  let url = url.trim();
  let path =
    ["https://", "http://", "git://", "ssh://git@", "git@"].iter().find_map(|scheme| url.strip_prefix(scheme))?;
  let path = path.strip_prefix("www.").unwrap_or(path);
  let path = path.strip_prefix("github.com")?;
  let path = path.strip_prefix(['/', ':'])?;

  let mut segments = path.split('/');
  let owner = segments.next()?;
  let name = segments.next()?;
  let name = name.strip_suffix(".git").unwrap_or(name);

  let valid = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
  (valid(owner) && valid(name)).then(|| GithubRepo { owner: owner.to_string(), name: name.to_string() })
}

/// The fields of a GitHub release (as returned by the REST API) that we use.
#[derive(Deserialize)]
pub struct GithubRelease {
  tag_name: String,
  html_url: String,
  body: Option<String>,
  #[serde(default)]
  draft: bool,
}

/// Parses the JSON array returned by GitHub's list releases endpoint.
pub fn parse_releases(json: &str) -> Result<Vec<GithubRelease>, CargoError> {
  serde_json::from_str(json).map_err(|source| CargoError::InvalidResponse { what: "GitHub releases", source })
}

/// Maps `releases` to a release note for each of the `wanted` versions that has one.
///
/// A tag is the version, optionally prefixed with `v` and/or the crate's name, so tags in a
/// repository that holds several crates only match the crate they were made for.
/// Drafts are ignored as they aren't public.
pub fn notes_from_releases(crate_name: &str, releases: &[GithubRelease], wanted: &[Version]) -> Vec<ReleaseNote> {
  let mut notes: HashMap<&Version, ReleaseNote> = HashMap::new();
  for release in releases.iter().filter(|r| !r.draft) {
    let Some(version) = tag_version(&release.tag_name, crate_name) else { continue };
    let Some(wanted) = wanted.iter().find(|w| **w == version) else { continue };
    notes.entry(wanted).or_insert_with(|| ReleaseNote {
      version: wanted.to_string(),
      url: release.html_url.clone(),
      body: release.body.clone().unwrap_or_default(),
    });
  }
  notes.into_values().collect()
}

/// The version a release tag stands for, eg: `v1.2.3`, `serde-1.2.3` and `serde@v1.2.3` are all `1.2.3`.
fn tag_version(tag: &str, crate_name: &str) -> Option<Version> {
  let rest = strip_crate_prefix(tag, crate_name).unwrap_or(tag);
  Version::parse(rest.strip_prefix('v').unwrap_or(rest)).ok()
}

/// Strips `<crate_name><separator>` from the start of `tag`. Cargo treats `-` and `_` in names the same.
fn strip_crate_prefix<'a>(tag: &'a str, crate_name: &str) -> Option<&'a str> {
  let prefix = tag.get(..crate_name.len())?;
  let same = prefix
    .chars()
    .zip(crate_name.chars())
    .all(|(a, b)| a.eq_ignore_ascii_case(&b) || (matches!(a, '-' | '_') && matches!(b, '-' | '_')));
  if !same {
    return None;
  }
  tag[crate_name.len()..].strip_prefix(['-', '_', '@', '/'])
}

/// Extracts a release note for each of the `wanted` versions that has a section in `changelog`.
///
/// A section starts at a markdown heading naming the version, eg: `## [1.2.3] - 2024-01-31`, and runs
/// until the next heading of the same or a higher level. `url` links to the heading of the changelog
/// on `repo`'s default branch.
pub fn notes_from_changelog(repo: &GithubRepo, changelog: &str, wanted: &[Version]) -> Vec<ReleaseNote> {
  let headings = headings(changelog);
  let lines: Vec<&str> = changelog.lines().collect();

  let mut notes: HashMap<Version, ReleaseNote> = HashMap::new();
  for (position, heading) in headings.iter().enumerate() {
    let Some(version) = heading_version(&heading.text) else { continue };
    if !wanted.contains(&version) || notes.contains_key(&version) {
      continue;
    }

    let end = headings[position + 1..].iter().find(|h| h.level <= heading.level).map_or(lines.len(), |h| h.line);
    let body = lines[heading.line + 1..end].join("\n").trim().to_string();
    let url = format!("https://github.com/{}/{}/blob/HEAD/CHANGELOG.md#{}", repo.owner, repo.name, heading.anchor);
    notes.insert(version.clone(), ReleaseNote { version: version.to_string(), url, body });
  }
  notes.into_values().collect()
}

struct Heading {
  /// Zero based index of the line the heading is on.
  line: usize,
  level: usize,
  text: String,
  /// The GitHub generated anchor for the heading.
  anchor: String,
}

/// Finds the ATX headings (`## Title`) in `markdown`, skipping anything inside fenced code blocks.
fn headings(markdown: &str) -> Vec<Heading> {
  let mut headings = Vec::new();
  let mut seen: HashMap<String, usize> = HashMap::new();
  let mut fence: Option<char> = None;

  for (line, content) in markdown.lines().enumerate() {
    let trimmed = content.trim_start();
    if let Some(marker) = trimmed.chars().next().filter(|c| matches!(c, '`' | '~'))
      && trimmed.starts_with(&marker.to_string().repeat(3))
    {
      match fence {
        None => fence = Some(marker),
        Some(open) if open == marker => fence = None,
        Some(_) => {}
      }
      continue;
    }
    // Markdown allows up to three spaces of indentation before a heading.
    if fence.is_some() || content.len() - trimmed.len() > 3 {
      continue;
    }

    let level = trimmed.chars().take_while(|c| *c == '#').count();
    let Some(rest) = trimmed[level..].strip_prefix([' ', '\t']).filter(|_| (1..=6).contains(&level)) else {
      continue;
    };
    let text = rest.trim().trim_end_matches('#').trim().to_string();

    let slug = slug(&text);
    let count = seen.entry(slug.clone()).or_insert(0);
    let anchor = if *count == 0 { slug } else { format!("{slug}-{count}") };
    *count += 1;

    headings.push(Heading { line, level, text, anchor });
  }
  headings
}

/// The first version mentioned in a changelog heading, eg: `[1.2.3] - 2024-01-31` or `Version v1.2.3`.
fn heading_version(text: &str) -> Option<Version> {
  text
    .split(|c: char| c.is_whitespace() || matches!(c, '[' | ']' | '(' | ')' | ',' | ':' | '*' | '`'))
    .find_map(|token| Version::parse(token.strip_prefix('v').unwrap_or(token)).ok())
}

/// How GitHub turns a heading into an anchor: lowercased, punctuation removed and spaces become hyphens.
fn slug(heading: &str) -> String {
  let mut text = String::new();
  let mut rest = heading;
  // A link renders as its text, so drop its destination.
  while let Some(start) = rest.find("](") {
    text.push_str(&rest[..start + 1]);
    match rest[start..].find(')') {
      Some(end) => rest = &rest[start + end + 1..],
      None => {
        rest = "";
        break;
      }
    }
  }
  text.push_str(rest);

  text
    .to_lowercase()
    .chars()
    .filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_'))
    .map(|c| if c == ' ' { '-' } else { c })
    .collect()
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::Occurrence;

  fn version(v: &str) -> Version {
    Version::parse(v).unwrap()
  }

  fn dependency(locked: &str) -> Dependency {
    Dependency {
      name: "foo".to_string(),
      registry: None,
      current_version: locked.to_string(),
      occurrences: vec![Occurrence {
        manifest_path: "Cargo.toml".to_string(),
        lock_path: Some("Cargo.lock".to_string()),
        current_version: "1".to_string(),
        locked_version: Some(locked.to_string()),
      }],
    }
  }

  fn index(versions: &[(&str, bool)]) -> String {
    versions.iter().map(|(v, yanked)| format!("{{\"vers\":\"{v}\",\"yanked\":{yanked}}}\n")).collect()
  }

  fn repo() -> GithubRepo {
    GithubRepo { owner: "acme".to_string(), name: "foo".to_string() }
  }

  fn release(tag: &str, body: Option<&str>, draft: bool) -> GithubRelease {
    GithubRelease {
      tag_name: tag.to_string(),
      html_url: format!("https://github.com/acme/foo/releases/tag/{tag}"),
      body: body.map(str::to_string),
      draft,
    }
  }

  fn sorted(mut notes: Vec<ReleaseNote>) -> Vec<(String, String, String)> {
    notes.sort_by(|a, b| a.version.cmp(&b.version));
    notes.into_iter().map(|n| (n.version, n.url, n.body)).collect()
  }

  #[test]
  fn range_is_after_the_installed_version_up_to_and_including_to() {
    let versions = versions_between(
      &dependency("1.1.0"),
      &index(&[("1.0.0", false), ("1.1.0", false), ("1.2.0", false), ("1.3.0", false), ("2.0.0", false)]),
      "1.3.0",
    )
    .unwrap();
    assert_eq!(versions, vec![version("1.3.0"), version("1.2.0")]);
  }

  #[test]
  fn range_skips_yanked_and_prereleases() {
    let versions = versions_between(
      &dependency("1.0.0"),
      &index(&[("1.0.0", false), ("1.1.0", true), ("1.2.0-rc.1", false), ("1.2.0", false)]),
      "1.2.0",
    )
    .unwrap();
    assert_eq!(versions, vec![version("1.2.0")]);
  }

  #[test]
  fn range_starts_at_the_lowest_occurrence() {
    let mut dep = dependency("1.3.0");
    dep.occurrences.push(Occurrence { locked_version: Some("1.1.0".to_string()), ..dep.occurrences[0].clone() });
    let versions = versions_between(&dep, &index(&[("1.1.0", false), ("1.2.0", false), ("1.3.0", false)]), "1.3.0");
    assert_eq!(versions.unwrap(), vec![version("1.3.0"), version("1.2.0")]);
  }

  #[test]
  fn invalid_to_is_an_error() {
    assert!(versions_between(&dependency("1.0.0"), &index(&[]), "nope").is_err());
  }

  #[test]
  fn repository_url_is_read_from_the_crate() {
    let json = r#"{"crate":{"id":"foo","repository":"https://github.com/acme/foo"},"versions":[]}"#;
    assert_eq!(repository_url(json).unwrap().as_deref(), Some("https://github.com/acme/foo"));
    assert_eq!(repository_url(r#"{"crate":{"repository":null}}"#).unwrap(), None);
    assert!(repository_url("nope").is_err());
  }

  #[test]
  fn github_urls_are_recognised() {
    for url in [
      "https://github.com/acme/foo",
      "https://www.github.com/acme/foo/",
      "https://github.com/acme/foo.git",
      "https://github.com/acme/foo/tree/main/crates/foo",
      "git@github.com:acme/foo.git",
      "ssh://git@github.com/acme/foo",
    ] {
      assert_eq!(github_repo(url), Some(repo()), "{url}");
    }
  }

  #[test]
  fn other_urls_are_not_github() {
    for url in
      ["https://gitlab.com/acme/foo", "https://github.com/acme", "https://github.com.evil.com/a/b", "acme/foo", ""]
    {
      assert_eq!(github_repo(url), None, "{url}");
    }
  }

  #[test]
  fn releases_match_plain_prefixed_and_crate_tags() {
    let releases = vec![
      release("v1.0.0", Some("one"), false),
      release("foo-v1.1.0", Some("two"), false),
      release("Foo_1.2.0", None, false),
      release("foo@1.3.0", Some("four"), false),
      release("other-1.4.0", Some("not ours"), false),
      release("1.5.0", Some("draft"), true),
      release("nightly", Some("not a version"), false),
    ];
    let wanted: Vec<_> = ["1.0.0", "1.1.0", "1.2.0", "1.3.0", "1.4.0", "1.5.0"].map(version).into();
    let notes = sorted(notes_from_releases("foo", &releases, &wanted));

    let bodies: Vec<_> = notes.iter().map(|(v, _, b)| (v.as_str(), b.as_str())).collect();
    assert_eq!(bodies, [("1.0.0", "one"), ("1.1.0", "two"), ("1.2.0", ""), ("1.3.0", "four")]);
    assert_eq!(notes[0].1, "https://github.com/acme/foo/releases/tag/v1.0.0");
  }

  #[test]
  fn releases_only_cover_wanted_versions() {
    let releases = vec![release("v1.0.0", Some("one"), false), release("v2.0.0", Some("two"), false)];
    let notes = notes_from_releases("foo", &releases, &[version("2.0.0")]);
    assert_eq!(sorted(notes).len(), 1);
  }

  const CHANGELOG: &str = "# Changelog

## [Unreleased]

- Nothing yet

## [1.2.0] - 2024-02-01

### Added

- A thing

```md
## [1.1.5] - not a real heading
```

## 1.1.0 (2024-01-01)

Fixed stuff

## [1.0.0](https://example.com/compare/v1.0.0) - 2023-12-01

- First
";

  #[test]
  fn changelog_sections_are_extracted_with_their_anchor() {
    let wanted = ["1.2.0", "1.1.0", "1.0.0", "1.1.5"].map(version);
    let notes = sorted(notes_from_changelog(&repo(), CHANGELOG, &wanted));

    let url = |anchor: &str| format!("https://github.com/acme/foo/blob/HEAD/CHANGELOG.md#{anchor}");
    assert_eq!(
      notes,
      [
        ("1.0.0".to_string(), url("100---2023-12-01"), "- First".to_string()),
        ("1.1.0".to_string(), url("110-2024-01-01"), "Fixed stuff".to_string()),
        (
          "1.2.0".to_string(),
          url("120---2024-02-01"),
          "### Added\n\n- A thing\n\n```md\n## [1.1.5] - not a real heading\n```".to_string()
        ),
      ]
    );
  }

  #[test]
  fn duplicate_headings_get_numbered_anchors() {
    let changelog = "## 1.0.0\n\nfirst\n\n## 1.0.0\n\nsecond\n";
    let headings = headings(changelog);
    assert_eq!(headings[0].anchor, "100");
    assert_eq!(headings[1].anchor, "100-1");
  }

  #[test]
  fn changelog_without_the_versions_has_no_notes() {
    assert!(notes_from_changelog(&repo(), CHANGELOG, &[version("9.9.9")]).is_empty());
    assert!(notes_from_changelog(&repo(), "", &[version("1.0.0")]).is_empty());
  }
}
