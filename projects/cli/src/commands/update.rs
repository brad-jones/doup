use crate::reconciliation::{DoupAction, HeadStatus, PullRequest, PullRequestState, ToVersion, reconcile};
use anyhow::{Result, bail};
use clap::Args;
use doup_config::{
  bot::{DoupBotConfigFile, DoupBotRepoAuth, GitBackendKind},
  file::ConfigFile,
  repo::DoupRepoConfigFile,
};
use doup_git::{Auth, CloneOptions, ClonedRepo, DOUP_BRANCH_PREFIX, Git, GitCli, Grit, UpdateBranch};
use doup_plugin_host::{
  EcosystemPlugin, PlatformPlugin, ecosystem_plugin::Dependency, platform_plugin::PullRequestContent,
};
use std::{collections::HashMap, num::NonZero};
use tempfile::TempDir;

/// Runs the doup update process
#[derive(Args)]
pub struct UpdateArgs {
  /// Does not make any changes or create any PRs, only reports what actions would be taken.
  #[arg(long)]
  dry_run: bool,
}

pub async fn update(args: UpdateArgs) -> Result<()> {
  // Load the bot config. This controls settings that affect the
  // overall operation of doup - eg: which git backend to use.
  let config = DoupBotConfigFile::load(None, None)?.merged;
  let git = match config.git_backend {
    GitBackendKind::GitCli => Git::new(GitCli),
    GitBackendKind::Grit => Git::new(Grit),
  };

  // Loop through every configured repo that we are to update
  let mut failed = 0;
  for (index, repo) in config.repos.iter().enumerate() {
    // Clone the repo into a new temp dir (auto deleted on drop)
    let dir = TempDir::new()?;
    let destination = dir.path();
    let mut options = CloneOptions::new(&repo.origin, destination).with_depth(NonZero::new(1).unwrap());

    if let Some(auth) = &repo.auth {
      options = options.with_auth(match auth {
        DoupBotRepoAuth::Https { user, pass } => Auth::Https { user: user.clone(), pass: pass.clone() },
        DoupBotRepoAuth::Ssh { key, .. } => match key {
          Some(key) => Auth::Ssh { key: key.clone() },
          None => bail!("ssh key not loaded"),
        },
      });
    }

    eprint!("Cloning {} into {}... ", repo.origin, destination.display());
    let clone_result = git.clone_repo(&options).await;
    if let Err(err) = clone_result {
      eprintln!("Failed to clone {}: {err:#}", repo.origin);
      failed += 1;
      continue;
    }
    eprintln!("DONE");
    let repo = clone_result.unwrap();

    // Load the repo's config
    let env_prefix = DoupRepoConfigFile::repo_env_prefix(index);
    let repo_config = DoupRepoConfigFile::load(Some(destination), Some(&env_prefix))?.merged;
    //eprintln!("Loaded repo config {:#?}", repo_config);

    // Load all the repo's plugins before we start work
    // A bad config or bad WASM module should be caught here, not half way through updating the repo.
    let module_search_dirs = vec![destination.to_path_buf()];

    eprint!("Loading {}... ", repo_config.platform.plugin);
    let platform = PlatformPlugin::load(&repo_config.platform, module_search_dirs.clone()).await?;
    eprintln!("DONE");

    let mut ecosystems = HashMap::new();
    for ecosystem in &repo_config.ecosystems {
      eprint!("Loading {}... ", ecosystem.plugin);
      ecosystems.insert(ecosystem, EcosystemPlugin::load(ecosystem, module_search_dirs.clone()).await?);
      eprintln!("DONE");
    }

    // Now discover all the dependencies that may need updating
    let mut dependencies_by_ecosystem = HashMap::new();
    for (&ecosystem_ref, ecosystem_plugin) in &ecosystems {
      eprint!("Discovering dependencies for {}... ", ecosystem_ref.plugin);
      dependencies_by_ecosystem.insert(ecosystem_ref, ecosystem_plugin.discover_deps(destination).await?);
      eprintln!("DONE");
    }

    // For each dependency, lookup the next version
    let mut next_versions = Vec::new();
    for (&ecosystem_ref, dependencies) in &dependencies_by_ecosystem {
      let ecosystem_plugin = &ecosystems[ecosystem_ref];
      for dependency in dependencies {
        eprint!("Looking up next version using {} for {}... ", ecosystem_ref.plugin, dependency.name);
        if let Some(next_version) = ecosystem_plugin.lookup_versions(dependency).await? {
          next_versions.push((ecosystem_ref, dependency, next_version.clone()));
          eprintln!("{}", next_version.next);
        } else {
          eprintln!("no update needed");
        }
      }
    }

    // Fetch release notes for all versions between current and next
    let mut candidates = Vec::new();
    for &(ecosystem_ref, dependency, ref versions) in &next_versions {
      let ecosystem_plugin = &ecosystems[ecosystem_ref];
      let to = if let Some(ref latest) = versions.latest { latest } else { &versions.next };
      eprint!("Downloading release notes using {} for {}... ", ecosystem_ref.plugin, dependency.name);
      let notes = ecosystem_plugin.fetch_release_notes(dependency, to).await?;
      eprintln!("found {} versions", notes.len());
      candidates.push((ecosystem_ref, dependency, versions, notes));
    }

    // In order for our pure reconciler to make decisions about what actions to
    // take we need a list of all existing doup branches and their current state.
    eprint!("Listing existing branches... ");
    let mut existing_branches: Vec<String> = Vec::new();
    let mut head_statuses: HashMap<String, HeadStatus> = HashMap::new();
    for branch in repo.list_remote_branches().await? {
      // Ignore any branch that doup does not manage
      if !branch.starts_with(DOUP_BRANCH_PREFIX) {
        continue;
      }

      // A branch with no authors has no commits, so it is safe to delete.
      // Otherwise, any author other than doup means the branch must be left
      // alone so it is never rebuilt nor scheduled for deletion.
      let authors = repo.branch_authors(&branch).await?;
      let only_doup = authors.iter().all(|a| a == repo.identity().email());

      // A doup only branch behind the default branch has its PR rebuilt on top of it.
      let head_status = if !only_doup {
        HeadStatus::ForeignCommits
      } else if repo.branch_is_behind(&branch).await? {
        HeadStatus::Behind
      } else {
        HeadStatus::UpToDate
      };
      head_statuses.insert(branch.clone(), head_status);

      if only_doup {
        existing_branches.push(branch);
      }
    }
    eprintln!("found {}", existing_branches.len());

    // Similarly we also need a list of existing PRs.
    // We need to fetch all open PRs with a head branch starting with `doup/`.
    let mut existing_prs: Vec<PullRequest> = Vec::new();
    eprint!("Listing existing open prs... ");
    let open_prs = platform.list_open_prs(DOUP_BRANCH_PREFIX).await?;
    eprintln!("found {}", open_prs.len());
    existing_prs.extend(open_prs.into_iter().map(|pr| PullRequest {
      number: pr.number,
      version: pr.version,
      // A branch we know nothing about is assumed up to date, so it is never needlessly rebuilt.
      head_status: head_statuses.get(&pr.head_branch).copied().unwrap_or(HeadStatus::UpToDate),
      head_branch: pr.head_branch,
      state: PullRequestState::Open,
    }));

    // And only closed PRs whose `head_branch` matches one of our `candidates`.
    let mut candidate_branches = Vec::new();
    for &(ecosystem_ref, dependency, versions, ref _notes) in &candidates {
      candidate_branches.push(UpdateBranch::new(ecosystem_ref, dependency, false).to_string());
      if versions.latest.is_some() {
        candidate_branches.push(UpdateBranch::new(ecosystem_ref, dependency, true).to_string());
      }
    }
    eprint!("Listing existing closed prs... ");
    let closed_prs = platform.list_closed_prs(&candidate_branches).await?;
    eprintln!("found {}", closed_prs.len());
    existing_prs.extend(closed_prs.into_iter().map(|pr| PullRequest {
      number: pr.number,
      version: pr.version,
      head_branch: pr.head_branch,
      state: PullRequestState::Closed,
      head_status: HeadStatus::UpToDate,
    }));

    // Now work out what actions we need to take.
    // This won't actually do anything, it just takes the data we have collected
    // thus far and returns a list of actions to take, whether or not we perform
    // the actions depends on if this is a dry run or not.
    let actions = reconcile(candidates, existing_prs, existing_branches)?;
    eprintln!("Planned actions: {}", actions.len());
    for action in &actions {
      eprintln!("  {}", action.summary());
    }
    if args.dry_run {
      return Ok(());
    }

    // Create or update all pull requests first
    let base_branch = repo.default_branch().await?;
    let mut action_failed = false;
    for action in &actions {
      let result: Result<()> = async {
        match action {
          DoupAction::CreatePR(doup_repo_plugin, dependency, to_version, _release_notes, _reason) => {
            let branch = UpdateBranch::new(doup_repo_plugin, dependency, to_version.latest).to_string();
            let title = commit_message(dependency, to_version);
            push_update_branch(&repo, &ecosystems[doup_repo_plugin], &branch, dependency, to_version, &title).await?;
            let pr = platform.create_pr(&branch, &pr_content(&base_branch, title, dependency, to_version)).await?;
            eprintln!("Created PR {}", pr.url);
          }
          DoupAction::UpdatePR(prnumber, doup_repo_plugin, dependency, to_version, _release_notes, _reason) => {
            let branch = UpdateBranch::new(doup_repo_plugin, dependency, to_version.latest).to_string();
            let title = commit_message(dependency, to_version);
            push_update_branch(&repo, &ecosystems[doup_repo_plugin], &branch, dependency, to_version, &title).await?;
            let content = pr_content(&base_branch, title, dependency, to_version);
            let pr = platform.update_pr(*prnumber.as_ref(), &content).await?;
            eprintln!("Updated PR {}", pr.url);
          }
          _ => {}
        }
        Ok(())
      }
      .await;
      if let Err(err) = result {
        eprintln!("Failed to apply action `{}`: {err:#}", action.summary());
        action_failed = true;
      }
    }

    // Then close any old prs
    for action in &actions {
      if let DoupAction::ClosePR(prnumber, _reason) = action {
        match platform.close_pr(*prnumber.as_ref(), None).await {
          Ok(pr) => eprintln!("Closed PR {}", pr.url),
          Err(err) => {
            eprintln!("Failed to apply action `{}`: {err:#}", action.summary());
            action_failed = true;
          }
        }
      }
    }
    // Finally delete any branches
    for action in &actions {
      if let DoupAction::DeleteBranch(branch_name, _reason) = action {
        match repo.delete_remote_branch(branch_name.as_ref()).await {
          Ok(()) => eprintln!("Deleted branch {branch_name}"),
          Err(err) => {
            eprintln!("Failed to apply action `{}`: {err:#}", action.summary());
            action_failed = true;
          }
        }
      }
    }
    if action_failed {
      failed += 1;
    }
  }

  if failed > 0 {
    bail!("{failed} repo(s) failed to update");
  }
  Ok(())
}

