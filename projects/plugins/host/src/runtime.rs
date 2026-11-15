use super::host::{HostState, host_api};
use std::path::Path;
use wasmtime::component::{Component, HasSelf, InstancePre, Linker};
use wasmtime::{Config, Engine, Result};

/// Process-wide wasmtime state: one `Engine` and one `Linker` per plugin world.
pub struct PluginRuntime {
  engine: Engine,
  ecosystem_linker: Linker<HostState>,
  platform_linker: Linker<HostState>,
}

impl PluginRuntime {
  pub fn new() -> Result<Self> {
    let mut config = Config::new();
    config.wasm_component_model(true);
    let engine = Engine::new(&config)?;
    Ok(Self {
      ecosystem_linker: new_linker(&engine)?,
      platform_linker: new_linker(&engine)?,
      engine,
    })
  }

  /// Compiles the plugin once; the returned handle instantiates cheaply, any number of times.
  pub(crate) fn ecosystem_pre(&self, path: &Path) -> Result<InstancePre<HostState>> {
    self.pre(&self.ecosystem_linker, path)
  }

  pub(crate) fn platform_pre(&self, path: &Path) -> Result<InstancePre<HostState>> {
    self.pre(&self.platform_linker, path)
  }

  fn pre(&self, linker: &Linker<HostState>, path: &Path) -> Result<InstancePre<HostState>> {
    let component = Component::from_file(&self.engine, path)?;
    linker.instantiate_pre(&component)
  }
}

// Kept per world so the two can diverge (eg: only some worlds get wasi:http).
fn new_linker(engine: &Engine) -> Result<Linker<HostState>> {
  let mut linker = Linker::new(engine);
  wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;
  host_api::add_to_linker::<HostState, HasSelf<HostState>>(&mut linker, |state| state)?;
  Ok(linker)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn runtime_builds_and_rejects_missing_plugin() {
    let runtime = PluginRuntime::new().unwrap();
    assert!(runtime.ecosystem_pre(Path::new("missing.wasm")).is_err());
    assert!(runtime.platform_pre(Path::new("missing.wasm")).is_err());
  }
}
