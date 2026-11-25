use super::http::{HttpHooks, HttpPolicy};
use anyhow::Result;
use wasmtime::component::{ResourceTable, bindgen};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpCtxView, WasiHttpView};

// Generated bindings for the `host_api` interface imported by plugins.
bindgen!({ path: "../wit", world: "shared" });
pub use doup::plugin::host_api;

/// Per-instance state stored in a Wasmtime `Store`.
///
/// Holds the WASI context and resource table that plugins need at runtime,
/// and implements the host-provided `host_api` imports.
pub struct HostState {
  wasi: WasiCtx,
  http: WasiHttpCtx,
  http_hooks: HttpHooks,
  table: ResourceTable,
}

impl HostState {
  /// Creates a new host state.
  ///
  /// `configure` receives the [`WasiCtxBuilder`] so callers can customise the
  /// WASI context (e.g. inherit stdio, preopen directories, set env vars)
  /// before it is built. `http_policy` decides whether the guest's `wasi:http` requests are sent.
  ///
  /// # Errors
  ///
  /// Returns an error if `configure` fails, e.g. a preopened directory cannot be opened.
  pub fn new(
    http_policy: HttpPolicy,
    configure: impl FnOnce(&mut WasiCtxBuilder) -> Result<&mut WasiCtxBuilder>,
  ) -> Result<Self> {
    Ok(Self {
      wasi: configure(&mut WasiCtxBuilder::new())?.build(),
      http: WasiHttpCtx::new(),
      http_hooks: HttpHooks { http_policy },
      table: ResourceTable::new(),
    })
  }
}

/// Exposes the `wasi:http` context, resource table and [`HttpHooks`] to the Wasmtime linker.
/// The hooks carry the [`HttpPolicy`] that decides whether outgoing guest requests are sent.
impl WasiHttpView for HostState {
  fn http(&mut self) -> WasiHttpCtxView<'_> {
    WasiHttpCtxView { hooks: &mut self.http_hooks, table: &mut self.table, ctx: &mut self.http }
  }
}

/// Exposes the WASI context and resource table to the Wasmtime linker.
impl WasiView for HostState {
  fn ctx(&mut self) -> WasiCtxView<'_> {
    WasiCtxView { ctx: &mut self.wasi, table: &mut self.table }
  }
}

/// Implements the functions the host offers to plugins.
impl host_api::Host for HostState {
  /// Returns the host's version as `host-<crate version>`.
  fn host_version(&mut self) -> String {
    format!("host-{}", env!("CARGO_PKG_VERSION"))
  }
}
