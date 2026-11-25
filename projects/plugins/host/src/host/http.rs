use anyhow::Result;
use std::sync::Once;
use wasmtime_wasi_http::{Error as HttpError, RequestOptions, WasiBody, WasiHttpHooks, default_send_request};

/// Whether a session's guest may make outbound HTTP requests via `wasi:http`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum HttpPolicy {
  /// Every outbound request is rejected with `HTTP-request-denied`.
  #[default]
  Denied,

  /// Outbound `http` and `https` requests are sent.
  Allowed,
}

/// Enforces a session's [`Http`] policy on every outbound `wasi:http` request.
pub struct HttpHooks {
  pub http_policy: HttpPolicy,
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
    let http_policy = self.http_policy;
    Box::new(async move {
      if http_policy == HttpPolicy::Denied {
        return Err(HttpError::HttpRequestDenied);
      }
      install_crypto_provider();
      let (response, io) = default_send_request(request, options).await?;
      Ok((response.map(http_body_util::BodyExt::boxed_unsync), Box::new(io) as Box<_>))
    })
  }
}

/// Selects rustls' process-wide crypto provider. Both `aws-lc-rs` and `ring` are in the dependency
/// tree, so rustls can't choose one itself and would panic on the first TLS connection.
fn install_crypto_provider() {
  static INSTALL: Once = Once::new();
  // An Err only means another provider was installed first, which is just as good.
  INSTALL.call_once(|| _ = rustls::crypto::aws_lc_rs::default_provider().install_default());
}
