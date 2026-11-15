use super::loader;
use plugin_macro::plugin_wrapper;
use std::path::Path;
use wasmtime::Result;

wasmtime::component::bindgen!({
  path: "../wit",
  world: "ecosystem",
  exports: { default: async },
  with: { "doup:plugin/host-api": super::host::host_api },
});

plugin_wrapper! {
  wit = "../wit",
  world = "ecosystem",
  bindings = Ecosystem,
  name = EcosystemPlugin,
  state = super::host::HostState,
}

pub async fn load_plugin(path: &Path) -> Result<EcosystemPlugin> {
  let mut loaded = loader::load(path, |_wasi| {})?;
  let bindings =
    Ecosystem::instantiate_async(&mut loaded.store, &loaded.component, &loaded.linker).await?;
  Ok(EcosystemPlugin::new(loaded.store, bindings))
}
