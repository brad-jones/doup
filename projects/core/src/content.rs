//! The text doup writes: commit messages, pull request titles & bodies, and the comments it posts.

use crate::reconcile::{CloseReason, PlannedUpdate, TargetKind, UpdateReason};
use crate::release_notes;
use doup_plugin_host::ecosystem_plugin::ReleaseNote;
use doup_plugin_host::platform_plugin::PullRequestContent;
use std::fmt::Write;

/// The most a pull request body may grow to, in bytes. Release notes are cut down to fit.
///
/// Comfortably below GitHub's limit of 65,536 characters, leaving room for anything the platform
/// plugin adds (eg: a version marker).
pub const MAX_BODY_LEN: usize = 60_000;

const DOUP_LINK: &str = "[doup](https://github.com/brad-jones/doup)";

/// Room kept free for the line saying how many releases were left out.
const OMITTED_RESERVE: usize = 64;

/// The commit message of an update, also used as its pull request's title.
/// eg: `chore(doup): update serde from 1.0.0 to 1.1.0`.
pub fn commit_message(update: &PlannedUpdate) -> String {
  let dependency = &update.dependency;
  format!("chore(doup): update {} from {} to {}", dependency.name, dependency.current_version, update.target.version)
}

/// Everything doup sets on the pull request of `update`, see [`pr_body`].
pub fn pr_content(
  base_branch: &str,
  update: &PlannedUpdate,
  notes: Option<&[ReleaseNote]>,
  reason: Option<&UpdateReason>,
) -> PullRequestContent {
  PullRequestContent {
    base_branch: base_branch.to_string(),
    title: commit_message(update),
    body: pr_body(update, notes, reason),
    version: update.target.version.clone(),
  }
}

/// The markdown body of the pull request of `update`, at most [`MAX_BODY_LEN`] bytes long.
///
/// - `notes` are the dependency's release notes, `None` when they could not be fetched. Notes for versions
///   newer than the update's target are left out, then the rest are ordered newest first.
/// - `reason` is why an existing pull request is being updated, if it is.
pub fn pr_body(update: &PlannedUpdate, notes: Option<&[ReleaseNote]>, reason: Option<&UpdateReason>) -> String {
  render_body(update, notes, reason, MAX_BODY_LEN)
}

/// The comment posted to a pull request as doup closes it.
pub fn close_comment(reason: &CloseReason) -> String {
  format!("{reason}\n\n<sub>Closed automatically by {DOUP_LINK}.</sub>\n")
}

fn render_body(
  update: &PlannedUpdate,
  notes: Option<&[ReleaseNote]>,
  reason: Option<&UpdateReason>,
  max_len: usize,
) -> String {
  let mut head = summary(update);
  head.push_str(&details_table(update));
  head.push_str(&occurrences(update));

  let mut tail = String::new();
  if let Some(reason) = reason {
    let _ = writeln!(tail, "_{reason}_\n");
  }
  tail.push_str(&footer());

  let notes = match notes {
    Some(notes) => release_notes(
      &release_notes::for_target(notes, &update.target.version),
      max_len.saturating_sub(head.len() + tail.len()),
    ),
    None => "### Release notes\n\n_doup couldn't fetch the release notes for this update._\n\n".to_string(),
  };
  format!("{head}{notes}{tail}")
}

/// What the update does, with a warning when it may contain breaking changes.
fn summary(update: &PlannedUpdate) -> String {
  let dependency = &update.dependency;
  let mut out = format!(
    "Updates {} from {} to {}.\n\n",
    code(&dependency.name),
    code(&dependency.current_version),
    code(&update.target.version)
  );

  if update.target.kind == TargetKind::Latest {
    let _ = write!(
      out,
      "> [!WARNING]\n> {} is the latest release of {}, but it's outside the version constraints in your manifests, so \
       it may contain breaking changes. Review the release notes below & update your code as needed before merging.\n\n",
      code(&update.target.version),
      code(&dependency.name),
    );
  }

  if let Some(companion) = &update.companion {
    let version = code(&companion.version);
    let _ = match companion.kind {
      TargetKind::Latest => write!(
        out,
        "> [!NOTE]\n> {version}, the latest release, is outside your version constraints & may contain breaking \
         changes. doup proposes it in a separate pull request.\n\n"
      ),
      TargetKind::Next => write!(
        out,
        "> [!NOTE]\n> {version}, the newest version within your version constraints, is proposed in a separate pull \
         request. Merge that one instead if you're not ready for breaking changes.\n\n"
      ),
    };
  }
  out
}

fn details_table(update: &PlannedUpdate) -> String {
  let dependency = &update.dependency;
  let registry = dependency.registry.as_deref().map_or_else(|| "default".to_string(), cell_code);
  format!(
    "| Dependency | Ecosystem | Registry | From | To |\n| --- | --- | --- | --- | --- |\n| {} | {} | {registry} | {} | {} |\n\n",
    cell_code(&dependency.name),
    cell_code(update.ecosystem.name()),
    cell_code(&dependency.current_version),
    cell_code(&update.target.version),
  )
}

