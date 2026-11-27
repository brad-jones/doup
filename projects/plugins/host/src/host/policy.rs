use super::container_runner::ContainerPolicy;
use super::http::HttpPolicy;
use anyhow::Result;
use std::fmt;
use wasmtime_wasi::WasiCtxBuilder;

/// Customises the [`WasiCtxBuilder`] of a session, eg: to preopen directories.
pub type WasiConfigurer = Box<dyn FnOnce(&mut WasiCtxBuilder) -> Result<&mut WasiCtxBuilder> + Send>;

/// What a session's guest is granted: its WASI context plus the capabilities beyond WASI.
/// Everything is denied by default, use the builder methods to grant more.
///
/// ```ignore
/// let policy = SessionPolicy::new()
///   .http(HttpPolicy::Allowed)
///   .wasi(move |b| Ok(b.preopened_dir(&dir, "/workspace", FsPerms::ReadOnly)?));
/// let mut session = self.session_pool.create_session(policy).await?;
/// ```
#[derive(Default)]
pub struct SessionPolicy {
  pub(crate) http: HttpPolicy,
  pub(crate) container: ContainerPolicy,
  pub(crate) wasi: Option<WasiConfigurer>,
}

impl SessionPolicy {
  /// Creates a policy that denies everything and leaves the WASI context bare.
  pub fn new() -> Self {
    Self::default()
  }

  /// Sets whether outbound `wasi:http` requests are sent.
  pub fn http(mut self, http: HttpPolicy) -> Self {
    self.http = http;
    self
  }

  /// Sets whether containers may be run via `doup:plugin/host-api`.
  pub fn containers(mut self, container: ContainerPolicy) -> Self {
    self.container = container;
    self
  }

  /// Sets the closure that customises the [`WasiCtxBuilder`] (e.g. preopen directories, set env vars).
  pub fn wasi(
    mut self,
    configure: impl FnOnce(&mut WasiCtxBuilder) -> Result<&mut WasiCtxBuilder> + Send + 'static,
  ) -> Self {
    self.wasi = Some(Box::new(configure));
    self
  }
}

impl fmt::Debug for SessionPolicy {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("SessionPolicy")
      .field("http", &self.http)
      .field("container", &self.container)
      .field("wasi", &self.wasi.as_ref().map(|_| "<closure>"))
      .finish()
  }
}
