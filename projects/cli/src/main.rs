use clap::{Parser, Subcommand};

mod plugin_host;
use plugin_host::run_plugin;

/// A toy WASM host
#[derive(Parser)]
#[command(version)]
struct Cli {
  #[command(subcommand)]
  command: Commands,
}

#[derive(Subcommand)]
enum Commands {
  /// Perform an operation on two numbers
  Operation {
    /// The operation to perform
    /// This refers to a path to a wasm module
    #[arg(short, long)]
    op: String,

    /// First number
    #[arg(short, long)]
    a: i64,

    /// Second number
    #[arg(short, long)]
    b: i64,
  },
}

fn main() {
  match Cli::parse().command {
    Commands::Operation { op, a, b } => match run_plugin(op, a, b) {
      Ok(value) => println!("Result: {value}"),
      Err(err) => {
        eprintln!("Error: operation failed ({err})");
        std::process::exit(1);
      }
    },
  }
}
