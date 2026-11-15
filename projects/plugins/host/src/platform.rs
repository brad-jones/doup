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
  world: "platform",
  exports: { default: async },
  with: { "doup:plugin/host-api": super::host::host_api },
});

// Doup's customised bindings layer for the platform plugin.
// These bindings deal with some of the lower level aspects of WASM,
// such as the Store, in an attempt to make WASM calls feel as native
// as possible.
//
// see: `./projects/plugins/macro`
plugin_wrapper! {
  wit = "../wit",
  world = "platform",
  bindings = Platform,
  name = PlatformWrapper,
  state = super::host::HostState,
}

/// A third and final wrapper, that wraps the PlatformWrapper.
///
/// This is manually written and provides an even better developer experience
/// for the caller, mainly to deal with the double Result<Result<>> shape.
///
/// Maybe one day in the future the functionality here might get merged into the
/// plugin_wrapper macro once we reach something approaching a stable API.
pub struct PlatformPlugin {
  /// Give direct access to the wrapper should the caller wish
  /// to do something custom that we currently do not support.
  pub wrapper: PlatformWrapper,
}

impl PlatformPlugin {
  pub async fn load(runtime: &PluginRuntime, plugin: &DoupRepoPlugin, repo_dir: Option<&Path>) -> Result<Self> {
    let plugin_path = resolve_plugin_path(&plugin.plugin, repo_dir)?;
    let plugin_pre = PlatformPre::new(runtime.platform_pre(&plugin_path)?)?;

    let mut wrapper = PlatformWrapper::new(plugin_pre, |_wasi| {
      // Set custom WASI permissions for this plugin type here
    });

    if let Err(err) = wrapper.configure(&plugin_config_json(plugin)?).await? {
      bail!("platform plugin {} rejected its config: {err}", plugin.plugin);
    }

    Ok(Self { wrapper })
  }

  pub async fn hello_world(&mut self) -> Result<String> {
    Ok(self.wrapper.hello_world().await?)
  }
}
