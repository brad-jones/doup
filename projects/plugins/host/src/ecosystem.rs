use super::plugin_config_json;
use super::resolve::resolve_plugin_path;
use super::runtime::PluginRuntime;
use anyhow::{Result, bail};
use doup_config::repo::DoupRepoPlugin;
use doup_plugin_macro::plugin_wrapper;
use std::path::Path;
use wasmtime::component::bindgen;

// The original wasmtime generated bindings for the platform plugin.
bindgen!({
  path: "../wit",
  world: "ecosystem",
  exports: { default: async },
  with: { "doup:plugin/host-api": super::host::host_api },
});

// Doup's customised bindings layer for the ecosystem plugin.
// These bindings deal with some of the lower level aspects of WASM,
// such as the Store, in an attempt to make WASM calls feel as native
// as possible.
//
// see: `./projects/plugins/macro`
plugin_wrapper! {
  wit = "../wit",
  world = "ecosystem",
  bindings = Ecosystem,
  name = EcosystemWrapper,
  state = super::host::HostState,
}

/// A third and final wrapper, that wraps the EcosystemWrapper.
///
/// This is manually written and provides an even better developer experience
/// for the caller, mainly to deal with the double Result<Result<>> shape.
///
/// Maybe one day in the future the functionality here might get merged into the
/// plugin_wrapper macro once we reach something approaching a stable API.
pub struct EcosystemPlugin {
  /// Give direct access to the wrapper should the caller wish
  /// to do something custom that we currently do not support.
  pub wrapper: EcosystemWrapper,
}

impl EcosystemPlugin {
  pub async fn load(
    runtime: &PluginRuntime,
    plugin: &DoupRepoPlugin,
    repo_dir: Option<&Path>,
  ) -> Result<Self> {
    let plugin_path = resolve_plugin_path(&plugin.plugin, repo_dir)?;
    let plugin_pre = EcosystemPre::new(runtime.ecosystem_pre(&plugin_path)?)?;

    let mut wrapper = EcosystemWrapper::new(plugin_pre, |_wasi| {
      // Set custom WASI permissions for this plugin type here
    });

    if let Err(err) = wrapper.configure(&plugin_config_json(plugin)?).await? {
      bail!(
        "ecosystem plugin {} rejected its config: {err}",
        plugin.plugin
      );
    }

    Ok(Self { wrapper })
  }

  pub async fn hello_world(&mut self) -> Result<String> {
    Ok(self.wrapper.hello_world().await?)
  }
}