/// Every manifest the dependency was found in, empty when the plugin didn't say.
fn occurrences(update: &PlannedUpdate) -> String {
  let occurrences = &update.dependency.occurrences;
  if occurrences.is_empty() {
    return String::new();
  }
  let mut out =
    "### Where it's used\n\n| Manifest | Declared | Lockfile | Locked |\n| --- | --- | --- | --- |\n".to_string();
  for occurrence in occurrences {
    let optional = |value: &Option<String>| value.as_deref().map_or_else(|| "-".to_string(), cell_code);
    let _ = writeln!(
      out,
      "| {} | {} | {} | {} |",
      cell_code(&occurrence.manifest_path),
      cell_code(&occurrence.current_version),
      optional(&occurrence.lock_path),
      optional(&occurrence.locked_version),
    );
  }
  out.push('\n');
  out
}

/// `notes` (newest first) in collapsible sections, within `max_len` bytes. Once a note no longer fits, it
/// & every older note is listed as a link instead. Any that don't fit even then are counted.
fn release_notes(notes: &[ReleaseNote], max_len: usize) -> String {
  let mut out = "### Release notes\n\n".to_string();
  if notes.is_empty() {
    out.push_str("_No release notes were found for this update._\n\n");
    return out;
  }

  let fits = |out: &str, more: &str| out.len() + more.len() + OMITTED_RESERVE <= max_len;
  let mut links_only = false;
  let mut omitted = 0;
  for (index, note) in notes.iter().enumerate() {
    if !links_only {
      let full = full_note(note, index == 0);
      if fits(&out, &full) {
        out.push_str(&full);
        continue;
      }
      links_only = true;
      let intro = "Older releases, too long to include here:\n\n";
      if fits(&out, intro) {
        out.push_str(intro);
      }
    }
    let link = format!("- {}\n", note_link(note));
    if omitted == 0 && fits(&out, &link) {
      out.push_str(&link);
    } else {
      omitted += 1;
    }
  }
  if links_only {
    out.push('\n');
  }
  if omitted > 0 {
    let _ = writeln!(out, "_...and {omitted} older release(s)._\n");
  }
  out
}

fn full_note(note: &ReleaseNote, open: bool) -> String {
  let mut out = format!("<details{}>\n<summary>{}</summary>\n\n", if open { " open" } else { "" }, html(&note.version));
  if !note.url.trim().is_empty() {
    let _ = writeln!(out, "[Full release notes]({})\n", note.url.trim());
  }
  match note.body.trim() {
    "" => out.push_str("_No release notes were provided for this version._\n"),
    body => {
      out.push_str(body);
      out.push('\n');
    }
  }
  out.push_str("\n</details>\n\n");
  out
}

fn note_link(note: &ReleaseNote) -> String {
  match note.url.trim() {
    "" => code(&note.version),
    url => format!("[{}]({url})", note.version.replace(['[', ']'], "")),
  }
}

fn footer() -> String {
  format!(
    "---\n\n<sub>This pull request is managed by {DOUP_LINK}. Close it to ignore this version: doup won't propose it \
     again, but will open a new pull request when a different version is available. doup keeps this branch up to \
     date with the default branch until someone else pushes to it.</sub>\n"
  )
}

/// `value` as inline code, even when it contains backticks.
fn code(value: &str) -> String {
  if value.contains('`') { format!("`` {value} ``") } else { format!("`{value}`") }
}

/// [`code`] that is safe inside a table cell.
fn cell_code(value: &str) -> String {
  code(&value.replace('|', "\\|"))
}