fn commit_message(dependency: &Dependency, to_version: &ToVersion) -> String {
  format!("chore(doup): update {} from {} to {}", dependency.name, dependency.current_version, to_version.version)
}

// TODO: Build a real PR body, eg: from the release notes.
fn pr_content(base_branch: &str, title: String, dependency: &Dependency, to_version: &ToVersion) -> PullRequestContent {
  PullRequestContent {
    base_branch: base_branch.to_string(),
    title,
    body: format!("Updates `{}` from `{}` to `{}`.", dependency.name, dependency.current_version, to_version.version),
    version: to_version.version.clone(),
  }
}

/// Applies the update of `dependency` to `to_version` on `branch`, starting from the default branch,
/// commits it with `message` & pushes it to `origin`.
async fn push_update_branch(
  repo: &ClonedRepo,
  ecosystem_plugin: &EcosystemPlugin,
  branch: &str,
  dependency: &Dependency,
  to_version: &ToVersion,
  message: &str,
) -> Result<()> {
  let worktree = repo.worktree_add(branch).await?;

  eprintln!("Created new worktree {} for {}, now applying changes...", worktree.path().display(), branch);
  ecosystem_plugin.apply_updates(worktree.path(), dependency, &to_version.version).await?;

  eprint!("Changed Applied, pushing... ");
  worktree.commit(message).await?;

  // The branch is always rebuilt from the default branch, so it is force pushed, even for a CreatePR.
  // When updating a PR this replaces our previous commit. When creating one, the branch may already
  // exist on `origin` (eg: the branch of a closed PR that was never deleted, or a previous run that
  // pushed but failed to open the PR), in which case a fast-forward only push would be rejected.
  // The lease (origin's tip when the worktree was added) still stops us clobbering concurrent pushes.
  worktree.push_force().await?;
  eprintln!("DONE");

  eprint!("Removing worktree {}... ", worktree.path().display());
  worktree.remove().await?;
  eprintln!("DONE");

  Ok(())
}
