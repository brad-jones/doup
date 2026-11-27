//! Gathers the release notes of a dependency from the network, using the pure helpers of [`release_notes`].

use crate::{
  Dependency, ReleaseNote,
  error::CargoError,
  http::{get, github_api_headers},
  registry, release_notes,
};
use std::collections::HashMap;

const CRATES_IO_API: &str = "https://crates.io/api/v1";
const GITHUB_API: &str = "https://api.github.com";
const RAW_GITHUB: &str = "https://raw.githubusercontent.com";
const RELEASES_PER_PAGE: usize = 100;
const MAX_RELEASE_PAGES: usize = 5;

/// Release notes are looked up on GitHub, as crates.io has none. The repository comes from the crate's
/// metadata, then GitHub releases are preferred with `CHANGELOG.md` filling in any versions without one.
///
/// Crates from other registries, or without a GitHub repository, have no release notes.
/// Any failure to reach GitHub (rate limited, missing, etc) is an error, so problems aren't masked.
/// The only exception is a repository without a `CHANGELOG.md`.
pub async fn release_notes(dependency: &Dependency, to: &str) -> Result<Vec<ReleaseNote>, CargoError> {
  if dependency.registry.is_some() {
    return Ok(Vec::new());
  }

  let index_url = registry::index_url(None, &dependency.name);
  let index = get(&index_url, &[]).await?.into_success(&index_url)?;
  let wanted = release_notes::versions_between(dependency, &index, to)?;
  if wanted.is_empty() {
    return Ok(Vec::new());
  }

  let crate_url = format!("{CRATES_IO_API}/crates/{}", dependency.name);
  let metadata = get(&crate_url, &[]).await?.into_success(&crate_url)?;
  let Some(repo) = release_notes::repository_url(&metadata)?.and_then(|url| release_notes::github_repo(&url)) else {
    return Ok(Vec::new());
  };

  let mut notes = HashMap::new();
  let mut missing = wanted.clone();

  // Paging stops as soon as every wanted version has a release, as most releases are never looked at.
  let api_headers = github_api_headers();
  for page in 1..=MAX_RELEASE_PAGES {
    let url =
      format!("{GITHUB_API}/repos/{}/{}/releases?per_page={RELEASES_PER_PAGE}&page={page}", repo.owner, repo.name);
    let body = get(&url, &api_headers).await?.into_success(&url)?;

    let releases = release_notes::parse_releases(&body)?;
    for note in release_notes::notes_from_releases(&dependency.name, &releases, &missing) {
      notes.insert(note.version.clone(), note);
    }
    missing.retain(|v| !notes.contains_key(&v.to_string()));

    if missing.is_empty() || releases.len() < RELEASES_PER_PAGE {
      break;
    }
  }

  if !missing.is_empty() {
    let url = format!("{RAW_GITHUB}/{}/{}/HEAD/CHANGELOG.md", repo.owner, repo.name);
    let response = get(&url, &[]).await?;
    // A repository simply may not have a changelog, anything else is unexpected.
    let changelog = if response.status == 404 { String::new() } else { response.into_success(&url)? };
    for note in release_notes::notes_from_changelog(&repo, &changelog, &missing) {
      notes.insert(note.version.clone(), note);
    }
  }

  Ok(wanted.iter().filter_map(|v| notes.remove(&v.to_string())).collect())
}
