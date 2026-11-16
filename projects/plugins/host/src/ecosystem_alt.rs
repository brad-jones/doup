use super::host::HostState;
use super::runtime::PluginRuntime;
use super::session::{SessionPool, World};
use anyhow::Result;
use doup_config::repo::DoupRepoPlugin;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use wasmtime::Store;
use wasmtime::component::bindgen;

bindgen!({
  path: "../wit",
  world: "ecosystem",
  exports: { default: async },
  with: { "doup:plugin/host-api": super::host::host_api },
});

/// Connects the generated `ecosystem` bindings to the shared session pool.
struct EcosystemWorld;

impl World for EcosystemWorld {
  type Pre = EcosystemPre<HostState>;
  type Bindings = Ecosystem;

  fn pre(runtime: &PluginRuntime, wasm_path: &Path) -> Result<Self::Pre> {
    Ok(EcosystemPre::new(runtime.ecosystem_pre(wasm_path)?)?)
  }

  async fn instantiate(pre: &Self::Pre, store: &mut Store<HostState>) -> Result<Self::Bindings> {
    Ok(pre.instantiate_async(store).await?)
  }

  async fn configure(
    bindings: &Self::Bindings,
    store: &mut Store<HostState>,
    config: &str,
  ) -> wasmtime::Result<Result<(), String>> {
    bindings.call_configure(store, config).await
  }
}

/// An ecosystem plugin that can be called from many tasks at once.
pub struct EcosystemPluginAlt {
  pool: SessionPool<EcosystemWorld>,
}

impl EcosystemPluginAlt {
  pub fn new(runtime: &Arc<PluginRuntime>, plugin: &DoupRepoPlugin, search_dirs: Vec<PathBuf>) -> Self {
    Self {
      pool: SessionPool::new(runtime, plugin, search_dirs),
    }
  }

  pub async fn hello_world(&self) -> Result<String> {
    let mut session = self.pool.checkout().await?;
    let result = session.bindings.call_hello_world(&mut session.store).await;
    self.pool.checkin(session, result)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn assert_send<T: Send>(_: T) {}

  // Callers need to be able to `tokio::spawn` the call.
  #[test]
  fn calls_are_send() {
    let runtime = Arc::new(PluginRuntime::new().unwrap());
    let plugin = DoupRepoPlugin {
      plugin: "missing.wasm".into(),
      with: None,
    };
    let wrapper = EcosystemPluginAlt::new(&runtime, &plugin, vec![]);
    assert_send(wrapper.hello_world());
  }

  #[tokio::test]
  async fn missing_plugin_is_reported_on_first_call_not_construction() {
    let runtime = Arc::new(PluginRuntime::new().unwrap());
    let plugin = DoupRepoPlugin {
      plugin: "./missing.wasm".into(),
      with: None,
    };
    let wrapper = EcosystemPluginAlt::new(&runtime, &plugin, vec![]);
    let err = wrapper.hello_world().await.unwrap_err();
    assert!(err.to_string().contains("plugin not found"), "{err}");
  }
}
