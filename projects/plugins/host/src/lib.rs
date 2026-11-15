use anyhow::Result;
use doup_config::repo::DoupRepoPlugin;

mod host;
mod resolve;
mod runtime;
pub use runtime::PluginRuntime;

mod platform;
pub use platform::PlatformPlugin;

mod ecosystem;
pub use ecosystem::EcosystemPlugin;

fn plugin_config_json(plugin: &DoupRepoPlugin) -> Result<String> {
  Ok(serde_json::to_string(&plugin.with.clone().unwrap_or_default())?)
}
