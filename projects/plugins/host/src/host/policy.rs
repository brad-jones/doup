use super::container_runner::ContainerPolicy;
use super::http::HttpPolicy;

/// The capabilities, beyond WASI, that a session's guest is granted. Everything is denied by default.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionPolicy {
  /// Whether outbound `wasi:http` requests are sent.
  pub http: HttpPolicy,

  /// Whether containers may be run via `doup:plugin/host-api`.
  pub container: ContainerPolicy,
}

impl SessionPolicy {
  /// Only outbound HTTP requests are allowed.
  pub fn http() -> Self {
    Self { http: HttpPolicy::Allowed, ..Self::default() }
  }
}
