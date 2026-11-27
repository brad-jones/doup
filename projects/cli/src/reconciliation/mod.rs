use std::collections::HashSet;

use anyhow::Result;
use doup_config::repo::DoupRepoPlugin;
use doup_git::{DOUP_BRANCH_PREFIX, UpdateBranch};
use doup_plugin_host::ecosystem_plugin::{Dependency, ReleaseNote, Versions};
use nutype::nutype;

type Updates<'a> = Vec<(&'a DoupRepoPlugin, &'a Dependency, &'a Versions, Vec<ReleaseNote>)>;

#[derive(Debug)]
pub struct ToVersion {
  pub version: String,
  pub latest: bool,
}

#[nutype(derive(AsRef, Debug, Display))]
pub struct PRNumber(i64);

#[nutype(derive(AsRef, Debug, Display))]
pub struct BranchName(String);

#[nutype(derive(AsRef, Debug, Display))]
pub struct Reason(String);

#[derive(Debug)]
pub enum DoupAction {
  CreatePR(DoupRepoPlugin, Dependency, ToVersion, Vec<ReleaseNote>, Reason),
  UpdatePR(PRNumber, DoupRepoPlugin, Dependency, ToVersion, Vec<ReleaseNote>, Reason),
  ClosePR(PRNumber, Reason),
  DeleteBranch(BranchName, Reason),
  SkipUpdate(Dependency, ToVersion, Reason),
}

impl DoupAction {
  /// A one-line, human friendly summary of the action, for debugging output.
  pub fn summary(&self) -> String {
    match self {
      Self::CreatePR(plugin, dep, to, ..) => {
        format!("CREATE PR: {} {} -> {}", plugin.plugin, dep_label(dep), to_label(to))
      }
      Self::UpdatePR(pr, plugin, dep, to, ..) => {
        format!("UPDATE PR: #{} {} {} -> {}", pr.as_ref(), plugin.plugin, dep_label(dep), to_label(to),)
      }
      Self::ClosePR(pr, reason) => format!("CLOSE PR: #{} ({})", pr.as_ref(), reason.as_ref()),
      Self::DeleteBranch(branch, reason) => format!("DELETE BRANCH: {} ({})", branch.as_ref(), reason.as_ref()),
      Self::SkipUpdate(dep, to, reason) => {
        format!("SKIP: {} -> {} ({})", dep_label(dep), to_label(to), reason.as_ref())
      }
    }
  }
}

fn dep_label(dep: &Dependency) -> String {
  match dep.registry {
    Some(ref reg) => format!("{}/{}", reg, dep.name),
    None => dep.name.clone(),
  }
}

fn to_label(to: &ToVersion) -> String {
  if to.latest { format!("{} (latest)", to.version) } else { to.version.clone() }
}

pub struct PullRequest {
  /// The pull request number.
  /// eg: https://github.com/<owner>/<repo>/pull/<number>
  pub number: i64,

  /// The version number that this pull request is updating to.
  /// This should be a plain version number without any semver constraints. eg: "1.2.3".
  /// Although does not strictly need to be a semver.
  /// When a PR is created this values from either `versions.next` or `versions.latest`.
  /// But it up to the platform plugin as to how this value is actually encoded into the PR. eg: PR Title.
  pub version: String,

  /// The head branch of the PR - eg: `doup/{{plugin}}/next/{{name}}-{{hash}}`
  /// [`reconcile`] only expects branches starting with the [`DOUP_BRANCH_PREFIX`].
  /// Do not include any arbitrary pull request in the list.
  /// For more details on the format see: [`UpdateBranch`]
  pub head_branch: String,

  /// The state of the PR, we only care about Open & Closed PRs.
  ///
  /// Merged PRs are meaningless to [`reconcile`] and are just extra noise,
  /// don't bundle merged PRs under [`PullRequestState::Closed`].
  ///
  /// When fetching closed PRs only include PRs whose `head_branch` matches one of our `candidates`.
  pub state: PullRequestState,

  /// Who has committed to the PR's head branch & how it relates to the default branch.
  /// Only consulted for [`PullRequestState::Open`] PRs, use [`HeadStatus::UpToDate`] for closed PRs.
  pub head_status: HeadStatus,
}

#[derive(PartialEq)]
pub enum PullRequestState {
  Open,
  Closed,
}

/// The state of a pull request's head branch, which decides whether doup may rebuild it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadStatus {
  /// Only doup has committed to the head branch & it contains the tip of the default branch,
  /// or its status is unknown.
  UpToDate,

  /// Only doup has committed to the head branch & it is behind the default branch,
  /// so it should be rebuilt on top of the default branch.
  Behind,

  /// Someone other than doup has committed to the head branch, so it is never rebuilt
  /// (whether behind the default branch or not) as that would throw their work away.
  ForeignCommits,
}

