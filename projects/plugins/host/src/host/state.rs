use super::container_runner::{self as runner, ContainerPolicy};
use super::http::{HttpHooks, HttpPolicy};
use super::policy::SessionPolicy;
use wasmtime::Result;
use wasmtime::component::{Accessor, HasSelf, ResourceTable, bindgen};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpCtxView, WasiHttpView};

// Generated bindings for the `host_api` & `types` interfaces imported by plugins.
bindgen!({ path: "../wit", world: "shared" });
pub use doup::plugin::{host_api, types};

/// Per-instance state stored in a Wasmtime `Store`.
///
/// Holds the WASI context and resource table that plugins need at runtime,
/// and implements the host-provided `host_api` imports.
pub struct HostState {
  wasi: WasiCtx,
  http: WasiHttpCtx,
  http_hooks: HttpHooks,
  container_policy: ContainerPolicy,
  table: ResourceTable,

  /// What the session's [`SessionPolicy`] grants, held back until [`HostState::grant`] is called.
  pending: Option<Granted>,
}

/// The parts of a [`SessionPolicy`] that [`HostState::grant`] installs.
struct Granted {
  wasi: WasiCtx,
  http: HttpPolicy,
  container: ContainerPolicy,
}

impl HostState {
  /// Creates a new host state that can do no I/O at all, not even write to stdout or stderr, until
  /// [`HostState::grant`] is called. This is what lets a guest's `configure` export run with nothing
  /// but its own memory, whatever the policy otherwise allows.
  ///
  /// `policy` decides which capabilities beyond WASI the guest is granted. Its `wasi` closures
  /// receive the [`WasiCtxBuilder`], in the order they were added, so callers can customise the
  /// WASI context (e.g. inherit stdio, preopen directories, set env vars) before it is built.
  ///
  /// # Errors
  ///
  /// Returns an error if a closure fails, e.g. a preopened directory cannot be opened.
  pub fn new(policy: SessionPolicy) -> Result<Self> {
    let mut builder = WasiCtxBuilder::new();
    for configure in policy.wasi {
      configure(&mut builder)?;
    }
    Ok(Self {
      wasi: WasiCtxBuilder::new().build(),
      http: WasiHttpCtx::new(),
      http_hooks: HttpHooks { http_policy: HttpPolicy::Denied },
      container_policy: ContainerPolicy::Denied,
      table: ResourceTable::new(),
      // Built now, rather than in `grant`, so a bad closure (eg: a missing preopen dir) fails session creation early.
      pending: Some(Granted { wasi: builder.build(), http: policy.http, container: policy.container }),
    })
  }

  /// Installs what the session's [`SessionPolicy`] grants, replacing the I/O free state from [`HostState::new`].
  /// This only swaps the host side state, the guest's memory & instance are left untouched. Does nothing if
  /// already granted.
  pub fn grant(&mut self) {
    if let Some(granted) = self.pending.take() {
      self.wasi = granted.wasi;
      self.http_hooks.http_policy = granted.http;
      self.container_policy = granted.container;
    }
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

/// Implements the `host_api` functions that need access to the store.
impl<T> host_api::HostWithStore<T> for HasSelf<HostState> {
  /// Runs a container, as permitted by the session's `ContainerPolicy`.
  async fn run(
    accessor: &Accessor<T, Self>,
    request: host_api::RunRequest,
  ) -> std::result::Result<host_api::RunOutput, host_api::RunError> {
    // The store can't be held across an await, so take what's needed from it up front.
    let policy = accessor.with(|mut access| access.get().container_policy.clone());
    Ok(runner::run(&policy, request).await?)
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::sync::{Arc, Mutex};

  #[test]
  fn nothing_is_granted_until_asked_for() {
    let policy = SessionPolicy::new().http(HttpPolicy::Allowed);
    let mut state = HostState::new(policy).unwrap();
    assert_eq!(state.http_hooks.http_policy, HttpPolicy::Denied);

    state.grant();
    assert_eq!(state.http_hooks.http_policy, HttpPolicy::Allowed);
  }

  #[test]
  fn wasi_configurers_are_additive_and_run_in_order() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let (first, second) = (calls.clone(), calls.clone());
    let policy = SessionPolicy::new()
      .wasi(move |b| {
        first.lock().unwrap().push(1);
        Ok(b)
      })
      .wasi(move |b| {
        second.lock().unwrap().push(2);
        Ok(b)
      });

    HostState::new(policy).unwrap();
    assert_eq!(*calls.lock().unwrap(), vec![1, 2]);
  }

  #[test]
  fn failing_wasi_configurer_stops_session_creation() {
    let ran_after = Arc::new(Mutex::new(false));
    let flag = ran_after.clone();
    let policy = SessionPolicy::new().wasi(|_| Err(wasmtime::format_err!("boom"))).wasi(move |b| {
      *flag.lock().unwrap() = true;
      Ok(b)
    });

    let Err(err) = HostState::new(policy) else { panic!("expected the failing configurer to error") };
    assert_eq!(err.to_string(), "boom");
    assert!(!*ran_after.lock().unwrap(), "configurers after a failure must not run");
  }
}
