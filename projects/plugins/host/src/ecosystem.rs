use super::host::HostState;
use super::plugin_config_json;
use super::resolve::resolve_plugin_path;
use super::runtime::PluginRuntime;
use anyhow::Result;
use doup_config::repo::DoupRepoPlugin;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::RwLock;
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
  session: RwLock<Option<Arc<(Store<HostState>, Ecosystem)>>>,
  repo_dir: Option<PathBuf>,
  configure: Arc<dyn Fn(&mut WasiCtxBuilder) + Send + Sync>,
}

impl EcosystemPlugin {
  pub fn new(runtime: &Arc<PluginRuntime>, plugin: &DoupRepoPlugin, repo_dir: Option<PathBuf>) -> Self {
    Self {
      runtime: Arc::clone(runtime),
      plugin: plugin.clone(),
      pre: OnceLock::new(),
      session: RwLock::new(None),
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

  async fn get_session(&self) -> Result<Arc<(Store<HostState>, Ecosystem)>> {
    if let Some(session) = self.session.read().unwrap().as_ref() {
      return Ok(Arc::clone(session));
    }

    let mut store = Store::new(&self.runtime.engine, HostState::new(&*self.configure));
    let bindings = self.get_pre()?.instantiate_async(&mut store).await?;

    let session = Arc::new((store, bindings));
    *self.session.write().unwrap() = Some(Arc::clone(&session));
    Ok(session)
  }

  fn clear_session(&self) {
    *self.session.write().unwrap() = None;
  }

  async fn configure(&mut self) -> Result<()> {
    let config = plugin_config_json(&self.plugin)?;
    let (mut store, bindings) = self.get_session().await?.as_ref();
    let result = bindings.call_configure(&mut store, &config).await;
    if result.is_err() {
      self.clear_session();
    }

    match result {
      Ok(v) => v,
      Err(v) => v,
    }
  }
}
