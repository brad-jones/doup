//! Carries out the [`Action`]s planned for a repo.
//!
//! Pull requests are created & updated first, concurrently, then pull requests are closed, then branches
//! deleted. That way a pull request is never closed before the one replacing it is open, and a branch is
//! only deleted once its pull request is closed.

use super::{Plugins, SharedRepo};
use crate::content;
use crate::error::ActionError;
use crate::plugins::{Ecosystem, Platform};
use crate::reconcile::{Action, CloseReason, DeleteReason, PlannedUpdate, PrNumber, TargetKind, UpdateReason};
use crate::report::{ActionOutcome, ActionReport};
use doup_plugin_host::ecosystem_plugin::ReleaseNote;
use futures::future::join_all;
use std::collections::{HashMap, HashSet};
use tokio::sync::{OnceCell, Semaphore};
use tracing::{Instrument, debug, error, info, info_span, warn};

/// Identifies a dependency's release notes: its ecosystem plugin, name, registry & the version fetched up to.
type NotesKey = (String, String, Option<String>, String);

pub(super) struct Executor<'a, P, E> {
  repo: &'a SharedRepo,
  plugins: &'a Plugins<P, E>,
  base_branch: &'a str,
  /// Bounds this repo's concurrent plugin calls.
  plugin_permits: &'a Semaphore,
  /// Bounds the concurrent `apply_updates` calls of every repo.
  apply_permits: &'a Semaphore,
  /// Release notes are fetched at most once per dependency, by whichever of its pull requests gets there first.
  notes: HashMap<NotesKey, OnceCell<Option<Vec<ReleaseNote>>>>,
}

impl<'a, P: Platform, E: Ecosystem> Executor<'a, P, E> {
  pub(super) fn new(
    repo: &'a SharedRepo,
    plugins: &'a Plugins<P, E>,
    base_branch: &'a str,
    plugin_permits: &'a Semaphore,
    apply_permits: &'a Semaphore,
    actions: &[Action],
  ) -> Self {
    let notes =
      actions.iter().filter_map(Action::pushes_update).map(|update| (notes_key(update), OnceCell::new())).collect();
    Self { repo, plugins, base_branch, plugin_permits, apply_permits, notes }
  }

  /// Carries out `actions`, returning each with its outcome, in the order given.
  pub(super) async fn run(&self, actions: Vec<Action>) -> Vec<ActionReport> {
    let mut outcomes: Vec<Option<ActionOutcome>> = actions.iter().map(|_| None).collect();

    let pushes = actions.iter().enumerate().filter_map(|(index, action)| {
      let push = match action {
        Action::CreatePr { update, .. } => self.push_pr(update, None, None),
        Action::UpdatePr { number, update, reason } => self.push_pr(update, Some(*number), Some(reason)),
        _ => return None,
      };
      Some(indexed(index, self.report(push).instrument(span(action))))
    });
    for (index, outcome) in join_all(pushes).await {
      outcomes[index] = Some(outcome);
    }

    let closes = actions.iter().enumerate().filter_map(|(index, action)| match action {
      Action::ClosePr { number, reason, .. } => {
        Some(indexed(index, self.report(self.close_pr(*number, reason)).instrument(span(action))))
      }
      _ => None,
    });
    let mut failed_closes = HashSet::new();
    for (index, outcome) in join_all(closes).await {
      if outcome.is_failure()
        && let Action::ClosePr { number, .. } = &actions[index]
      {
        failed_closes.insert(*number);
      }
      outcomes[index] = Some(outcome);
    }

    // Git runs one operation at a time anyway, see `SharedRepo`.
    for (index, action) in actions.iter().enumerate() {
      let Action::DeleteBranch { branch, reason } = action else {
        continue;
      };
      let outcome = match reason {
        // Deleting the branch would close the pull request without the comment explaining why.
        DeleteReason::PrClosing { number } if failed_closes.contains(number) => {
          warn!(%branch, "not deleting the branch, as closing its pull request failed");
          ActionOutcome::NotAttempted
        }
        _ => self.report(self.delete_branch(branch)).instrument(span(action)).await,
      };
      outcomes[index] = Some(outcome);
    }

    actions
      .into_iter()
      .zip(outcomes)
      .map(|(action, outcome)| ActionReport { outcome: outcome.unwrap_or(ActionOutcome::Skipped), action })
      .collect()
  }

  /// Turns the result of an action into its outcome, logging it.
  async fn report(&self, action: impl Future<Output = Result<Option<String>, ActionError>>) -> ActionOutcome {
    match action.await {
      Ok(url) => {
        info!(url = url.as_deref().unwrap_or_default(), "done");
        ActionOutcome::Done { url }
      }
      Err(err) => {
        error!(error = &err as &dyn std::error::Error, "failed");
        ActionOutcome::Failed(err)
      }
    }
  }