/// `value` escaped for use as HTML text.
fn html(value: &str) -> String {
  value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::reconcile::Target;
  use doup_config::repo::DoupRepoPlugin;
  use doup_plugin_host::ecosystem_plugin::{Dependency, Occurrence};

  fn update(target: Target, companion: Option<Target>) -> PlannedUpdate {
    PlannedUpdate {
      ecosystem: DoupRepoPlugin { plugin: "./plugins/cargo.wasm".into(), registry: None, with: None },
      dependency: Dependency {
        name: "serde".into(),
        registry: None,
        occurrences: vec![Occurrence {
          manifest_path: "Cargo.toml".into(),
          lock_path: Some("Cargo.lock".into()),
          current_version: "^1.0 || ^0.9".into(),
          locked_version: Some("1.0.0".into()),
        }],
        current_version: "1.0.0".into(),
      },
      target,
      companion,
    }
  }

  fn next() -> PlannedUpdate {
    update(Target::new("1.1.0", TargetKind::Next), Some(Target::new("2.0.0", TargetKind::Latest)))
  }

  fn latest() -> PlannedUpdate {
    update(Target::new("2.0.0", TargetKind::Latest), Some(Target::new("1.1.0", TargetKind::Next)))
  }

  fn note(version: &str, body: &str) -> ReleaseNote {
    ReleaseNote { version: version.into(), url: format!("https://example.com/{version}"), body: body.into() }
  }

  #[test]
  fn the_commit_message_names_the_update() {
    assert_eq!(commit_message(&next()), "chore(doup): update serde from 1.0.0 to 1.1.0");
    let content = pr_content("main", &latest(), Some(&[]), None);
    assert_eq!(content.title, "chore(doup): update serde from 1.0.0 to 2.0.0");
    assert_eq!((content.base_branch.as_str(), content.version.as_str()), ("main", "2.0.0"));
  }

  #[test]
  fn the_body_summarises_the_update() {
    let body = pr_body(&next(), Some(&[]), None);
    assert!(body.starts_with("Updates `serde` from `1.0.0` to `1.1.0`.\n"), "{body}");
    assert!(body.contains("| `serde` | `cargo` | default | `1.0.0` | `1.1.0` |"), "{body}");
    assert!(body.contains("| `Cargo.toml` | `^1.0 \\|\\| ^0.9` | `Cargo.lock` | `1.0.0` |"), "{body}");
    assert!(body.contains("_No release notes were found for this update._"), "{body}");
    assert!(body.contains("Close it to ignore this version"), "{body}");
  }

  #[test]
  fn next_and_latest_bodies_point_at_each_other() {
    let next = pr_body(&next(), Some(&[]), None);
    assert!(!next.contains("[!WARNING]"), "{next}");
    assert!(next.contains("> `2.0.0`, the latest release, is outside your version constraints"), "{next}");

    let latest = pr_body(&latest(), Some(&[]), None);
    assert!(latest.contains("> [!WARNING]\n> `2.0.0` is the latest release of `serde`"), "{latest}");
    assert!(latest.contains("> `1.1.0`, the newest version within your version constraints"), "{latest}");
  }

  #[test]
  fn release_notes_are_filtered_to_the_target_newest_first() {
    let notes = [note("1.0.1", "fixes"), note("2.0.0", "breaking"), note("1.1.0", "")];
    let body = pr_body(&next(), Some(&notes), None);
    assert!(!body.contains("2.0.0</summary>"), "{body}");
    let newest = body.find("<details open>\n<summary>1.1.0</summary>").expect(&body);
    let older = body.find("<details>\n<summary>1.0.1</summary>").expect(&body);
    assert!(newest < older, "{body}");
    assert!(body.contains("[Full release notes](https://example.com/1.0.1)\n\nfixes\n"), "{body}");
    assert!(body.contains("_No release notes were provided for this version._"), "{body}");

    let body = pr_body(&latest(), Some(&notes), None);
    assert!(body.contains("<details open>\n<summary>2.0.0</summary>"), "{body}");
  }

  #[test]
  fn missing_release_notes_are_explained() {
    let body = pr_body(&next(), None, None);
    assert!(body.contains("_doup couldn't fetch the release notes for this update._"), "{body}");
  }

  #[test]
  fn the_update_reason_is_noted() {
    let body = pr_body(&next(), Some(&[]), Some(&UpdateReason::BehindDefaultBranch));
    assert!(body.contains(&format!("_{}_", UpdateReason::BehindDefaultBranch)), "{body}");
  }

  #[test]
  fn release_notes_that_do_not_fit_become_links_then_a_count() {
    let notes: Vec<_> = (0..6).rev().map(|patch| note(&format!("1.0.{patch}"), &"x".repeat(400))).collect();
    let section = release_notes(&notes, 1_200);
    assert!(section.len() <= 1_200, "{}", section.len());
    assert!(section.contains("<summary>1.0.5</summary>"), "{section}");
    assert!(section.contains("Older releases, too long to include here:"), "{section}");
    assert!(!section.contains("<summary>1.0.0</summary>"), "{section}");

    let section = release_notes(&notes, 900);
    assert!(section.contains("- [1.0.4](https://example.com/1.0.4)\n"), "{section}");

    let section = release_notes(&notes, 100);
    assert!(section.contains("_...and 6 older release(s)._"), "{section}");
  }

  #[test]
  fn huge_release_notes_never_overflow_the_body() {
    let notes: Vec<_> = (0..200).map(|patch| note(&format!("1.0.{patch}"), &"y".repeat(5_000))).collect();
    let body = pr_body(&update(Target::new("1.0.200", TargetKind::Next), None), Some(&notes), None);
    assert!(body.len() <= MAX_BODY_LEN, "{}", body.len());
    assert!(body.ends_with("</sub>\n"), "the footer must survive");
  }

  #[test]
  fn markdown_is_escaped() {
    assert_eq!(code("a`b"), "`` a`b ``");
    assert_eq!(cell_code("a|b"), "`a\\|b`");
    assert_eq!(html("<b>&"), "&lt;b&gt;&amp;");
  }

  #[test]
  fn the_close_comment_explains_why() {
    let comment = close_comment(&CloseReason::UpToDate);
    assert!(comment.starts_with(&CloseReason::UpToDate.to_string()), "{comment}");
    assert!(comment.contains("Closed automatically by [doup]"), "{comment}");
  }
}
