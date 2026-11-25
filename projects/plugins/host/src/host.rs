//! Host-side state and bindings shared with WASM plugins.
//!
//! Generates the Wasmtime bindings for the `shared` WIT world and provides
//! [`HostState`], the store data that backs WASI and the host API imports.

use anyhow::Result;
use std::sync::Once;
use wasmtime::component::{ResourceTable, bindgen};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};
use wasmtime_wasi_http::{
  Error as HttpError, RequestOptions, WasiBody, WasiHttpCtx, WasiHttpCtxView, WasiHttpHooks, WasiHttpView,
  default_send_request,
};

// Generated bindings for the `host_api` interface imported by plugins.
bindgen!({ path: "../wit", world: "shared" });
pub use doup::plugin::host_api;

/// Whether a session's guest may make outbound HTTP requests via `wasi:http`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Network {
  /// Every outbound request is rejected with `HTTP-request-denied`.
  #[default]
  Denied,

  /// Outbound `http` and `https` requests are sent.
  Allowed,
}

/// Selects rustls' process-wide crypto provider. Both `aws-lc-rs` and `ring` are in the dependency
/// tree, so rustls can't choose one itself and would panic on the first TLS connection.
fn install_crypto_provider() {
  static INSTALL: Once = Once::new();
  // An Err only means another provider was installed first, which is just as good.
  INSTALL.call_once(|| _ = rustls::crypto::aws_lc_rs::default_provider().install_default());
}

/// Enforces a session's [`Network`] policy on every outbound `wasi:http` request.
struct HttpHooks {
  network: Network,
}

impl WasiHttpHooks for HttpHooks {
  fn send_request(
    &mut self,
    request: http::Request<WasiBody>,
    options: Option<RequestOptions>,
    _fut: Box<dyn Future<Output = Result<(), HttpError>> + Send>,
  ) -> Box<
    dyn Future<
        Output = Result<(http::Response<WasiBody>, Box<dyn Future<Output = Result<(), HttpError>> + Send>), HttpError>,
      > + Send,
  > {
    let network = self.network;
    Box::new(async move {
      if network == Network::Denied {
        return Err(HttpError::HttpRequestDenied);
      }
      install_crypto_provider();
      let (response, io) = default_send_request(request, options).await?;
      Ok((response.map(http_body_util::BodyExt::boxed_unsync), Box::new(io) as Box<_>))
    })
  }
}

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
  /// before it is built. `network` decides whether the guest's `wasi:http` requests are sent.
  ///
  /// # Errors
  ///
  /// Returns an error if `configure` fails, e.g. a preopened directory cannot be opened.
  pub fn new(
    network: Network,
    configure: impl FnOnce(&mut WasiCtxBuilder) -> Result<&mut WasiCtxBuilder>,
  ) -> Result<Self> {
    Ok(Self {
      wasi: configure(&mut WasiCtxBuilder::new())?.build(),
      http: WasiHttpCtx::new(),
      http_hooks: HttpHooks { network },
      table: ResourceTable::new(),
    })
  }
}

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