/// This function is pure & should remain pure.
/// Based on a set of inputs, it will decide what actions doup should take.
///
/// - `candidates`: The list of candidate updates that have been discovered by
///   inspecting the default branch of a given repo.
///
/// - `existing_prs`: A list of existing pull request metadata.
///   All Open PRs with the prefix `doup/` should be included.
///   All Closed PRs that match one of the candidates branch names should be included.
///   Use [`UpdateBranch`] to build a branch name for a given dependency.
///   An open PR at the version we want is still updated when its [`HeadStatus`] is [`HeadStatus::Behind`],
///   while one with [`HeadStatus::ForeignCommits`] is never updated.
///
/// - `existing_branches`: A list of existing branch names.
///   **WARNING:** Any branch given in this list that starts with `doup/` may be scheduled for deletion.
///   If you do not want any branches to be deleted under any circumstances, just send an empty list.
pub fn reconcile(
  candidates: Updates,
  existing_prs: Vec<PullRequest>,
  existing_branches: Vec<String>,
) -> Result<Vec<DoupAction>> {
  let mut actions: Vec<DoupAction> = Vec::new();

  // Loop through every candidate update and decide how to proceed.
  // This will result in either a CreatePR, UpdatePR or SkipUpdate.
  let mut claimed_prs: HashSet<i64> = HashSet::new();
  for (ecosystem_ref, dependency, versions, ref notes) in candidates {
    // When only a breaking release is newer, `next` is just the version already installed.
    if versions.next != dependency.current_version {
      claimed_prs.extend(reconcile_candidate(
        &mut actions,
        (ToVersion { version: versions.next.clone(), latest: false }, ecosystem_ref, dependency, notes),
        &existing_prs,
      ));
    }

    // Every candidate effectively has the option for a BREAKING CHANGE candidate with-in it.
    if let Some(latest) = versions.latest.as_ref().filter(|latest| **latest != dependency.current_version) {
      claimed_prs.extend(reconcile_candidate(
        &mut actions,
        (ToVersion { version: latest.clone(), latest: true }, ecosystem_ref, dependency, notes),
        &existing_prs,
      ));
    }
  }

  // Now find any old PRs that we can close
  let unmatched_open_prs: Vec<&PullRequest> = existing_prs
    .iter()
    .filter(|pr| pr.head_branch.starts_with(DOUP_BRANCH_PREFIX))
    .filter(|pr| pr.state == PullRequestState::Open)
    .filter(|pr| !claimed_prs.contains(&pr.number))
    .collect();

  for pr in unmatched_open_prs {
    actions.push(DoupAction::ClosePR(
      PRNumber::new(pr.number),
      Reason::new(
        "
        This pr is being closed because it does not match any of our
        candidates and is not already marked for update.
        ",
      ),
    ));

    if existing_branches.iter().find(|branch| **branch == pr.head_branch).is_some() {
      actions.push(DoupAction::DeleteBranch(
        BranchName::new(pr.head_branch.clone()),
        Reason::new(
          "
          This branch is being deleted because it's pr is being closed
          & was listed in the existing_branches list.
          ",
        ),
      ));
    }
  }

  // Similarly do a round of cleanup against any branches not attached to any PRs.
  let in_use_branches: HashSet<String> = existing_prs
    .iter()
    .map(|pr| pr.head_branch.clone())
    .chain(actions.iter().filter_map(|action| match action {
      DoupAction::CreatePR(plugin, dependency, to_version, ..)
      | DoupAction::UpdatePR(_, plugin, dependency, to_version, ..) => {
        Some(UpdateBranch::new(plugin, dependency, to_version.latest).to_string())
      }
      _ => None,
    }))
    .collect();

  let orphaned_branches: Vec<String> = existing_branches
    .into_iter()
    .filter(|branch| branch.starts_with(DOUP_BRANCH_PREFIX))
    .filter(|branch| !in_use_branches.contains(branch))
    .collect();

  for branch in orphaned_branches {
    actions.push(DoupAction::DeleteBranch(
      BranchName::new(branch),
      Reason::new(
        "
        This branch is being deleted because it has become orphaned, there are
        no existing prs that are associated with this branch, nor would any of
        our other actions create or update the branch.
        ",
      ),
    ));
  }

  Ok(actions)
}

