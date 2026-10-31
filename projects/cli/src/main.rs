use clap::{Parser, Subcommand};
use wasmtime::component::{Component, HasSelf, Linker, ResourceTable};
use wasmtime::{Config, Engine, Result, Store};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

wasmtime::component::bindgen!({
  path: "../wit",
  world: "plugin",
});

use calculator::plugin::host_api;

struct HostState {
  wasi: WasiCtx,
  table: ResourceTable,
}

impl WasiView for HostState {
  fn ctx(&mut self) -> WasiCtxView<'_> {
    WasiCtxView {
      ctx: &mut self.wasi,
      table: &mut self.table,
    }
  }
}

impl host_api::Host for HostState {
  fn host_version(&mut self) -> String {
    format!("host-{}", env!("CARGO_PKG_VERSION"))
  }
}

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

fn main() -> Result<()> {
  let cli = Cli::parse();

  match cli.command {
    Commands::Operation { op, a, b } => {
      let mut config = Config::new();
      config.wasm_component_model(true);
      let engine = Engine::new(&config)?;
      let component = Component::from_file(&engine, op)?;
      let mut wasi = WasiCtxBuilder::new();
      wasi.inherit_stdout().inherit_stderr();
      let state = HostState {
        wasi: wasi.build(),
        table: ResourceTable::new(),
      };
      let mut store = Store::new(&engine, state);
      let mut linker: Linker<HostState> = Linker::new(&engine);
      wasmtime_wasi::p2::add_to_linker_sync(&mut linker)?;
      host_api::add_to_linker::<HostState, HasSelf<HostState>>(&mut linker, |state| state)?;
      let plugin = Plugin::instantiate(&mut store, &component, &linker)?;
      let plugin_result = plugin.call_operation(&mut store, a, b)?;
      match plugin_result {
        Ok(result) => println!("Result: {result}"),
        Err(()) => {
          eprintln!("Error: operation failed (overflow or division by zero)");
          std::process::exit(1);
        }
      }
    }
  }

  Ok(())
}
