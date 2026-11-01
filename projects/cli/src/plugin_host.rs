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

pub fn run_plugin(op: String, a: i64, b: i64) -> Result<i64, wasmtime::Error> {
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
  match plugin.call_operation(&mut store, a, b) {
    Ok(Ok(value)) => Ok(value),
    Ok(Err(())) => Err(wasmtime::Error::msg("plugin operation failed")),
    Err(err) => Err(err),
  }
}
