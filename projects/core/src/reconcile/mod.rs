//! Decides what doup should do to a repo, given what it found there.
//!
//! [`reconcile`] is pure: it takes a [`RepoState`] & returns a list of [`Action`]s, each with the
//! reason it was chosen. Carrying the actions out is someone else's job.

mod action;
mod reason;

pub use action::{Action, PlannedUpdate, PrNumber, Target, TargetKind};
pub use reason::{CloseReason, CreateReason, DeleteReason, SkipReason, UpdateReason};

use doup_config::repo::DoupRepoPlugin;
use doup_git::{DOUP_BRANCH_PREFIX, UpdateBranch};
use doup_plugin_host::ecosystem_plugin::{Dependency, Versions};
use std::collections::{HashMap, HashSet};

/// A dependency discovered in the repo, with the versions it may be updated to.
#[derive(Debug, Clone)]
pub struct Candidate {
  /// The ecosystem plugin that discovered the dependency.
  pub ecosystem: DoupRepoPlugin,

  pub dependency: Dependency,

  /// `None` when the dependency is already up to date. Still worth passing to [`reconcile`], as it
  /// explains why any open pull request for the dependency is closed.
  pub versions: Option<Versions>,
}

/// A pull request doup opened, as it is now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExistingPr {
  pub number: PrNumber,

  /// The version the pull request proposes, eg: "1.2.3", see `pull-request.version` in the WIT.
  pub version: String,

  /// The head branch of the pull request, see [`UpdateBranch`].
  pub head_branch: String,

  pub state: PrState,

  /// Only consulted for [`PrState::Open`] pull requests, use [`HeadStatus::UpToDate`] for closed ones.
  pub head_status: HeadStatus,
}

/// The state of a pull request. Merged pull requests are of no interest, so must never be given as closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrState {
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

/// Everything doup found out about a repo, the input to [`reconcile`].
#[derive(Debug, Clone, Default)]
pub struct RepoState {
  /// Every dependency discovered on the default branch, whose versions could be looked up.
  pub candidates: Vec<Candidate>,

  /// Every open pull request whose head branch starts with [`DOUP_BRANCH_PREFIX`], plus every closed
  /// pull request whose head branch is that of one of the `candidates`' updates.
  pub prs: Vec<ExistingPr>,

  /// The branches on `origin` that doup may delete: those starting with [`DOUP_BRANCH_PREFIX`] that only
  /// doup has committed to. **Any of these may be deleted**, pass an empty list to never delete a branch.
  pub branches: Vec<String>,

  /// Branches doup must leave alone: their pull requests are never closed & they are never deleted.
  /// eg: the branches of a dependency whose versions could not be looked up, as there is no telling
  /// whether its pull requests are still wanted.
  pub protected_branches: HashSet<String>,
}

/// Decides what doup should do about `state`. This function is pure & must stay pure.
///
/// - Every candidate update gets exactly one of [`Action::CreatePr`], [`Action::UpdatePr`] or [`Action::Skip`].
/// - Every open doup pull request that no candidate claims is closed, see [`Action::ClosePr`].
/// - Every branch in [`RepoState::branches`] that no remaining pull request or update uses is deleted,
///   see [`Action::DeleteBranch`].
///
/// Actions are returned in that order.
pub fn reconcile(state: &RepoState) -> Vec<Action> {
  let mut actions = Vec::new();
  let mut claimed_prs: HashSet<PrNumber> = HashSet::new();
  // Every branch a candidate could use, so a pull request on it can be closed with a precise reason.
  let mut known_branches: HashMap<String, (TargetKind, &Candidate)> = HashMap::new();

  for candidate in &state.candidates {
    for kind in [TargetKind::Next, TargetKind::Latest] {
      let branch = UpdateBranch::new(&candidate.ecosystem, &candidate.dependency, kind == TargetKind::Latest);
      known_branches.insert(branch.to_string(), (kind, candidate));
    }
    for update in planned_updates(candidate) {
      let (action, claimed) = reconcile_update(update, &state.prs);
      claimed_prs.extend(claimed);
      actions.push(action);
    }
  }

  let is_protected = |branch: &str| state.protected_branches.contains(branch);

  let mut closing: HashMap<&str, PrNumber> = HashMap::new();
  for pr in &state.prs {
    let unclaimed = pr.state == PrState::Open && !claimed_prs.contains(&pr.number);
    if unclaimed && pr.head_branch.starts_with(DOUP_BRANCH_PREFIX) && !is_protected(&pr.head_branch) {
      let reason = close_reason(known_branches.get(&pr.head_branch).copied());
      actions.push(Action::ClosePr { number: pr.number, head_branch: pr.head_branch.clone(), reason });
      closing.insert(&pr.head_branch, pr.number);
    }
  }

  let in_use: HashSet<String> = state
    .prs
    .iter()
    .filter(|pr| pr.state == PrState::Open && claimed_prs.contains(&pr.number))
    .map(|pr| pr.head_branch.clone())
    .chain(actions.iter().filter_map(Action::pushes_update).map(PlannedUpdate::branch))
    .collect();

  for branch in &state.branches {
    if !branch.starts_with(DOUP_BRANCH_PREFIX) || in_use.contains(branch) || is_protected(branch) {
      continue;
    }
    let closed_pr = || {
      let closed = state.prs.iter().filter(|pr| pr.state == PrState::Closed && pr.head_branch == *branch);
      closed.map(|pr| pr.number).max()
    };
    let reason = match (closing.get(branch.as_str()), closed_pr()) {
      (Some(&number), _) => DeleteReason::PrClosing { number },
      (None, Some(number)) => DeleteReason::PrClosed { number },
      (None, None) => DeleteReason::Orphaned,
    };
    actions.push(Action::DeleteBranch { branch: branch.clone(), reason });
  }

  actions
}

