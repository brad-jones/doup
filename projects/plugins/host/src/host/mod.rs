//! Host-side runtime support for WASM plugins.
//!
//! This module provides what a plugin instance needs from the host:
//!
//! - [`HostState`]: the per-instance state held in a wasmtime `Store`. It carries the WASI
//!   context and resource table, and implements the host-provided [`host_api`] imports.
//!
//! - [`SessionPolicy`]: the capabilities granted to a session's guest, all denied by default:
//!   - [`HttpPolicy`]: whether outbound `wasi:http` requests are sent or rejected.
//!   - [`ContainerPolicy`]: whether containers may be run via [`host_api`], and
//!     which host directories are mounted into them.

mod container_runner;
mod http;
mod policy;
mod state;

pub use container_runner::{ContainerMounts, ContainerPolicy};
pub use http::HttpPolicy;
pub use policy::SessionPolicy;
pub use state::HostState;
pub use state::{host_api, types};
