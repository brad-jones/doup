use doup_git::ClonedRepo;
use std::future::Future;
use tokio::sync::Mutex;

/// A [`ClonedRepo`] whose git operations are run one at a time.
///
/// Git isn't built for concurrent writes to one clone: fetches (especially shallow ones) & worktree
/// bookkeeping take locks that fail, rather than wait, when contended. So everything that touches the
/// clone goes through [`Self::locked`], while plugin calls (the slow part) run concurrently outside it.
pub(super) struct SharedRepo {
  repo: ClonedRepo,
  lock: Mutex<()>,
}

impl SharedRepo {
  pub(super) fn new(repo: ClonedRepo) -> Self {
    Self { repo, lock: Mutex::new(()) }
  }

  pub(super) fn get(&self) -> &ClonedRepo {
    &self.repo
  }

  /// Runs `operation` once no other git operation on this clone is running.
  pub(super) async fn locked<F: Future>(&self, operation: F) -> F::Output {
    let _guard = self.lock.lock().await;
    operation.await
  }
}