/// The updates worth proposing for `candidate`: none when it's up to date, else its `next` and/or `latest` version.
fn planned_updates(candidate: &Candidate) -> Vec<PlannedUpdate> {
  let Some(versions) = &candidate.versions else {
    return Vec::new();
  };
  let current = &candidate.dependency.current_version;

  // When only a breaking release is newer, `next` is just the version already installed.
  let next = (versions.next != *current).then(|| Target::new(&versions.next, TargetKind::Next));
  // A `latest` that is no newer than `next` would just propose the same update twice.
  let latest = versions
    .latest
    .as_ref()
    .filter(|latest| *latest != current && **latest != versions.next)
    .map(|latest| Target::new(latest, TargetKind::Latest));

  let update = |target: &Target, companion: &Option<Target>| PlannedUpdate {
    ecosystem: candidate.ecosystem.clone(),
    dependency: candidate.dependency.clone(),
    target: target.clone(),
    companion: companion.clone(),
  };
  next.iter().map(|next| update(next, &latest)).chain(latest.iter().map(|latest| update(latest, &next))).collect()
}

/// Decides what to do about one update, returning the open pull request it claims (so it's not closed), if any.
fn reconcile_update(update: PlannedUpdate, prs: &[ExistingPr]) -> (Action, Option<PrNumber>) {
  let branch = update.branch();
  let prs = prs.iter().filter(|pr| pr.head_branch == branch);

  if let Some(pr) = prs.clone().find(|pr| pr.state == PrState::Open) {
    let number = pr.number;
    let action = if pr.head_status == HeadStatus::ForeignCommits {
      Action::Skip { update, reason: SkipReason::ForeignCommits { number } }
    } else if pr.version != update.target.version {
      Action::UpdatePr { number, update, reason: UpdateReason::NewerVersion { previous: pr.version.clone() } }
    } else if pr.head_status == HeadStatus::Behind {
      Action::UpdatePr { number, update, reason: UpdateReason::BehindDefaultBranch }
    } else {
      Action::Skip { update, reason: SkipReason::AlreadyProposed { number } }
    };
    return (action, Some(number));
  }

  // Any closed pull request at this version means it was deliberately ignored, not just the most recent one.
  if let Some(pr) = prs.clone().find(|pr| pr.version == update.target.version) {
    return (Action::Skip { update, reason: SkipReason::PreviouslyClosed { number: pr.number } }, None);
  }

  let reason = match prs.max_by_key(|pr| pr.number) {
    Some(pr) => CreateReason::PreviousPrClosed { number: pr.number, version: pr.version.clone() },
    None => CreateReason::NoExistingPr,
  };
  (Action::CreatePr { update, reason }, None)
}

