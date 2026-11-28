//! Updates one repo: clone it, find its dependencies & their updates, work out what to do, then do it.

mod execute;
mod shared_repo;

use crate::error::{DependencyError, RepoError};
use crate::plugins::{Ecosystem, LoadContext, Platform, PluginLoader};
use crate::reconcile::{Action, Candidate, ExistingPr, HeadStatus, PrNumber, PrState, RepoState, reconcile};
use crate::report::{ActionOutcome, ActionReport, RepoReport};
use doup_config::bot::{DoupBotRepo, DoupBotRepoAuth};
use doup_config::file::ConfigFile;
use doup_config::lock::{DoupLockFile, LOCK_FILE_NAME};
use doup_config::repo::{DoupRepoConfigFile, DoupRepoPlugin};
use doup_git::{Auth, CloneOptions, DOUP_BRANCH_PREFIX, Git, GitError, UpdateBranch};
use doup_plugin_host::ecosystem_plugin::Dependency;
use execute::Executor;
use futures::future::{join_all, try_join_all};
use shared_repo::SharedRepo;
use std::collections::{HashMap, HashSet};
use std::num::{NonZeroU32, NonZeroUsize};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tracing::{Instrument, debug, error, info, info_span, warn};

/// Everything needed to update one of the bot's repos.
pub(crate) struct RepoJob<L> {
  /// The repo's position in the bot config, which scopes its env overrides.
  pub(crate) index: usize,
  pub(crate) repo: DoupBotRepo,
  pub(crate) origin: String,
  pub(crate) git: Git,
  pub(crate) plugins: Arc<L>,
  pub(crate) dry_run: bool,
  pub(crate) plugin_calls: NonZeroUsize,
  /// Shared by every repo, as applying updates may run containers.
  pub(crate) apply_permits: Arc<Semaphore>,
}

/// The plugins of one repo. Ecosystems are kept in config order.
struct Plugins<P, E> {
  platform: P,
  ecosystems: Vec<(DoupRepoPlugin, E)>,
}

impl<L: PluginLoader> RepoJob<L> {
  /// Updates the repo, never failing: any error is recorded in the report instead.
  pub(crate) async fn run(self) -> RepoReport {
    let mut report = RepoReport::new(self.origin.clone());
    match self.update(&mut report).await {
      Ok(()) if report.is_success() => info!("repo updated"),
      Ok(()) => warn!("repo updated, with errors"),
      Err(err) => {
        error!(error = &err as &dyn std::error::Error, "failed to update repo");
        report.error = Some(err);
      }
    }
    report
  }

  async fn update(&self, report: &mut RepoReport) -> Result<(), RepoError> {
    let auth = clone_auth(&self.repo)?;
    // Deleted when dropped, which takes the clone & every worktree in it along.
    let dir = tempfile::Builder::new().prefix("doup-repo-").tempdir().map_err(RepoError::TempDir)?;
    let workspace = dir.path();

    let mut options = CloneOptions::new(&self.repo.origin, workspace).with_depth(NonZeroU32::MIN);
    if let Some(auth) = auth {
      options = options.with_auth(auth);
    }
    info!(path = %workspace.display(), "cloning");
    let repo = SharedRepo::new(self.git.clone_repo(&options).await.map_err(|err| RepoError::Clone(err.into()))?);

    let env_prefix = DoupRepoConfigFile::repo_env_prefix(self.index);
    let loaded = DoupRepoConfigFile::load(Some(workspace), Some(&env_prefix)).map_err(RepoError::Config)?;
    let context = load_context(loaded.file.as_ref(), workspace)?;
    let config = loaded.merged;

    // Bounds this repo's concurrent plugin calls.
    let plugin_permits = Semaphore::new(self.plugin_calls.get());

    // Every plugin is loaded before any work starts, so a bad config or module fails fast.
    let plugins = self.load_plugins(&config, &context).await?;
    let dependencies = discover(&plugins, workspace).await?;
    let (candidates, protected_branches) = self.lookup_versions(&plugins, dependencies, &plugin_permits, report).await;
    let observed = self.observe(&repo, &plugins.platform, candidates, protected_branches).await?;

    let actions = reconcile(&observed.state);
    info!(count = actions.len(), "planned actions");
    for action in &actions {
      info!(action = %action, reason = %action.reason(), "planned");
    }

    report.actions = match observed.base_branch {
      // A dry run never reads the base branch, it's only needed to carry the actions out.
      None => actions.into_iter().map(|action| ActionReport { outcome: planned(&action), action }).collect(),
      Some(base_branch) => {
        let executor = Executor::new(&repo, &plugins, &base_branch, &plugin_permits, &self.apply_permits, &actions);
        executor.run(actions).await
      }
    };
    Ok(())
  }

