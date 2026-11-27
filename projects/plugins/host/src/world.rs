//! Abstraction over the WIT worlds a plugin component can target.
//!
//! The host loads WASM components for more than one world (currently ecosystem and platform
//! plugins). Each world has its own generated bindings and pre-instantiated type, but the
//! lifecycle around them is identical: compile the component, build a [`Linker`] with the host
//! imports, instantiate it per session, then call its `configure` export.
//!
//! [`PluginWorld`] captures only the parts that differ between worlds, implemented by a marker
//! type per world, so the rest of the host can be written once and be generic over the world.
//! The trait also provides the default linker (WASI p3, `wasi:http` and the host API) and the
//! default WASI permissions, both of which a world may override.

use crate::host::types::GuestError;
use crate::host::{HostState, host_api};
use std::path::Path;
use wasmtime::component::{HasSelf, Linker};
use wasmtime::{Engine, Result, Store};
use wasmtime_wasi::WasiCtxBuilder;
use wasmtime_wasi::p3::add_to_linker;
use wasmtime_wasi_http::p3::add_to_linker as add_http_to_linker;

/// What differs between plugin worlds. Implemented by a marker type per world.
/// See EcosystemWorld & PlatformWorld for the two current implemented worlds.
pub trait PluginWorld: 'static {
  /// The compiled plugin, cheap to instantiate many times (eg: `EcosystemPre<HostState>`).
  type Pre;

  /// The generated bindings for one instance (eg: `Ecosystem`).
  type Bindings;

  /// Factory for [`PluginWorld::Pre`], typical implementation:
  ///
  /// ```ignore
  /// fn pre(engine: &Engine, linker: &Linker<HostState>, path: &Path) -> Result<Self::Pre> {
  ///   let component = Component::from_file(engine, path)?;
  ///   let pre = {{WORLD_NAME}}Pre::new(linker.instantiate_pre(&component)?)?;
  ///   Ok(pre)
  /// }
  /// ```
  fn pre(engine: &Engine, linker: &Linker<HostState>, path: &Path) -> Result<Self::Pre>;

  /// Factory for [`PluginWorld::Bindings`], typical implementation:
  ///
  /// ```ignore
  /// async fn instantiate(pre: &Self::Pre, store: &mut Store<HostState>) -> Result<Self::Bindings> {
  ///   Ok(pre.instantiate_async(store).await?)
  /// }
  /// ```
  fn instantiate(pre: &Self::Pre, store: &mut Store<HostState>) -> impl Future<Output = Result<Self::Bindings>> + Send;

  /// Calls the world's `configure` export as part of session creation so
  /// that a guest component is always configured before being used.
  ///
  /// Typical implementation:
  ///
  /// ```ignore
  /// async fn configure(
  ///   bindings: &Self::Bindings,
  ///   store: &mut Store<HostState>,
  ///   config: &str,
  /// ) -> wasmtime::Result<Result<(), GuestError>> {
  ///   bindings.call_configure(store, config).await
  /// }
  /// ```
  fn configure(
    bindings: &Self::Bindings,
    store: &mut Store<HostState>,
    config: &str,
  ) -> impl Future<Output = Result<std::result::Result<(), GuestError>>> + Send;

  /// The default linker that will be used, worlds are free to override.
  /// The result from this method is cached by [`crate::session::PluginSessionPool`].
  ///
  /// `wasi:http` is always linked because a component that imports it can't be instantiated otherwise.
  /// Whether requests actually go out is decided per session by [`crate::host::HttpPolicy`].
  ///
  /// If your world requires something different feel free to override.
  fn linker(engine: &Engine) -> Result<Linker<HostState>> {
    let mut linker = Linker::new(engine);
    add_to_linker(&mut linker)?;
    add_http_to_linker(&mut linker)?;
    host_api::add_to_linker::<HostState, HasSelf<HostState>>(&mut linker, |state| state)?;
    Ok(linker)
  }

  /// Override to give a world different WASI permissions.
  fn wasi(builder: &mut WasiCtxBuilder) -> Result<&mut WasiCtxBuilder> {
    Ok(builder.inherit_stdout().inherit_stderr())
  }
}
