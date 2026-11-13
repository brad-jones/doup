use anyhow::{Context, Result, anyhow, bail};
use clap::Args;
use git::{Auth, CloneOptions, Git, GitCli, Grit};
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
  let mut failed = 0;

  for repo in &config.repos {
    println!("Repo: {}", repo.origin);

    if let Err(err) = update_repo(config.git_backend, repo).await {
      eprintln!("Failed to update {}: {err:#}", repo.origin);
      failed += 1;
    }
  }

  if failed > 0 {
    bail!("{failed} repo(s) failed to update");
  }
  Ok(())
}

async fn update_repo(backend: GitBackendKind, repo: &DoupBotRepo) -> Result<()> {
  // The temp dir is deleted when `dir` is dropped at the end of this function.
  let dir = TempDir::new().context("failed to create a temp dir")?;
  let destination = dir.path().join("repo");

  let mut options = CloneOptions::new(&repo.origin, &destination);
  if let Some(auth) = &repo.auth {
    options = options.with_auth(to_git_auth(auth)?);
  }
  clone_repo(backend, options).await?;
  println!("Cloned to {}", destination.display());

  Ok(())
}

/// `GitBackend` returns `impl Future` so it can't be a `dyn` trait object; dispatch with a match.
async fn clone_repo(backend: GitBackendKind, options: CloneOptions) -> Result<()> {
  match backend {
    GitBackendKind::GitCli => Git::new(GitCli).clone_repo(options).await,
    GitBackendKind::Grit => Git::new(Grit).clone_repo(options).await,
  }
  .context("clone failed")
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