  async fn load_plugins(
    &self,
    config: &DoupRepoConfigFile,
    context: &LoadContext,
  ) -> Result<Plugins<L::Platform, L::Ecosystem>, RepoError> {
    let platform = async {
      debug!(plugin = %config.platform.plugin, "loading platform plugin");
      self.plugins.load_platform(&config.platform, context).await.map_err(|err| RepoError::LoadPlatform(err.into()))
    };
    let ecosystems = try_join_all(config.ecosystems.iter().map(|ecosystem| async move {
      debug!(plugin = %ecosystem.plugin, "loading ecosystem plugin");
      let plugin =
        self.plugins.load_ecosystem(ecosystem, context).await.map_err(|err| RepoError::LoadEcosystem(err.into()))?;
      Ok((ecosystem.clone(), plugin))
    }));
    let (platform, ecosystems) = tokio::try_join!(platform, ecosystems)?;
    info!(ecosystems = ecosystems.len(), "loaded plugins");
    Ok(Plugins { platform, ecosystems })
  }

  /// Looks up the versions of every dependency, concurrently.
  ///
  /// A failed lookup is recorded in `report` and the dependency's branches are returned as protected,
  /// so that its pull requests aren't closed just because its versions are unknown.
  async fn lookup_versions<P>(
    &self,
    plugins: &Plugins<P, L::Ecosystem>,
    dependencies: Vec<(usize, Dependency)>,
    plugin_permits: &Semaphore,
    report: &mut RepoReport,
  ) -> (Vec<Candidate>, HashSet<String>) {
    let lookups = join_all(dependencies.into_iter().map(|(index, dependency)| {
      let (ecosystem, plugin) = &plugins.ecosystems[index];
      let span = info_span!("dependency", name = %dependency.name, ecosystem = ecosystem.name());
      async move {
        let _permit = plugin_permits.acquire().await.ok();
        let versions = plugin.lookup_versions(&dependency).await;
        (ecosystem, dependency, versions)
      }
      .instrument(span)
    }))
    .await;

    let mut candidates = Vec::new();
    let mut protected = HashSet::new();
    for (ecosystem, dependency, versions) in lookups {
      let _span = info_span!("dependency", name = %dependency.name, ecosystem = ecosystem.name()).entered();
      match versions {
        Ok(versions) => {
          match &versions {
            Some(v) => {
              info!(current = %dependency.current_version, next = %v.next, latest = ?v.latest, "update available")
            }
            None => debug!(current = %dependency.current_version, "up to date"),
          }
          candidates.push(Candidate { ecosystem: ecosystem.clone(), dependency, versions });
        }
        Err(source) => {
          let err = DependencyError::LookupVersions {
            ecosystem: ecosystem.name().to_string(),
            dependency: dependency.name.clone(),
            source: source.into(),
          };
          error!(error = &err as &dyn std::error::Error, "leaving the dependency's pull requests alone");
          protected.extend([false, true].map(|latest| UpdateBranch::new(ecosystem, &dependency, latest).to_string()));
          report.dependency_errors.push(err);
        }
      }
    }
    (candidates, protected)
  }

