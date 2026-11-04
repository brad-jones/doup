mod commands;
mod schema;

use clap::Parser;
use commands::Commands;

/// A dependency updater bot
#[derive(Parser)]
#[command(name = "doup", version)]
struct Cli {
  #[command(subcommand)]
  command: Commands,
}

#[tokio::main]
async fn main() {
  if let Err(err) = match Cli::parse().command {
    Commands::Init(args) => commands::init(args).await,
    Commands::Update(args) => commands::update(args).await,
    Commands::SelfUpdate(args) => commands::self_update(args).await,
  } {
    eprintln!("Error: {err:#}");
    std::process::exit(1);
  }
}