fn reconcile_candidate(
  actions: &mut Vec<DoupAction>,
  candidate: (ToVersion, &DoupRepoPlugin, &Dependency, &Vec<ReleaseNote>),
  existing_prs: &[PullRequest],
) -> Option<i64> {
  let (to_version, ecosystem_ref, dependency, notes) = candidate;

  if let Some(pr) = existing_prs
    .iter()
    .find(|pr| pr.head_branch == UpdateBranch::new(ecosystem_ref, dependency, to_version.latest).to_string())
  {
    // An open PR matched by a candidate (updated or skipped) must not be closed as unmatched.
    let claimed = (pr.state == PullRequestState::Open).then_some(pr.number);
    match pr.state {
      PullRequestState::Open => {
        if pr.head_status == HeadStatus::ForeignCommits {
          actions.push(DoupAction::SkipUpdate(
            dependency.clone(),
            to_version,
            Reason::new(
              "
              There is an existing open pr for our dependency but someone other
              than doup has committed to it, so we leave it alone rather than
              throw their work away.
              ",
            ),
          ));
        } else if pr.version != to_version.version {
          actions.push(DoupAction::UpdatePR(
            PRNumber::new(pr.number),
            ecosystem_ref.clone(),
            dependency.clone(),
            to_version,
            notes.clone(),
            Reason::new(
              "
              There is an existing open pr for our dependency that is not
              already at the version we are updating to.
              ",
            ),
          ));
        } else if pr.head_status == HeadStatus::Behind {
          actions.push(DoupAction::UpdatePR(
            PRNumber::new(pr.number),
            ecosystem_ref.clone(),
            dependency.clone(),
            to_version,
            notes.clone(),
            Reason::new(
              "
              There is an existing open pr for our dependency that is at the
              version we want to update to but it is behind the default branch
              so we will update it in order to keep it on top of default &
              re-trigger it's CI checks.
              ",
            ),
          ));
        } else {
          actions.push(DoupAction::SkipUpdate(
            dependency.clone(),
            to_version,
            Reason::new(
              "
              There is an existing open pr for our dependency that is at the
              same version as we want to update to & is up to date with the
              default branch so there is nothing to do.
              ",
            ),
          ));
        }
      }

      PullRequestState::Closed => {
        if pr.version == to_version.version {
          actions.push(DoupAction::SkipUpdate(
            dependency.clone(),
            to_version,
            Reason::new(
              "
              There is an existing closed pr for our dependency that matches
              the same version that we want to update to. ie: The user elected
              to ignore this version update.
              ",
            ),
          ));
        } else {
          actions.push(DoupAction::CreatePR(
            ecosystem_ref.clone(),
            dependency.clone(),
            to_version,
            notes.clone(),
            Reason::new(
              "
              There is an existing closed pr for our dependency but it does not
              match the version that we want to update to so we will create a
              new PR for the new version.
              ",
            ),
          ));
        }
      }
    }
    claimed
  } else {
    actions.push(DoupAction::CreatePR(
      ecosystem_ref.clone(),
      dependency.clone(),
      to_version,
      notes.clone(),
      Reason::new(
        "
        There is no pre-existing pr for our dependency so we create one.
        ",
      ),
    ));
    None
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn plugin() -> DoupRepoPlugin {
    DoupRepoPlugin { plugin: "./cargo.wasm".to_string(), with: None }
  }

  fn dependency() -> Dependency {
    Dependency {
      name: "serde".to_string(),
      registry: None,
      occurrences: Vec::new(),
      current_version: "1.0.0".to_string(),
    }
  }

  /// Reconciles a single `serde` 1.0.0 -> 1.1.0 candidate against one open PR for it.
  fn reconcile_open_pr(version: &str, head_status: HeadStatus) -> Vec<DoupAction> {
    let (plugin, dependency) = (plugin(), dependency());
    let versions = Versions { next: "1.1.0".to_string(), latest: None };
    let pr = PullRequest {
      number: 7,
      version: version.to_string(),
      head_branch: UpdateBranch::new(&plugin, &dependency, false).to_string(),
      state: PullRequestState::Open,
      head_status,
    };
    reconcile(vec![(&plugin, &dependency, &versions, Vec::new())], vec![pr], Vec::new()).unwrap()
  }

  fn summaries(actions: &[DoupAction]) -> Vec<String> {
    actions.iter().map(DoupAction::summary).collect()
  }

  #[test]
  fn an_up_to_date_pr_at_the_same_version_is_skipped() {
    let actions = reconcile_open_pr("1.1.0", HeadStatus::UpToDate);
    assert!(matches!(actions[..], [DoupAction::SkipUpdate(..)]), "{:?}", summaries(&actions));
  }

  #[test]
  fn a_behind_pr_at_the_same_version_is_updated() {
    let actions = reconcile_open_pr("1.1.0", HeadStatus::Behind);
    assert!(
      matches!(actions[..], [DoupAction::UpdatePR(ref pr, ..)] if *pr.as_ref() == 7),
      "{:?}",
      summaries(&actions)
    );
  }

  #[test]
  fn a_pr_with_foreign_commits_is_skipped_and_kept_open_at_any_version() {
    for version in ["1.1.0", "1.0.5"] {
      let actions = reconcile_open_pr(version, HeadStatus::ForeignCommits);
      assert!(matches!(actions[..], [DoupAction::SkipUpdate(..)]), "{version}: {:?}", summaries(&actions));
    }
  }

  #[test]
  fn a_doup_only_pr_at_another_version_is_updated() {
    for head_status in [HeadStatus::UpToDate, HeadStatus::Behind] {
      let actions = reconcile_open_pr("1.0.5", head_status);
      assert!(matches!(actions[..], [DoupAction::UpdatePR(..)]), "{head_status:?}: {:?}", summaries(&actions));
    }
  }
}
