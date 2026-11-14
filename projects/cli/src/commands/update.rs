use anyhow::{Context, Result, anyhow, bail};
use clap::Args;
use git::{Auth, CloneOptions, Git, GitBackend, GitCli, Grit};
use tempfile::TempDir;

use crate::config::{
  bot::{DoupBotConfigFile, DoupBotRepo, DoupBotRepoAuth, GitBackendKind},
  file::ConfigFile,
  repo::DoupRepoConfigFile,
};

/// Runs the doup update process
#[derive(Args)]
pub struct UpdateArgs {
  /// Does not make any changes or create any PRs, only reports what needs to be updated
  #[arg(long)]
  dry_run: bool,
}

pub async fn update(_args: UpdateArgs) -> Result<()> {
  let config = DoupBotConfigFile::load(None)?.merged;

  let git = match config.git_backend {
    GitBackendKind::GitCli => Git::new(GitCli),
    GitBackendKind::Grit => Git::new(Grit),
  };

  let mut failed = 0;

  for repo in &config.repos {
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

    let repo_config = DoupRepoConfigFile::load(Some(destination))?.merged;
    println!("Loaded repo config {:#?}", repo_config);
  }

  if failed > 0 {
    bail!("{failed} repo(s) failed to update");
  }
  Ok(())
}
