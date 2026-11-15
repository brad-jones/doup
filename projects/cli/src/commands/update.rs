use anyhow::{Result, bail};
use clap::Args;
use doup_config::{
  bot::{DoupBotConfigFile, DoupBotRepoAuth, GitBackendKind},
  file::ConfigFile,
  repo::DoupRepoConfigFile,
};
use doup_git::{Auth, CloneOptions, Git, GitBackend, GitCli, Grit};
use doup_plugin_host::{load_ecosystem_plugin, load_platform_plugin, resolve_plugin_path};
use tempfile::TempDir;

/// Runs the doup update process
#[derive(Args)]
pub struct UpdateArgs {
  /// Does not make any changes or create any PRs, only reports what needs to be updated
  #[arg(long)]
  dry_run: bool,
}

pub async fn update(_args: UpdateArgs) -> Result<()> {
  let config = DoupBotConfigFile::load(None, None)?.merged;

  let git = match config.git_backend {
    GitBackendKind::GitCli => Git::new(GitCli),
    GitBackendKind::Grit => Git::new(Grit),
  };

  let mut failed = 0;

  for (index, repo) in config.repos.iter().enumerate() {
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

    let env_prefix = DoupRepoConfigFile::repo_env_prefix(index);
    let repo_config = DoupRepoConfigFile::load(Some(destination), Some(&env_prefix))?.merged;
    println!("Loaded repo config {:#?}", repo_config);

    let platform_path = resolve_plugin_path(&repo_config.platform.plugin, Some(destination))?;
    let mut platform = load_platform_plugin(&platform_path).await?;
    println!("Platform says: {}", platform.hello_world().await?);

    for ecosystem in &repo_config.ecosystems {
      let ecosystem_path = resolve_plugin_path(&ecosystem.plugin, Some(destination))?;
      let mut ecosystem = load_ecosystem_plugin(&ecosystem_path).await?;
      println!("Ecosystem says: {}", ecosystem.hello_world().await?);
    }
  }

  if failed > 0 {
    bail!("{failed} repo(s) failed to update");
  }
  Ok(())
}
