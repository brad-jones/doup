use super::host::{HostState, host_api};
use std::path::Path;
use wasmtime::component::{Component, HasSelf, Linker};
use wasmtime::{Config, Engine, Result, Store};
use wasmtime_wasi::WasiCtxBuilder;

/// Everything needed to instantiate a plugin; the store moves into the plugin wrapper.
pub struct Loaded {
  pub store: Store<HostState>,
  pub component: Component,
  pub linker: Linker<HostState>,
}

pub fn load(path: &Path, configure_wasi: impl FnOnce(&mut WasiCtxBuilder)) -> Result<Loaded> {
  let mut config = Config::new();
  config.wasm_component_model(true);
  let engine = Engine::new(&config)?;
  let component = Component::from_file(&engine, path)?;
  let store = Store::new(&engine, HostState::new(configure_wasi));
  let mut linker: Linker<HostState> = Linker::new(&engine);
  wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;
  host_api::add_to_linker::<HostState, HasSelf<HostState>>(&mut linker, |state| state)?;
  Ok(Loaded {
    store,
    component,
    linker,
  })
}
