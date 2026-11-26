use anyhow::{Result, bail};
use clap::Args;
use doup_config::{
  bot::{DoupBotConfigFile, DoupBotRepoAuth, GitBackendKind},
  file::ConfigFile,
  repo::DoupRepoConfigFile,
};
use doup_git::{Auth, CloneOptions, Git, GitCli, Grit};
use doup_plugin_host::{EcosystemPlugin, PlatformPlugin};
use std::{collections::HashMap, num::NonZero};
use tempfile::TempDir;

use crate::reconciliation::{DoupAction, PullRequest, PullRequestState, doup_branch, reconcile};

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

    eprint!("Listing existing branches... ");
    let mut existing_branches: Vec<String> = Vec::new();
    for branch in repo.list_remote_branches().await? {
      // Ignore any branch that doup does not manage
      if !branch.starts_with("doup/") {
        continue;
      }

      // A branch with no authors has no commits, so it is safe to delete.
      // Otherwise, any author other than doup means the branch must be left
      // alone so it is never scheduled for deletion.
      let authors = repo.branch_authors(&branch).await?;
      let only_doup = authors.iter().all(|a| a == repo.identity().email());
      if only_doup {
        existing_branches.push(branch);
      }
    }
    eprintln!("found {}", existing_branches.len());

    // List existing PRs.
    // We need to fetch all open PRs with a head branch starting with `doup/`.
    let mut existing_prs: Vec<PullRequest> = Vec::new();
    eprint!("Listing existing open prs... ");
    let open_prs = platform.list_open_prs("doup/").await?;
    eprintln!("found {}", open_prs.len());
    existing_prs.extend(open_prs.into_iter().map(|pr| PullRequest {
      number: pr.number,
      version: pr.version,
      head_branch: pr.head_branch,
      state: PullRequestState::Open,
    }));

    // And only closed PRs whose `head_branch` matches one of our `candidates`.
    let mut candidate_branches = Vec::new();
    for &(ecosystem_ref, dependency, versions, ref _notes) in &candidates {
      candidate_branches.push(doup_branch(ecosystem_ref, dependency, versions.latest.is_some()));
    }
    eprint!("Listing existing closed prs... ");
    let closed_prs = platform.list_closed_prs(&candidate_branches).await?;
    eprintln!("found {}", closed_prs.len());
    existing_prs.extend(closed_prs.into_iter().map(|pr| PullRequest {
      number: pr.number,
      version: pr.version,
      head_branch: pr.head_branch,
      state: PullRequestState::Closed,
    }));

    // Now work out what actions we need to take.
    // This won't actually do anything, it just takes the data we have collected
    // thus far and returns a list of actions to take, whether or not we perform
    // the actions depends on if this a dry run or not.
    let actions = reconcile(candidates, existing_prs, existing_branches)?;
    eprintln!("Planned actions: {}", actions.len());
    for action in &actions {
      eprintln!("  {}", action.summary());
    }
    if args.dry_run {
      return Ok(());
    }

    // Create and update all pull requests first
    for action in &actions {
      match action {
        DoupAction::CreatePR(doup_repo_plugin, dependency, to_version, release_notes, reason) => {
          let ecosystem_plugin = &ecosystems[doup_repo_plugin];
          let branch = doup_branch(doup_repo_plugin, dependency, to_version.latest);
          let worktree = repo.worktree_add(&branch).await?;
          ecosystem_plugin.apply_updates(worktree.path()).await?;
          worktree
            .commit(format!(
              "chore(doup): update {} from {} to {}",
              dependency.name, dependency.current_version, to_version.version
            ))
            .await?;
          worktree.push().await?;
          worktree.remove().await?;
          let pr = platform
            .upcert_pr(PrRequest {
              number: None,
              base_branch: repo.default_branch()?,
              head_branch: branch,
              title: format!("todo"),
              body: format!("todo"),
            })
            .await?;
          eprintln!("Created PR {}", pr.url);
        }
        DoupAction::UpdatePR(prnumber, doup_repo_plugin, dependency, to_version, release_notes, reason) => {
          // The update case is basically identical to create, except we force push our changes
          let ecosystem_plugin = &ecosystems[doup_repo_plugin];
          let branch = doup_branch(doup_repo_plugin, dependency, to_version.latest);
          let worktree = repo.worktree_add(&branch).await?;
          ecosystem_plugin.apply_updates(worktree.path()).await?;
          worktree
            .commit(format!(
              "chore(doup): update {} from {} to {}",
              dependency.name, dependency.current_version, to_version.version
            ))
            .await?;
          worktree.push_force().await?;
          worktree.remove().await?;
          let pr = platform
            .upcert_pr(PrRequest {
              number: prnumber.as_ref(),
              base_branch: repo.default_branch()?,
              head_branch: branch,
              title: format!("todo"),
              body: format!("todo"),
            })
            .await?;
          eprintln!("Updated PR {}", pr.url);
        }
        _ => {}
      }
    }

    // Then close any old prs
    for action in &actions {
      if let DoupAction::ClosePR(prnumber, _reason) = action {
        eprintln!("TODO: Close PR #{}", prnumber)
      }
    }

    // Finally delete any branches
    for action in &actions {
      if let DoupAction::DeleteBranch(branch_name, _reason) = action {
        eprintln!("TODO: Delete branch {}", branch_name)
      }
    }
  }

  if failed > 0 {
    bail!("{failed} repo(s) failed to update");
  }
  Ok(())
}
