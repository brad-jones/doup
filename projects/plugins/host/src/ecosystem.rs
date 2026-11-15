use super::runtime::PluginRuntime;
use doup_plugin_macro::plugin_wrapper;
use std::path::Path;
use wasmtime::Result;
use wasmtime::component::bindgen;

bindgen!({
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

pub fn load_plugin(runtime: &PluginRuntime, path: &Path) -> Result<EcosystemPlugin> {
  Ok(EcosystemPlugin::new(
    EcosystemPre::new(runtime.ecosystem_pre(path)?)?,
    |_wasi| {
      // Set custom WASI permissions for this plugin type here
    },
  ))
}
