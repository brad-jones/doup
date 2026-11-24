//! Host-side state and bindings shared with WASM plugins.
//!
//! Generates the Wasmtime bindings for the `shared` WIT world and provides
//! [`HostState`], the store data that backs WASI and the host API imports.

use wasmtime::component::{ResourceTable, bindgen};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

// Generated bindings for the `host_api` interface imported by plugins.
bindgen!({ path: "../wit", world: "shared" });
pub use doup::plugin::host_api;

/// Per-instance state stored in a Wasmtime `Store`.
///
/// Holds the WASI context and resource table that plugins need at runtime,
/// and implements the host-provided `host_api` imports.
pub struct HostState {
  wasi: WasiCtx,
  table: ResourceTable,
}

impl HostState {
  /// Creates a new host state.
  ///
  /// `configure` receives the [`WasiCtxBuilder`] so callers can customise the
  /// WASI context (e.g. inherit stdio, preopen directories, set env vars)
  /// before it is built.
  pub fn new(configure: impl FnOnce(&mut WasiCtxBuilder)) -> Self {
    let mut builder = WasiCtxBuilder::new();
    configure(&mut builder);
    Self {
      wasi: builder.build(),
      table: ResourceTable::new(),
    }
  }
}

/// Exposes the WASI context and resource table to the Wasmtime linker.
impl WasiView for HostState {
  fn ctx(&mut self) -> WasiCtxView<'_> {
    WasiCtxView {
      ctx: &mut self.wasi,
      table: &mut self.table,
    }
  }
}

/// Implements the functions the host offers to plugins.
impl host_api::Host for HostState {
  /// Returns the host's version as `host-<crate version>`.
  fn host_version(&mut self) -> String {
    format!("host-{}", env!("CARGO_PKG_VERSION"))
  }
}
