use anyhow::{Context, Result, anyhow, bail};
use clap::Args;
use git::{Auth, CloneOptions, Git, GitBackend, GitCli, Grit};
use tempfile::TempDir;

use crate::config::{
  bot::{DoupBotConfigFile, DoupBotRepo, DoupBotRepoAuth, GitBackendKind},
  file::ConfigFile,
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
    println!("Cloning: {}", repo.origin);

    if let Err(err) = update_repo(&git, repo).await {
      eprintln!("Failed to update {}: {err:#}", repo.origin);
      failed += 1;
    }
  }

  if failed > 0 {
    bail!("{failed} repo(s) failed to update");
  }
  Ok(())
}

async fn update_repo(git: &Git, repo: &DoupBotRepo) -> Result<()> {
  // The temp dir is deleted when `dir` is dropped at the end of this function.
  let dir = TempDir::new().context("failed to create a temp dir")?;
  let destination = dir.path().join("repo");

  let mut options = CloneOptions::new(&repo.origin, &destination);
  if let Some(auth) = &repo.auth {
    options = options.with_auth(to_git_auth(auth)?);
  }
  git.clone_repo(&options).await.context("clone failed")?;
  println!("Cloned to {}", destination.display());

  Ok(())
}

fn to_git_auth(auth: &DoupBotRepoAuth) -> Result<Auth> {
  Ok(match auth {
    DoupBotRepoAuth::Https { user, pass } => Auth::Https {
      user: user.clone(),
      pass: pass.clone(),
    },
    DoupBotRepoAuth::Ssh { key, .. } => Auth::Ssh {
      key: key
        .clone()
        .ok_or_else(|| anyhow!("ssh key was not loaded"))?,
    },
  })
}
