use anyhow::{Result, bail};
use clap::Args;
use doup_config::{
  bot::{DoupBotConfigFile, DoupBotRepoAuth, GitBackendKind},
  file::ConfigFile,
  repo::DoupRepoConfigFile,
};
use doup_git::{Auth, CloneOptions, Git, GitBackend, GitCli, Grit};
use doup_plugin_host::{EcosystemPlugin, PlatformPlugin};
use std::{collections::HashMap, num::NonZero};
use tempfile::TempDir;

/// Runs the doup update process
#[derive(Args)]
pub struct UpdateArgs {
  /// Does not make any changes or create any PRs, only reports what actions would be taken.
  #[arg(long)]
  dry_run: bool,
}

pub async fn update(_args: UpdateArgs) -> Result<()> {
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
    if let Err(err) = git.clone_repo(&options).await {
      eprintln!("Failed to clone {}: {err:#}", repo.origin);
      failed += 1;
      continue;
    }
    eprintln!("DONE");

    // Load the repo's config
    let env_prefix = DoupRepoConfigFile::repo_env_prefix(index);
    let repo_config = DoupRepoConfigFile::load(Some(destination), Some(&env_prefix))?.merged;
    //eprintln!("Loaded repo config {:#?}", repo_config);

    // Load all the repo's plugins before we start work
    // A bad config or bad WASM module should be caught here, not half way through updating the repo.
    let module_search_dirs = vec![destination.to_path_buf()];

    eprint!("Loading {}... ", repo_config.platform.plugin);
    let _platform = PlatformPlugin::load(&repo_config.platform, module_search_dirs.clone()).await?;
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
    let mut release_notes = Vec::new();
    for &(ecosystem_ref, dependency, ref versions) in &next_versions {
      let ecosystem_plugin = &ecosystems[ecosystem_ref];
      let to = if let Some(ref latest) = versions.latest { latest } else { &versions.next };
      eprint!("Downloading release notes using {} for {}... ", ecosystem_ref.plugin, dependency.name);
      let notes = ecosystem_plugin.fetch_release_notes(dependency, to).await?;
      eprintln!("found {} versions", notes.len());
      release_notes.push((ecosystem_ref, dependency, versions, notes));
    }

    /*
      TODO:

      - Collect all remaining information, such as remote branches, existing PRs, etc.
        To be able to create a list of actions to perform:

        - Upsert (that is create or replace) a PR because we have detected a
          difference between the current version and the next version.
          - There may in fact be 2 PRs, one for the `next` version
            and one for the `latest` version.

          - The ecosystem world gets a new method, which would actually make the changes inside a git worktree.
            But the host still does all the actual git operations, the guest is only writing to files inside the provided worktree @ /workspace.
            Then once done the host pushes the changes to the PR.
            eg: export async apply-updates(dependency: dependency, to: string) -> result<(), string>

        - Skip the update because it was excluded by a set of blacklist rules.
          NB: These rules are yet to be added into `DoupRepoConfigFile`.
          But the plan is to allow the user to configure doup to ignore certain updates.
          The exact shape & form these rules will take are to be determined.
          We should look at what RenovateBot does here for inspiration.

        - Close existing PRs (& delete their branch) because we detected that
          the default branch is already up to date. ie: A user manually updated
          outside of doup.

        - Delete any remaining orphaned branches. (That is branches without PRs)

        - IMPORTANT: Only ever delete branches that are the sole author of the
          doup bot, any commits from any other authors means the branch stays.

      - A dry run prints a summary of all the info we have up until this point and then dies.

      - Otherwise perform each action

      Reconciliation DESIGN:

      ```
      // NB: Whether or not we actually bother with these newtypes I'm not sure
      // but just to communicate intent I think this is helpful.
      struct ToVersion(String);
      struct PRNumber(i64);
      struct BranchName(String);

      enum DoupAction {
        /// Use ecosystems[DoupRepoPlugin].apply_updates(Dependency, ToVersion) inside a new git worktree.
        /// Maybe we also give the ecosystem plugin an opportunity to participate in the PR title & body building?
        /// There is the commit message too.
        /// The problem we have at the moment is that the host does not have a reliable way to define the "current" version.
        /// At the moment the "current" version is buried in the occurrences.
        /// We will want to use phrases like: "updating {{name}} from {{current}} to {{next}}"
        /// let body = pr_body_template(Vec<ReleaseNote>);
        /// Then platform.create_pull_request(title, body)
        UpsertPR(DoupRepoPlugin, Dependency, ToVersion, Vec<ReleaseNote>),

        /// Here we are saying we have on purpose decided to not update this dependency to this version.
        /// This would ultimately result in a log message, nothing more.
        /// Again we kind of need to know the "CurrentVersion"?
        /// This action is probably out of scope for now as it depends on defining the exclusion rules.
        SkipUpdate(Dependency, ToVersion),

        /// In order to know what PRs to close we need to know what PRs already exist.
        ClosePR(PRNumber),

        /// Likewise, in order to know what branches to delete we need to know what branches already exist.
        DeleteBranch(BranchName),
      }
      ```

      - reconcile(release_notes, existing_prs, existing_branches) -> Result<Vec<Action>>
    */
  }

  if failed > 0 {
    bail!("{failed} repo(s) failed to update");
  }
  Ok(())
}
