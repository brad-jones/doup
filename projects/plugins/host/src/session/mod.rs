//! Plugin session management for the WASM plugin host.
//!
//! A wasmtime `Store` holds a guest's memory and can only be used by one
//! caller at a time, so concurrent calls into a plugin need separate instances.
//!
//! This module provides:
//!
//! - [`PluginSessionPool`]: resolves and pre-compiles the plugin component once,
//!   then hands out idle sessions via [`PluginSessionPool::checkout_session`]
//!   (creating and configuring new ones on demand) and takes them back via
//!   [`PluginSessionPool::checkin_session`].
//!
//! Sessions that trap or fail are dropped rather than returned to the pool.

mod plugin_session;
mod plugin_session_pool;
pub use plugin_session_pool::PluginSessionPool;
