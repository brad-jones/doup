use anyhow::Result;
use clap::Args;

use crate::config::{
  bot::{
    DoupBotConfigFile,
    DoupBotRepoAuth::{Https, Ssh},
  },
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

  for repo in config.repos {
    println!("Repo: {}", repo.origin);

    match repo.auth {
      Some(auth) => match auth {
        Https { user, pass } => println!("Https Auth: {}:{}", user, pass),
        Ssh { key, key_path: _ } => println!("Ssh Auth: {:#?}", key.unwrap()),
      },
      None => println!("No auth"),
    }
  }

  Ok(())
}