/// Why an unclaimed open pull request on a branch `known` to belong to a candidate (or not) is closed.
fn close_reason(known: Option<(TargetKind, &Candidate)>) -> CloseReason {
  match known {
    None => CloseReason::DependencyNotFound,
    Some((_, Candidate { versions: None, .. })) => CloseReason::UpToDate,
    // A `next` update would have claimed it, so the dependency is at the newest version within its constraints.
    Some((TargetKind::Next, _)) => CloseReason::NoUpdateWithinConstraints,
    // Likewise, the `latest` version is no longer beyond the `next` one.
    Some((TargetKind::Latest, _)) => CloseReason::LatestWithinConstraints,
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  const ORPHAN: &str = "doup/cargo/next/orphan-1234";

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

  fn versions(next: &str, latest: Option<&str>) -> Option<Versions> {
    Some(Versions { next: next.to_string(), latest: latest.map(str::to_string) })
  }

  fn candidate(versions: Option<Versions>) -> Candidate {
    Candidate { ecosystem: plugin(), dependency: dependency(), versions }
  }

  fn branch(latest: bool) -> String {
    UpdateBranch::new(&plugin(), &dependency(), latest).to_string()
  }

  fn pr(number: i64, version: &str, head_branch: &str, state: PrState, head_status: HeadStatus) -> ExistingPr {
    ExistingPr {
      number: PrNumber::new(number),
      version: version.to_string(),
      head_branch: head_branch.to_string(),
      state,
      head_status,
    }
  }

  fn open(number: i64, version: &str, head_branch: &str) -> ExistingPr {
    pr(number, version, head_branch, PrState::Open, HeadStatus::UpToDate)
  }

  fn closed(number: i64, version: &str, head_branch: &str) -> ExistingPr {
    pr(number, version, head_branch, PrState::Closed, HeadStatus::UpToDate)
  }

  fn state(candidates: Vec<Candidate>, prs: Vec<ExistingPr>, branches: &[&str]) -> RepoState {
    RepoState {
      candidates,
      prs,
      branches: branches.iter().map(|b| b.to_string()).collect(),
      protected_branches: HashSet::new(),
    }
  }

  fn summaries(actions: &[Action]) -> Vec<String> {
    actions.iter().map(ToString::to_string).collect()
  }

  /// Reconciles a single `serde` 1.0.0 -> 1.1.0 candidate against one open PR for it.
  fn reconcile_open_pr(version: &str, head_status: HeadStatus) -> Vec<Action> {
    let pr = pr(7, version, &branch(false), PrState::Open, head_status);
    reconcile(&state(vec![candidate(versions("1.1.0", None))], vec![pr], &[]))
  }

  #[test]
  fn a_candidate_without_any_pr_is_created() {
    let actions = reconcile(&state(vec![candidate(versions("1.1.0", None))], vec![], &[]));
    assert_eq!(summaries(&actions), ["create PR: cargo serde 1.0.0 -> 1.1.0"]);
    assert!(matches!(&actions[0], Action::CreatePr { reason: CreateReason::NoExistingPr, .. }));
  }

  #[test]
  fn a_candidate_with_latest_creates_two_prs_that_know_about_each_other() {
    let actions = reconcile(&state(vec![candidate(versions("1.1.0", Some("2.0.0")))], vec![], &[]));
    assert_eq!(
      summaries(&actions),
      ["create PR: cargo serde 1.0.0 -> 1.1.0", "create PR: cargo serde 1.0.0 -> 2.0.0 (latest)"]
    );
    let companions: Vec<_> = actions.iter().map(|a| a.update().unwrap().companion.clone().unwrap()).collect();
    assert_eq!(companions, [Target::new("2.0.0", TargetKind::Latest), Target::new("1.1.0", TargetKind::Next)]);
  }

  #[test]
  fn only_latest_is_created_when_next_is_the_current_version() {
    let actions = reconcile(&state(vec![candidate(versions("1.0.0", Some("2.0.0")))], vec![], &[]));
    assert_eq!(summaries(&actions), ["create PR: cargo serde 1.0.0 -> 2.0.0 (latest)"]);
    assert_eq!(actions[0].update().unwrap().companion, None);
  }

  #[test]
  fn a_latest_equal_to_next_is_only_proposed_once() {
    let actions = reconcile(&state(vec![candidate(versions("1.1.0", Some("1.1.0")))], vec![], &[]));
    assert_eq!(summaries(&actions), ["create PR: cargo serde 1.0.0 -> 1.1.0"]);
  }

  #[test]
  fn an_up_to_date_pr_at_the_same_version_is_skipped() {
    let actions = reconcile_open_pr("1.1.0", HeadStatus::UpToDate);
    assert!(matches!(&actions[..], [Action::Skip { reason: SkipReason::AlreadyProposed { .. }, .. }]));
  }

  #[test]
  fn a_behind_pr_at_the_same_version_is_updated() {
    let actions = reconcile_open_pr("1.1.0", HeadStatus::Behind);
    assert!(
      matches!(&actions[..], [Action::UpdatePr { number, reason: UpdateReason::BehindDefaultBranch, .. }] if number.get() == 7),
      "{:?}",
      summaries(&actions)
    );
  }

  #[test]
  fn a_pr_with_foreign_commits_is_skipped_and_kept_open_at_any_version() {
    for version in ["1.1.0", "1.0.5"] {
      let actions = reconcile_open_pr(version, HeadStatus::ForeignCommits);
      assert!(
        matches!(&actions[..], [Action::Skip { reason: SkipReason::ForeignCommits { .. }, .. }]),
        "{version}: {:?}",
        summaries(&actions)
      );
    }
  }

  #[test]
  fn a_doup_only_pr_at_another_version_is_updated() {
    for head_status in [HeadStatus::UpToDate, HeadStatus::Behind] {
      let actions = reconcile_open_pr("1.0.5", head_status);
      assert!(
        matches!(&actions[..], [Action::UpdatePr { reason: UpdateReason::NewerVersion { previous }, .. }] if previous == "1.0.5"),
        "{head_status:?}: {:?}",
        summaries(&actions)
      );
    }
  }

  #[test]
  fn a_skipped_pr_is_never_closed_nor_its_branch_deleted() {
    let next = branch(false);
    let prs = vec![open(7, "1.1.0", &next)];
    let actions = reconcile(&state(vec![candidate(versions("1.1.0", None))], prs, &[&next]));
    assert_eq!(actions.len(), 1, "{:?}", summaries(&actions));
  }

  #[test]
  fn a_closed_pr_at_the_same_version_is_skipped() {
    let actions =
      reconcile(&state(vec![candidate(versions("1.1.0", None))], vec![closed(3, "1.1.0", &branch(false))], &[]));
    assert!(
      matches!(&actions[..], [Action::Skip { reason: SkipReason::PreviouslyClosed { number }, .. }] if number.get() == 3)
    );
  }

  #[test]
  fn any_closed_pr_at_the_same_version_is_skipped_not_just_the_first() {
    let next = branch(false);
    let prs = vec![closed(5, "1.0.5", &next), closed(3, "1.1.0", &next)];
    let actions = reconcile(&state(vec![candidate(versions("1.1.0", None))], prs, &[]));
    assert!(
      matches!(&actions[..], [Action::Skip { reason: SkipReason::PreviouslyClosed { number }, .. }] if number.get() == 3)
    );
  }

  #[test]
  fn a_closed_pr_at_another_version_creates_a_new_pr() {
    let next = branch(false);
    let prs = vec![closed(2, "1.0.2", &next), closed(3, "1.0.5", &next)];
    let actions = reconcile(&state(vec![candidate(versions("1.1.0", None))], prs, &[&next]));
    assert!(
      matches!(&actions[..], [Action::CreatePr { reason: CreateReason::PreviousPrClosed { number, version }, .. }]
        if number.get() == 3 && version == "1.0.5"),
      "{:?}",
      summaries(&actions)
    );
  }

  #[test]
  fn an_open_pr_wins_over_closed_ones_on_the_same_branch() {
    let next = branch(false);
    let prs = vec![closed(3, "1.1.0", &next), open(4, "1.0.5", &next)];
    let actions = reconcile(&state(vec![candidate(versions("1.1.0", None))], prs, &[]));
    assert!(
      matches!(&actions[..], [Action::UpdatePr { number, .. }] if number.get() == 4),
      "{:?}",
      summaries(&actions)
    );
  }

  #[test]
  fn the_branch_of_a_closed_pr_is_deleted_once_unused() {
    let next = branch(false);
    let actions =
      reconcile(&state(vec![candidate(versions("1.1.0", None))], vec![closed(3, "1.1.0", &next)], &[&next]));
    assert!(
      matches!(&actions[..], [Action::Skip { .. }, Action::DeleteBranch { branch, reason: DeleteReason::PrClosed { number } }]
        if *branch == next && number.get() == 3),
      "{:?}",
      summaries(&actions)
    );
  }

  #[test]
  fn an_unmatched_open_pr_is_closed_and_its_branch_deleted() {
    let actions = reconcile(&state(vec![], vec![open(9, "0.1.0", ORPHAN)], &[ORPHAN]));
    assert!(
      matches!(&actions[..], [
        Action::ClosePr { number, reason: CloseReason::DependencyNotFound, .. },
        Action::DeleteBranch { branch, reason: DeleteReason::PrClosing { number: closing } },
      ] if number.get() == 9 && branch == ORPHAN && closing == number),
      "{:?}",
      summaries(&actions)
    );
  }

  #[test]
  fn an_unmatched_open_pr_with_a_foreign_branch_is_closed_but_its_branch_kept() {
    // Foreign branches are never passed in `branches`.
    let actions = reconcile(&state(vec![], vec![open(9, "0.1.0", ORPHAN)], &[]));
    assert!(matches!(&actions[..], [Action::ClosePr { .. }]), "{:?}", summaries(&actions));
  }

  #[test]
  fn open_prs_not_made_by_doup_are_ignored() {
    let actions = reconcile(&state(vec![], vec![open(9, "0.1.0", "feature/x")], &["feature/x"]));
    assert!(actions.is_empty(), "{:?}", summaries(&actions));
  }

  #[test]
  fn closing_a_pr_explains_why_its_update_is_gone() {
    let reason = |candidate: Candidate, latest: bool| {
      let actions = reconcile(&state(vec![candidate], vec![open(9, "1.0.5", &branch(latest))], &[]));
      let Some(Action::ClosePr { reason, .. }) = actions.into_iter().find(|a| matches!(a, Action::ClosePr { .. }))
      else {
        panic!("expected the PR to be closed");
      };
      reason
    };
    assert_eq!(reason(candidate(None), false), CloseReason::UpToDate);
    assert_eq!(reason(candidate(None), true), CloseReason::UpToDate);
    assert_eq!(reason(candidate(versions("1.0.0", Some("2.0.0"))), false), CloseReason::NoUpdateWithinConstraints);
    assert_eq!(reason(candidate(versions("2.0.0", None)), true), CloseReason::LatestWithinConstraints);
  }

  #[test]
  fn orphaned_doup_branches_are_deleted_but_others_are_not() {
    let next = branch(false);
    let actions = reconcile(&state(vec![candidate(versions("1.1.0", None))], vec![], &[&next, ORPHAN, "main"]));
    assert!(
      matches!(&actions[..], [Action::CreatePr { .. }, Action::DeleteBranch { branch, reason: DeleteReason::Orphaned }] if branch == ORPHAN),
      "{:?}",
      summaries(&actions)
    );
  }

  #[test]
  fn protected_branches_are_never_closed_or_deleted() {
    let mut state = state(vec![], vec![open(9, "0.1.0", ORPHAN)], &[ORPHAN]);
    state.protected_branches.insert(ORPHAN.to_string());
    assert!(reconcile(&state).is_empty());
  }

  #[test]
  fn reasons_read_as_sentences() {
    let number = PrNumber::new(3);
    let reasons: Vec<String> = vec![
      CreateReason::NoExistingPr.to_string(),
      CreateReason::PreviousPrClosed { number, version: "1.0.5".into() }.to_string(),
      UpdateReason::NewerVersion { previous: "1.0.5".into() }.to_string(),
      UpdateReason::BehindDefaultBranch.to_string(),
      SkipReason::AlreadyProposed { number }.to_string(),
      SkipReason::ForeignCommits { number }.to_string(),
      SkipReason::PreviouslyClosed { number }.to_string(),
      CloseReason::UpToDate.to_string(),
      CloseReason::NoUpdateWithinConstraints.to_string(),
      CloseReason::LatestWithinConstraints.to_string(),
      CloseReason::DependencyNotFound.to_string(),
      DeleteReason::PrClosing { number }.to_string(),
      DeleteReason::PrClosed { number }.to_string(),
      DeleteReason::Orphaned.to_string(),
    ];
    for reason in reasons {
      assert!(reason.ends_with('.') && !reason.contains("  ") && !reason.contains('\n'), "{reason:?}");
    }
    assert!(CreateReason::PreviousPrClosed { number, version: "1.0.5".into() }.to_string().contains("#3"));
  }
}
