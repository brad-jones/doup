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

fn main() {
  if let Err(err) = match Cli::parse().command {
    Commands::Init(args) => commands::init(args),
    Commands::Update(args) => commands::update(args),
    Commands::SelfUpdate(args) => commands::self_update(args),
  } {
    eprintln!("Error: {err:#}");
    std::process::exit(1);
  }
}
