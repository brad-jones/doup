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
//! reference and its config from the repo's doup config) plus the path of its wasm module.
//! [`EcosystemPlugin::load`] and [`PlatformPlugin::load`] find that module with a [`ModuleResolver`]
//! & also warm the plugin up front so that load, instantiation and configuration failures surface early.
//!
//! # How it works
//!
//! Every plugin world shares the same lifecycle, which is written once and made generic over the world:
//!
//! 1. The component is located by a [`ModuleResolver`] (an absolute path, a relative one searched for
//!    in the current directory and then the supplied search dirs, or a locked package downloaded from its
//!    registry) and compiled once into a pre-instantiated form that is reused for every later session.
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
//!   `HttpPolicy::Allowed`. Ecosystem plugins only get it for the calls that need it, while every
//!   platform plugin call (including pooled sessions) has it, as talking to the platform is its job.
//!
//! - **Containers**: the `run` function of `doup:plugin/host-api` lets a guest run a short lived container
//!   of any image, with its `/workspace`, a scratch `/container-init` dir & a shared `/container-cache` dir
//!   mounted in. The cache lives as long as the plugin handle & lets a plugin keep an ecosystem's tooling
//!   caches between runs without the host knowing anything about them. This lets a
//!   plugin lean on an ecosystem's own tooling (eg: `cargo update`) rather than re-implementing it.
//!   Runs are denied unless a session is created with `ContainerPolicy::Allowed`, currently only
//!   `apply-updates` is. Containers are run with Docker (or anything compatible), configured by the
//!   usual `DOCKER_HOST` etc. environment variables.
//!
//! - **Host API**: the `doup:plugin/host-api` imports, which are provided to every plugin.
//!
//! Every capability beyond WASI, and the WASI context itself, is described by the `SessionPolicy` a session is created with.
//! Each world provides a default policy (`PluginWorld::default_policy`) that pooled sessions use as is
//! and that call specific policies build on.
//!
//! By default stdout and stderr are inherited so plugin output is visible to the user.
//!
//! The `configure` export is the exception: it always runs with no I/O at all (no HTTP, containers, files, stdout
//! or stderr), whatever the policy says, and the policy is only applied once it has returned. Its only job is to
//! parse its config and keep it in the guest's memory.
//!
//! # Errors
//!
//! Every fallible function returns a [`PluginError`], one variant per way loading or calling a
//! plugin can fail, eg: [`PluginError::NotFound`] or [`PluginError::Trap`] when a guest traps.
//!
//! Plugins return errors as the WIT `guest-error` record (`doup:plugin/types`), which is surfaced as
//! a [`GuestError`] by [`PluginError::Configure`] & [`PluginError::Guest`]. Its [`GuestErrorKind`]
//! categorises the failure (eg: `http`, `not-found` or `invalid-workspace`) so callers can react to
//! it without parsing messages, and the causes the plugin reported form its
//! [`source`](std::error::Error::source) chain. Render with `{:#}` (anyhow) to see the whole chain.
//!
//! Likewise `host-api.run` fails with a typed `run-error`, so a guest can tell a command that exited
//! with a non-zero code apart from containers not being permitted or the runtime being unavailable.
//!
//! # Remote plugins
//!
//! A plugin's `use` may be a wasm package reference (`namespace:package@version`) rather than a path, eg:
//! `doup:ecosystem-plugin-cargo@0.1.0`. Package references are always pinned by a `doup.lock` file (see
//! [`doup_config::lock`]), which `doup lock` writes. A [`ModuleResolver`] only ever runs the exact release the
//! lock pins, failing with [`PluginError::NotLocked`] otherwise, & the [`wkg::PackageClient`] it uses verifies
//! every download against its locked digest. Downloads are cached on disk, see the [`wkg`] module.
//!
//! # Adding a world
//!
//! Implement the `world::PluginWorld` trait for a marker type (see the existing ecosystem and
//! platform plugins), then wrap a `session::PluginSessionPool` of it in a public handle type.
//!
//! [wasmtime]: https://docs.wasmtime.dev

mod error;
mod host;
mod resolver;
mod session;
pub mod wkg;
mod world;

pub use error::{BoxError, GuestError, GuestErrorKind, PluginError, WasmtimeError};
pub use resolver::ModuleResolver;

pub mod ecosystem_plugin;
pub use ecosystem_plugin::EcosystemPlugin;

pub mod platform_plugin;
pub use platform_plugin::PlatformPlugin;
