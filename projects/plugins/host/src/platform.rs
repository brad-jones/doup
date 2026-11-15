use super::loader;
use doup_plugin_macro::plugin_wrapper;
use std::path::Path;
use wasmtime::Result;

wasmtime::component::bindgen!({
  path: "../wit",
  world: "platform",
  exports: { default: async },
  with: { "doup:plugin/host-api": super::host::host_api },
});

plugin_wrapper! {
  wit = "../wit",
  world = "platform",
  bindings = Platform,
  name = PlatformPlugin,
  state = super::host::HostState,
}

pub async fn load_plugin(path: &Path) -> Result<PlatformPlugin> {
  let mut loaded = loader::load(path, |_wasi| {})?;
  let bindings =
    Platform::instantiate_async(&mut loaded.store, &loaded.component, &loaded.linker).await?;
  Ok(PlatformPlugin::new(loaded.store, bindings))
}
