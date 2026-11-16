use wasmtime::component::{ResourceTable, bindgen};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

bindgen!({
  path: "../wit",
  world: "shared",
});

pub use doup::plugin::host_api;

pub struct HostState {
  wasi: WasiCtx,
  table: ResourceTable,
}

impl HostState {
  pub fn new(configure: impl FnOnce(&mut WasiCtxBuilder)) -> Self {
    let mut builder = WasiCtxBuilder::new();
    configure(&mut builder);
    Self {
      wasi: builder.build(),
      table: ResourceTable::new(),
    }
  }
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
