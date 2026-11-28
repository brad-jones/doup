//! [`Updater`], the entry point: updates every repo the bot is configured for.

use crate::error::RepoError;
use crate::plugins::{PluginLoader, WasmPlugins};
use crate::repo::RepoJob;
use crate::report::{RepoReport, UpdateReport};
use doup_config::bot::{Concurrency, DoupBotConfigFile, DoupBotRepo, GitBackendKind};
use doup_git::{Git, GitCli, Grit, redact_url};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tracing::{Instrument, info, info_span};

/// Updates the dependencies of a list of repos, opening, updating & closing pull requests as needed.
///
/// ```no_run
/// use doup_config::{bot::DoupBotConfigFile, file::ConfigFile};
/// use doup_core::Updater;
///
/// # async fn run() -> Result<(), doup_config::ConfigError> {
/// let config = DoupBotConfigFile::load(None, None)?.merged;
/// let report = Updater::from_config(&config).dry_run(true).run().await;
/// for repo in report.repos() {
///   for action in repo.actions() {
///     println!("{}: {} ({})", repo.origin(), action.action(), action.action().reason());
///   }
/// }
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct Updater<L = WasmPlugins> {
  git: Git,
  repos: Vec<DoupBotRepo>,
  concurrency: Concurrency,
  dry_run: bool,
  plugins: L,
}

impl Updater<WasmPlugins> {
  /// Updates `repos` using `git`, with the default [`Concurrency`] & WASM plugins.
  pub fn new(git: Git, repos: Vec<DoupBotRepo>) -> Self {
    Self { git, repos, concurrency: Concurrency::default(), dry_run: false, plugins: WasmPlugins::default() }
  }

  /// Updates the repos in `config`, with its git backend & concurrency.
  pub fn from_config(config: &DoupBotConfigFile) -> Self {
    let git = match config.git_backend {
      GitBackendKind::GitCli => Git::new(GitCli),
      GitBackendKind::Grit => Git::new(Grit),
    };
    Self::new(git, config.repos.clone()).concurrency(config.concurrency)
  }
}

impl<L: PluginLoader> Updater<L> {
  /// When set, works out what to do to every repo without doing any of it.
  /// Repos are still cloned & plugins still run, but nothing is pushed & no pull request is touched.
  pub fn dry_run(mut self, dry_run: bool) -> Self {
    self.dry_run = dry_run;
    self
  }

  pub fn concurrency(mut self, concurrency: Concurrency) -> Self {
    self.concurrency = concurrency;
    self
  }

  /// Loads plugins with `plugins` rather than as WASM modules.
  pub fn plugins<M: PluginLoader>(self, plugins: M) -> Updater<M> {
    let Self { git, repos, concurrency, dry_run, .. } = self;
    Updater { git, repos, concurrency, dry_run, plugins }
  }

  /// Updates every repo, up to [`Concurrency::repos`] at once.
  ///
  /// Never fails as a whole: every error is confined to the repo, dependency or action it hit & recorded
  /// in the report, so one bad repo never stops the others. Must be called within a tokio runtime.
  pub async fn run(self) -> UpdateReport {
    let Self { git, repos, concurrency, dry_run, plugins } = self;
    info!(repos = repos.len(), dry_run, "updating repos");

    let plugins = Arc::new(plugins);
    let repo_permits = Arc::new(Semaphore::new(concurrency.repos.get()));
    let apply_permits = Arc::new(Semaphore::new(concurrency.apply_updates.get()));

    let mut tasks = JoinSet::new();
    let mut jobs = HashMap::new();
    for (index, repo) in repos.into_iter().enumerate() {
      let origin = redact_url(&repo.origin);
      let span = info_span!("repo", %origin);
      let job = RepoJob {
        index,
        repo,
        origin: origin.clone(),
        git: git.clone(),
        plugins: plugins.clone(),
        dry_run,
        plugin_calls: concurrency.plugin_calls,
        apply_permits: apply_permits.clone(),
      };
      let repo_permits = repo_permits.clone();
      let task = tasks.spawn(
        async move {
          let _permit = repo_permits.acquire_owned().await.ok();
          job.run().await
        }
        .instrument(span),
      );
      jobs.insert(task.id(), (index, origin));
    }

    let mut reports: Vec<(usize, RepoReport)> = Vec::with_capacity(jobs.len());
    while let Some(joined) = tasks.join_next_with_id().await {
      let id = match &joined {
        Ok((id, _)) => *id,
        Err(err) => err.id(),
      };
      let Some((index, origin)) = jobs.remove(&id) else {
        continue;
      };
      let report = joined.map(|(_, report)| report).unwrap_or_else(|err| {
        let mut report = RepoReport::new(origin);
        report.error = Some(RepoError::Panicked(err));
        report
      });
      reports.push((index, report));
    }
    reports.sort_by_key(|(index, _)| *index);
    let report = UpdateReport { repos: reports.into_iter().map(|(_, report)| report).collect() };
    info!(failed = report.failed_repos().count(), "updated repos");
    report
  }
}
