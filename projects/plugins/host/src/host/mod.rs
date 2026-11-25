//! Host-side runtime support for WASM plugins.
//!
//! This module provides what a plugin instance needs from the host:
//!
//! - [`HostState`]: the per-instance state held in a wasmtime `Store`. It carries the WASI
//!   context and resource table, and implements the host-provided [`host_api`] imports.
//!
//! - [`HttpPolicy`]: controls whether a guest's outbound `wasi:http` requests are sent or
//!   rejected. Requests are denied by default.

mod http;
mod state;

pub use http::HttpPolicy;
pub use state::HostState;
pub use state::host_api;
