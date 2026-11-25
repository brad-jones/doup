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
  /// Does not make any changes or create any PRs, only reports what needs to be updated
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
  }

  if failed > 0 {
    bail!("{failed} repo(s) failed to update");
  }
  Ok(())
}
