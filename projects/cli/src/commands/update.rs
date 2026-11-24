use anyhow::{Result, bail};
use clap::Args;
use doup_config::{
  bot::{DoupBotConfigFile, DoupBotRepoAuth, GitBackendKind},
  file::ConfigFile,
  repo::DoupRepoConfigFile,
};
use doup_git::{Auth, CloneOptions, Git, GitBackend, GitCli, Grit};
use doup_plugin_host::{EcosystemPlugin, PlatformPlugin};
use std::collections::HashMap;
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
    let mut options = CloneOptions::new(&repo.origin, destination);

    if let Some(auth) = &repo.auth {
      options = options.with_auth(match auth {
        DoupBotRepoAuth::Https { user, pass } => Auth::Https {
          user: user.clone(),
          pass: pass.clone(),
        },
        DoupBotRepoAuth::Ssh { key, .. } => match key {
          Some(key) => Auth::Ssh { key: key.clone() },
          None => bail!("ssh key not loaded"),
        },
      });
    }

    println!("Cloning {} into {}", repo.origin, destination.display());
    if let Err(err) = git.clone_repo(&options).await {
      eprintln!("Failed to clone {}: {err:#}", repo.origin);
      failed += 1;
      continue;
    }

    // Load the repo's config
    let env_prefix = DoupRepoConfigFile::repo_env_prefix(index);
    let repo_config = DoupRepoConfigFile::load(Some(destination), Some(&env_prefix))?.merged;
    println!("Loaded repo config {:#?}", repo_config);

    // Load all the repo's plugins before we start work
    // A bad config or bad WASM module should be caught here, not half way through updating the repo.
    let module_search_dirs = vec![destination.to_path_buf()];
    let _platform = PlatformPlugin::load(&repo_config.platform, module_search_dirs.clone()).await?;
    let mut ecosystems = vec![];
    for ecosystem in &repo_config.ecosystems {
      ecosystems.push((
        ecosystem,
        EcosystemPlugin::load(ecosystem, module_search_dirs.clone()).await?,
      ));
    }

    // Now discover all the dependencies that may need updating
    let mut dependencies_by_ecosystem = HashMap::new();
    for (ecosystem_ref, ecosystem_plugin) in ecosystems {
      dependencies_by_ecosystem.insert(ecosystem_ref, ecosystem_plugin.discover_deps().await?);
    }

    for (ecosystem_ref, dependencies) in dependencies_by_ecosystem {
      println!("{:#?}: {:#?}", ecosystem_ref, dependencies)
    }
  }

  if failed > 0 {
    bail!("{failed} repo(s) failed to update");
  }
  Ok(())
}