  /// Gathers the state of the repo's branches & pull requests, concurrently.
  async fn observe(
    &self,
    repo: &SharedRepo,
    platform: &L::Platform,
    candidates: Vec<Candidate>,
    protected_branches: HashSet<String>,
  ) -> Result<Observed, RepoError> {
    // Closed pull requests only matter when they're for an update that is available now.
    let candidate_branches: Vec<String> = candidates
      .iter()
      .filter_map(|candidate| Some((candidate, candidate.versions.as_ref()?)))
      .flat_map(|(candidate, versions)| {
        let latest = versions.latest.is_some().then_some(true);
        [Some(false), latest]
          .into_iter()
          .flatten()
          .map(|latest| UpdateBranch::new(&candidate.ecosystem, &candidate.dependency, latest).to_string())
      })
      .collect();

    let open_prs =
      async { platform.list_open_prs(DOUP_BRANCH_PREFIX).await.map_err(|err| RepoError::ListOpenPrs(err.into())) };
    let closed_prs = async {
      if candidate_branches.is_empty() {
        return Ok(Vec::new());
      }
      platform.list_closed_prs(&candidate_branches).await.map_err(|err| RepoError::ListClosedPrs(err.into()))
    };
    let base_branch = async {
      if self.dry_run {
        return Ok(None);
      }
      repo.locked(repo.get().default_branch()).await.map(Some).map_err(|err| RepoError::DefaultBranch(err.into()))
    };
    let ((branches, head_statuses), open_prs, closed_prs, base_branch) =
      tokio::try_join!(inspect_branches(repo), open_prs, closed_prs, base_branch)?;
    info!(branches = branches.len(), open_prs = open_prs.len(), closed_prs = closed_prs.len(), "found existing work");

    let open = open_prs.into_iter().map(|pr| ExistingPr {
      number: PrNumber::new(pr.number),
      version: pr.version,
      // A branch we know nothing about is assumed up to date, so it is never needlessly rebuilt.
      head_status: head_statuses.get(&pr.head_branch).copied().unwrap_or(HeadStatus::UpToDate),
      head_branch: pr.head_branch,
      state: PrState::Open,
    });
    let closed = closed_prs.into_iter().map(|pr| ExistingPr {
      number: PrNumber::new(pr.number),
      version: pr.version,
      head_branch: pr.head_branch,
      state: PrState::Closed,
      head_status: HeadStatus::UpToDate,
    });
    let state = RepoState { candidates, prs: open.chain(closed).collect(), branches, protected_branches };
    Ok(Observed { state, base_branch })
  }
}

/// What [`RepoJob::observe`] found.
struct Observed {
  state: RepoState,
  /// The branch pull requests are opened against, `None` on a dry run.
  base_branch: Option<String>,
}

/// The outcome of an action that is not carried out.
fn planned(action: &Action) -> ActionOutcome {
  match action {
    Action::Skip { .. } => ActionOutcome::Skipped,
    _ => ActionOutcome::Planned,
  }
}

/// Where the plugins of the repo cloned to `workspace` are found: relative module paths in the workspace &
/// package plugins pinned by the `doup.lock` next to its config file. A missing lock file is treated as
/// empty, so only package plugins that need it fail.
fn load_context(file: Option<&DoupRepoConfigFile>, workspace: &Path) -> Result<LoadContext, RepoError> {
  let lock_file = file.and_then(DoupRepoConfigFile::lock_path).unwrap_or_else(|| workspace.join(LOCK_FILE_NAME));
  let lock = DoupLockFile::load(&lock_file).map_err(RepoError::Config)?.unwrap_or_default();
  debug!(path = %lock_file.display(), packages = lock.packages().len(), "loaded lock file");
  Ok(LoadContext {
    search_dirs: vec![workspace.to_path_buf()],
    lock: Arc::new(lock),
    // Relative to the repo, as the clone is temporary.
    lock_file: lock_file.strip_prefix(workspace).map(Path::to_path_buf).unwrap_or(lock_file),
  })
}

fn clone_auth(repo: &DoupBotRepo) -> Result<Option<Auth>, RepoError> {
  Ok(match &repo.auth {
    None => None,
    Some(DoupBotRepoAuth::Https { user, pass }) => Some(Auth::Https { user: user.clone(), pass: pass.clone() }),
    Some(DoupBotRepoAuth::Ssh { key: Some(key), .. }) => Some(Auth::Ssh { key: key.clone() }),
    Some(DoupBotRepoAuth::Ssh { key: None, .. }) => return Err(RepoError::SshKeyNotLoaded),
  })
}