  /// Builds `update` on its branch, then opens a pull request for it, or updates `existing`.
  async fn push_pr(
    &self,
    update: &PlannedUpdate,
    existing: Option<PrNumber>,
    reason: Option<&UpdateReason>,
  ) -> Result<Option<String>, ActionError> {
    let ecosystem = self.ecosystem(update)?;
    let branch = update.branch();
    let (notes, pushed) =
      tokio::join!(self.release_notes(ecosystem, update), self.push_branch(ecosystem, update, &branch));
    pushed?;

    let content = content::pr_content(self.base_branch, update, notes.as_deref(), reason);
    let _permit = self.plugin_permits.acquire().await.ok();
    let platform = &self.plugins.platform;
    let pr = match existing {
      None => platform.create_pr(&branch, &content).await.map_err(|err| ActionError::CreatePr(err.into()))?,
      Some(number) => platform
        .update_pr(number.get(), &content)
        .await
        .map_err(|source| ActionError::UpdatePr { number, source: source.into() })?,
    };
    Ok(Some(pr.url))
  }

  /// Applies `update` to a fresh checkout of the default branch, commits it to `branch` & pushes it to `origin`.
  async fn push_branch(&self, ecosystem: &E, update: &PlannedUpdate, branch: &str) -> Result<(), ActionError> {
    let checkout = |source| ActionError::Checkout { branch: branch.to_string(), source: Box::new(source) };
    let worktree = self.repo.locked(self.repo.get().worktree_add(branch)).await.map_err(checkout)?;

    {
      let _permit = self.apply_permits.acquire().await.ok();
      debug!(path = %worktree.path().display(), "applying the update");
      let (dependency, version) = (&update.dependency, &update.target.version);
      ecosystem
        .apply_updates(worktree.path(), dependency, version)
        .await
        .map_err(|err| ActionError::ApplyUpdate(err.into()))?;
    }

    let message = content::commit_message(update);
    self
      .repo
      .locked(async {
        worktree.commit(&message).await.map_err(|err| ActionError::Commit(err.into()))?;
        // The branch is always rebuilt from the default branch, so it is force pushed, even for a new pull
        // request: the branch may already exist on `origin` (eg: that of a closed pull request, or a previous
        // run pushed it but failed to open the pull request), and a fast-forward only push would be rejected.
        // The lease (origin's tip when the worktree was added) still stops us clobbering concurrent pushes.
        worktree
          .push_force()
          .await
          .map_err(|source| ActionError::Push { branch: branch.to_string(), source: source.into() })
      })
      .await?;
    debug!("pushed the update");

    // Everything that matters has been pushed, and the worktree is cleaned up with the clone regardless.
    if let Err(err) = self.repo.locked(worktree.remove()).await {
      warn!(error = &err as &dyn std::error::Error, "failed to remove the worktree");
    }
    Ok(())
  }

  /// The release notes of `update`'s dependency, `None` when they could not be fetched.
  async fn release_notes(&self, ecosystem: &E, update: &PlannedUpdate) -> &Option<Vec<ReleaseNote>> {
    static NONE: Option<Vec<ReleaseNote>> = None;
    let key = notes_key(update);
    let Some(cell) = self.notes.get(&key) else {
      return &NONE;
    };
    cell
      .get_or_init(|| async {
        let _permit = self.plugin_permits.acquire().await.ok();
        match ecosystem.fetch_release_notes(&update.dependency, &key.3).await {
          Ok(notes) => {
            debug!(count = notes.len(), "fetched release notes");
            Some(notes)
          }
          Err(err) => {
            warn!(error = &err as &dyn std::error::Error, "failed to fetch release notes, carrying on without them");
            None
          }
        }
      })
      .await
  }

  async fn close_pr(&self, number: PrNumber, reason: &CloseReason) -> Result<Option<String>, ActionError> {
    let comment = content::close_comment(reason);
    let _permit = self.plugin_permits.acquire().await.ok();
    let pr = self
      .plugins
      .platform
      .close_pr(number.get(), Some(&comment))
      .await
      .map_err(|source| ActionError::ClosePr { number, source: source.into() })?;
    Ok(Some(pr.url))
  }

  async fn delete_branch(&self, branch: &str) -> Result<Option<String>, ActionError> {
    let deleted = self.repo.locked(self.repo.get().delete_remote_branch(branch)).await;
    deleted.map_err(|source| ActionError::DeleteBranch { branch: branch.to_string(), source: source.into() })?;
    Ok(None)
  }

  fn ecosystem(&self, update: &PlannedUpdate) -> Result<&E, ActionError> {
    let found = self.plugins.ecosystems.iter().find(|(plugin, _)| *plugin == update.ecosystem);
    found
      .map(|(_, plugin)| plugin)
      .ok_or_else(|| ActionError::UnknownEcosystem { plugin: update.ecosystem.plugin.clone() })
  }
}

/// The key of `update`'s release notes. Both of a dependency's updates share notes fetched up to the newest of them,
/// each then only shows those up to its own target.
fn notes_key(update: &PlannedUpdate) -> NotesKey {
  let to = match &update.companion {
    Some(companion) if companion.kind == TargetKind::Latest => &companion.version,
    _ => &update.target.version,
  };
  let dependency = &update.dependency;
  (update.ecosystem.plugin.clone(), dependency.name.clone(), dependency.registry.clone(), to.clone())
}

fn span(action: &Action) -> tracing::Span {
  info_span!("action", %action)
}

/// Pairs the output of `future` with `index`.
async fn indexed<F: Future>(index: usize, future: F) -> (usize, F::Output) {
  (index, future.await)
}
