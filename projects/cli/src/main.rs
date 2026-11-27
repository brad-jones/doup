mod commands;
mod logging;

use clap::Parser;
use commands::Commands;

/// A dependency updater bot
#[derive(Parser)]
#[command(name = "doup", version)]
struct Cli {
  #[command(subcommand)]
  command: Commands,

  #[command(flatten)]
  log: logging::LogArgs,
}

#[tokio::main]
async fn main() {
  let cli = Cli::parse();
  logging::init(&cli.log);
  if let Err(err) = match cli.command {
    Commands::Init(args) => commands::init(args).await,
    Commands::Add(args) => commands::add(args).await,
    Commands::Remove(args) => commands::remove(args).await,
    Commands::Update(args) => commands::update(args).await,
    Commands::SelfUpdate(args) => commands::self_update(args).await,
  } {
    eprintln!("Error: {err:#}");
    std::process::exit(1);
  }
}
