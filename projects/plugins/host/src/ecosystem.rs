use super::host::HostState;
use super::plugin_config_json;
use super::resolve::resolve_plugin_path;
use super::runtime::PluginRuntime;
use anyhow::Result;
use doup_config::repo::DoupRepoPlugin;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::OnceLock;
use tokio::sync::Mutex;
use wasmtime::Store;
use wasmtime::component::bindgen;
use wasmtime_wasi::WasiCtxBuilder;

bindgen!({
  path: "../wit",
  world: "ecosystem",
  exports: { default: async },
  with: { "doup:plugin/host-api": super::host::host_api },
});

pub struct EcosystemPlugin {
  runtime: Arc<PluginRuntime>,
  plugin: DoupRepoPlugin,
  wasm_module_path: OnceLock<PathBuf>,
  pre: OnceLock<EcosystemPre<HostState>>,
  session: Mutex<Option<(Store<HostState>, Ecosystem)>>,
  repo_dir: Option<PathBuf>,
  configure: Arc<dyn Fn(&mut WasiCtxBuilder) + Send + Sync>,
}

impl EcosystemPlugin {
  pub fn new(runtime: &Arc<PluginRuntime>, plugin: &DoupRepoPlugin, repo_dir: Option<PathBuf>) -> Self {
    Self {
      runtime: Arc::clone(runtime),
      plugin: plugin.clone(),
      pre: OnceLock::new(),
      session: Mutex::new(None),
      wasm_module_path: OnceLock::new(),
      repo_dir,
      configure: Arc::new(|wasi| {
        wasi.inherit_stdout().inherit_stderr();
      }),
    }
  }

  fn resolve_plugin_path(&self) -> Result<&Path> {
    if let Some(wasm_module_path) = self.wasm_module_path.get() {
      return Ok(wasm_module_path);
    }

    let wasm_module_path = resolve_plugin_path(
      &self.plugin.plugin,
      match &self.repo_dir {
        Some(dir) => Some(dir.as_path()),
        None => None,
      },
    )?;

    Ok(self.wasm_module_path.get_or_init(|| wasm_module_path))
  }

  fn get_pre(&self) -> Result<&EcosystemPre<HostState>> {
    if let Some(pre) = self.pre.get() {
      return Ok(pre);
    }

    let wasm_module_path = self.resolve_plugin_path()?;
    let instance_pre = self.runtime.ecosystem_pre(wasm_module_path)?;
    let pre = EcosystemPre::new(instance_pre)?;

    Ok(self.pre.get_or_init(|| pre))
  }

  async fn new_session(&self) -> Result<(Store<HostState>, Ecosystem)> {
    let mut store = Store::new(&self.runtime.engine, HostState::new(&*self.configure));
    let bindings = self.get_pre()?.instantiate_async(&mut store).await?;
    Ok((store, bindings))
  }

  async fn configure(&self) -> Result<()> {
    let config = plugin_config_json(&self.plugin)?;
    let mut guard = self.session.lock().await;

    // Taking the session out means a failed call drops it, so the next call starts fresh.
    let (mut store, bindings) = match guard.take() {
      Some(session) => session,
      None => self.new_session().await?,
    };

    bindings
      .call_configure(&mut store, &config)
      .await?
      .map_err(anyhow::Error::msg)?;

    *guard = Some((store, bindings));
    Ok(())
  }
}
