use super::host::{HostState, host_api};
use crate::plugin::{IBindings, IPre, Plugin, PluginSession};
use anyhow::Result;
use std::{path::Path, sync::OnceLock};
use wasmtime::{
  Engine,
  component::{HasSelf, Linker, bindgen},
};
use wasmtime_wasi::p2::add_to_linker_async;

bindgen!({
  path: "../wit",
  world: "ecosystem",
  exports: { default: async },
  with: { "doup:plugin/host-api": super::host::host_api },
});

impl IPre<HostState, Ecosystem> for EcosystemPre<HostState> {
  fn new(instance_pre: wasmtime::component::InstancePre<HostState>) -> wasmtime::Result<Self> {
    EcosystemPre::new(instance_pre)
  }

  async fn instantiate_async(
    &self,
    store: impl wasmtime::AsContextMut<Data = HostState>,
  ) -> wasmtime::Result<Ecosystem> {
    EcosystemPre::instantiate_async(self, store).await
  }
}

impl IBindings<HostState> for Ecosystem {
  async fn call_configure(
    &self,
    store: &mut wasmtime::Store<HostState>,
    arg0: &str,
  ) -> wasmtime::Result<Result<(), wasmtime::component::__internal::String>> {
    Ecosystem::call_configure(self, store, arg0).await
  }
}

static CACHED_LINKER: OnceLock<Linker<HostState>> = OnceLock::new();

fn linker(engine: &Engine) -> Result<Linker<HostState>> {
  if let Some(linker) = CACHED_LINKER.get() {
    return Ok(linker.clone());
  }

  let mut linker = Linker::new(&engine);
  add_to_linker_async(&mut linker)?;
  host_api::add_to_linker::<HostState, HasSelf<HostState>>(&mut linker, |state| state)?;
  let _ = CACHED_LINKER.set(linker.clone());
  Ok(linker)
}

pub struct EcosystemPlugin {
  plugin: Plugin<HostState, EcosystemPre<HostState>, Ecosystem>,
}

impl EcosystemPlugin {
  pub fn new(engine: &Engine, path: &Path) -> Result<Self> {
    Ok(Self {
      plugin: Plugin::new(engine, &linker(engine)?, path, |wasi| {
        wasi.inherit_stdout().inherit_stderr();
      }),
    })
  }

  pub async fn hello_world(&self) -> Result<String> {
    let PluginSession(mut store, bindings) = self.plugin.checkout_session().await?;
    let result = bindings.call_hello_world(&mut store).await;
    self.plugin.checkin_session(PluginSession(store, bindings), result)
  }
}
