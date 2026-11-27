//! Loads and runs doup's WASM plugins using [wasmtime].
//!
//! doup is extended through WebAssembly components rather than native code. Each plugin is a
//! component that targets one of the worlds defined in the WIT package (`projects/plugins/wit`),
//! and this crate is the host side of that contract: it finds the component, compiles it,
//! instantiates it in a sandbox and calls its exports.
//!
//! # Plugin kinds
//!
//! - [`EcosystemPlugin`]: teaches doup about a package ecosystem.
//! - [`PlatformPlugin`]: teaches doup about a hosting platform.
//!
//! Both are cheap handles created from a [`doup_config::repo::DoupRepoPlugin`] (the plugin
//! reference and its config from the repo's doup config) plus a list of directories used to
//! resolve relative plugin paths. [`EcosystemPlugin::load`] and [`PlatformPlugin::load`] also
//! warm the plugin up front so that load, instantiation and configuration failures surface early.
//!
//! # How it works
//!
//! Every plugin world shares the same lifecycle, which is written once and made generic over the world:
//!
//! 1. The component is located (an absolute path, or a relative one searched for in the current
//!    directory and then the supplied search dirs) and compiled once into a pre-instantiated form
//!    that is reused for every later session.
//!
//! 2. A session, which is a wasmtime `Store` plus the world's generated bindings, is created from
//!    it. The plugin's `configure` export is always called with the plugin's config (serialised
//!    as JSON) before the session is used, and a plugin that rejects its config fails the session.
//!
//! 3. Calls run against a session. Sessions that are idle are pooled and reused, and a new one is
//!    created whenever none are free. A session that traps is dropped rather than reused.
//!
//! The wasmtime [`Engine`](wasmtime::Engine) and the [`Linker`](wasmtime::component::Linker) for
//! each world are created lazily and shared process wide, as both are expensive to build.
//!
//! # Sandboxing
//!
//! Guests only get the capabilities the host explicitly grants:
//!
//! - **Filesystem**: nothing by default. Calls that need files preopen just the directory they
//!   need, such as a read-only `/workspace` when discovering dependencies.
//!
//! - **HTTP**: outbound `wasi:http` requests are denied unless a session is created with
//!   `HttpPolicy::Allowed`. Pooled sessions never have network access.
//!
//! - **Containers**: the `run` function of `doup:plugin/host-api` lets a guest run a short lived container
//!   of any image, with its `/workspace` & a scratch `/container-init` dir mounted in. This lets a
//!   plugin lean on an ecosystem's own tooling (eg: `cargo update`) rather than re-implementing it.
//!   Runs are denied unless a session is created with `ContainerPolicy::Allowed`, currently only
//!   `apply-updates` is. Containers are run with Docker (or anything compatible), configured by the
//!   usual `DOCKER_HOST` etc. environment variables.
//!
//! - **Host API**: the `doup:plugin/host-api` imports, which are provided to every plugin.
//!
//! Every capability beyond WASI is described by the `SessionPolicy` a session is created with.
//!
//! Stdout and stderr are inherited so plugin output is visible to the user.
//!
//! # Remote plugins
//!
//! References in the `wkg` form (`namespace:package@version`) are recognised but not supported
//! yet. Plugins must currently be local component files.
//!
//! # Adding a world
//!
//! Implement the `world::PluginWorld` trait for a marker type (see the existing ecosystem and
//! platform plugins), then wrap a `session::PluginSessionPool` of it in a public handle type.
//!
//! [wasmtime]: https://docs.wasmtime.dev

use anyhow::Result;
use wasmtime_wasi::WasiCtxBuilder;

mod host;
mod session;
mod wkg;
mod world;

pub mod ecosystem_plugin;
pub use ecosystem_plugin::EcosystemPlugin;

pub mod platform_plugin;
pub use platform_plugin::PlatformPlugin;

/// Just a simple wrapper to make it easy to put a WasiCtxBuilder closure into a variable.
///
/// Before:
///
/// ```ignore
/// let mut session = self
///   .session_pool
///   .create_session(SessionPolicy::default(), |b| Ok(b.preopened_dir(workspace_dir, "/workspace", FsPerms::ReadOnly)?))
///   .await?;
/// ```
///
/// After:
///
/// ```ignore
/// let permissions = wasi(|b| Ok(b.preopened_dir(workspace_dir, "/workspace", FsPerms::ReadOnly)?));
/// let mut session = self.session_pool.create_session(SessionPolicy::default(), permissions).await?;
/// ```
///
/// It's purely a stylistic thing,
fn wasi(
  b: impl FnOnce(&mut WasiCtxBuilder) -> Result<&mut WasiCtxBuilder>,
) -> impl FnOnce(&mut WasiCtxBuilder) -> Result<&mut WasiCtxBuilder> {
  b
}
