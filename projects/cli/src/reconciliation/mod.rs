use std::collections::HashSet;

use anyhow::Result;
use doup_config::repo::DoupRepoPlugin;
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

  /// The head branch of the PR - eg: `doup/{{ecosystem}}/{{name}}`
  /// [`reconcile`] only expects branches starting with the `doup/` prefix.
  /// Do not include any arbitrary pull request in the list.
  /// For more details on the format see: [`doup_branch`]
  pub head_branch: String,

  /// The state of the PR, we only care about Open & Closed PRs.
  ///
  /// Merged PRs are meaningless to [`reconcile`] and are just extra noise,
  /// don't bundle merged PRs under [`PullRequestState::Closed`].
  ///
  /// When fetching closed PRs only include PRs whose `head_branch` matches one of our `candidates`.
  pub state: PullRequestState,
}

#[derive(PartialEq)]
pub enum PullRequestState {
  Open,
  Closed,
}

// TODO: We need to use some sort of slugify func over these strings to make
// them safe for use in a branch name, possibly also include a small hash of
// all the relevant data if we can not reliably keep full fidelity.
pub fn doup_branch(plugin: &DoupRepoPlugin, dependency: &Dependency, latest: bool) -> String {
  if let Some(ref reg) = dependency.registry {
    if latest {
      return format!("doup/{}/{}/{}_latest", plugin.plugin, reg, dependency.name);
    }
    return format!("doup/{}/{}/{}", plugin.plugin, reg, dependency.name);
  }
  if latest {
    return format!("doup/{}/{}_latest", plugin.plugin, dependency.name);
  }
  format!("doup/{}/{}", plugin.plugin, dependency.name)
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
///   Use [`doup_branch`] to build a branch name for a given dependency.
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
  for (ecosystem_ref, dependency, versions, ref notes) in candidates {
    reconcile_candidate(
      &mut actions,
      (ToVersion { version: versions.next.clone(), latest: false }, ecosystem_ref, dependency, notes),
      &existing_prs,
    );

    // Every candidate effectively has the option for a BREAKING CHANGE candidate with-in it.
    if let Some(ref latest) = versions.latest {
      reconcile_candidate(
        &mut actions,
        (ToVersion { version: latest.clone(), latest: true }, ecosystem_ref, dependency, notes),
        &existing_prs,
      );
    }
  }

  // Now find any old PRs that we can close
  let unmatched_open_prs: Vec<&PullRequest> = existing_prs
    .iter()
    .filter(|pr| pr.head_branch.starts_with("doup/"))
    .filter(|pr| pr.state == PullRequestState::Open)
    .filter(|pr| {
      !actions
        .iter()
        .any(|action| matches!(action, DoupAction::UpdatePR(updated, ..) if *updated.as_ref() == pr.number))
    })
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
        Some(doup_branch(plugin, dependency, to_version.latest))
      }
      _ => None,
    }))
    .collect();

  let orphaned_branches: Vec<String> = existing_branches
    .into_iter()
    .filter(|branch| branch.starts_with("doup/"))
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
) {
  let (to_version, ecosystem_ref, dependency, notes) = candidate;

  if let Some(pr) =
    existing_prs.iter().find(|pr| pr.head_branch == doup_branch(ecosystem_ref, dependency, to_version.latest))
  {
    match pr.state {
      PullRequestState::Open => {
        actions.push(DoupAction::UpdatePR(
          PRNumber::new(pr.number),
          ecosystem_ref.clone(),
          dependency.clone(),
          to_version,
          notes.clone(),
          Reason::new(
            "
            There is an existing open pr for our dependency so we update it
            regardless of what version the current PR is updating to.
            ",
          ),
        ));
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
  }
}