/// Lists every dependency in `workspace`, each with the index of the ecosystem that found it.
async fn discover<P, E: Ecosystem>(
  plugins: &Plugins<P, E>,
  workspace: &Path,
) -> Result<Vec<(usize, Dependency)>, RepoError> {
  let discovered = try_join_all(plugins.ecosystems.iter().enumerate().map(|(index, (ecosystem, plugin))| {
    async move {
      let dependencies = plugin.discover_deps(workspace).await.map_err(|err| RepoError::DiscoverDeps(err.into()))?;
      info!(count = dependencies.len(), "discovered dependencies");
      Ok::<_, RepoError>(dependencies.into_iter().map(move |dependency| (index, dependency)))
    }
    .instrument(info_span!("ecosystem", name = ecosystem.name()))
  }))
  .await?;
  Ok(discovered.into_iter().flatten().collect())
}

/// Lists the doup branches on `origin` that doup may rebuild or delete, along with the status of every doup branch.
///
/// A branch that someone other than doup has committed to is never returned, so it is never deleted, and its
/// status is [`HeadStatus::ForeignCommits`], so it is never rebuilt.
async fn inspect_branches(repo: &SharedRepo) -> Result<(Vec<String>, HashMap<String, HeadStatus>), RepoError> {
  repo
    .locked(async {
      let cloned = repo.get();
      let mut branches = Vec::new();
      let mut statuses = HashMap::new();
      let all = cloned.list_remote_branches().await.map_err(|err| RepoError::ListBranches(err.into()))?;
      for branch in all.into_iter().filter(|branch| branch.starts_with(DOUP_BRANCH_PREFIX)) {
        let status = match cloned.branch_status(&branch).await {
          Ok(status) => status,
          // Deleted since it was listed, so there's nothing left to inspect.
          Err(GitError::BranchNotFound { .. }) => continue,
          Err(source) => return Err(RepoError::InspectBranch { branch, source: source.into() }),
        };
        // A branch with no authors has no commits of its own (eg: it was merged), so it's safe to delete.
        let only_doup = status.authors().iter().all(|author| author == cloned.identity().email());
        let head_status = match (only_doup, status.is_behind()) {
          (false, _) => HeadStatus::ForeignCommits,
          (true, true) => HeadStatus::Behind,
          (true, false) => HeadStatus::UpToDate,
        };
        debug!(%branch, ?head_status, "inspected branch");
        statuses.insert(branch.clone(), head_status);
        if only_doup {
          branches.push(branch);
        }
      }
      Ok((branches, statuses))
    })
    .await
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn load_context_reads_the_lock_next_to_the_config() {
    let workspace = tempfile::tempdir().unwrap();
    let dir = workspace.path().join("sub");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("doup.yaml"), "platform: { use: doup:github@1 }\necosystems: [{ use: doup:cargo@1 }]\n")
      .unwrap();
    let lock = "version = 1\n[[packages]]\nname = \"doup:github\"\n[[packages.versions]]\nrequirement = \"^1\"\nversion = \"1.0.0\"\ndigest = \"sha256:00\"\nsize = 1\n";
    std::fs::write(dir.join("doup.lock"), lock).unwrap();

    let config = DoupRepoConfigFile::from_file(&dir.join("doup.yaml")).unwrap();
    let context = load_context(Some(&config), workspace.path()).unwrap();
    assert_eq!(context.lock_file, Path::new("sub/doup.lock"));
    assert_eq!(context.lock.packages()[0].name, "doup:github");
    assert_eq!(context.search_dirs, [workspace.path()]);

    // Without a config file, eg: env only, an absent lock is simply empty.
    let context = load_context(None, workspace.path()).unwrap();
    assert_eq!(context.lock_file, Path::new("doup.lock"));
    assert!(context.lock.is_empty());
  }
}
