// Generated code, the canonical ABI flattens `fetch-release-notes(dependency, to)` into 9 core wasm params.
#[allow(clippy::too_many_arguments)]
mod bindings {
  wit_bindgen::generate!({
    world: "ecosystem",
    path: "../../wit",
    default_bindings_module: "crate::bindings",
  });
}

use bindings::*;
use doup::plugin::types::ErrorKind;

use std::sync::OnceLock;

static CONFIG: OnceLock<serde_json::Value> = OnceLock::new();

struct DoupEcosystemPluginDoup;

impl Guest for DoupEcosystemPluginDoup {
  fn configure(config: String) -> Result<(), GuestError> {
    let value = serde_json::from_str(&config).map_err(|err| invalid_config("invalid config", vec![err.to_string()]))?;
    CONFIG.set(value).map_err(|_| invalid_config("already configured", Vec::new()))
  }

  async fn discover_deps() -> Result<Vec<Dependency>, GuestError> {
    Ok(Vec::new())
  }

  async fn lookup_versions(_dependency: Dependency) -> Result<Option<Versions>, GuestError> {
    Ok(None)
  }

  async fn fetch_release_notes(_dependency: Dependency, _to: String) -> Result<Vec<ReleaseNote>, GuestError> {
    Ok(Vec::new())
  }

  async fn apply_updates(_dependency: Dependency, _to: String) -> Result<(), GuestError> {
    Ok(())
  }
}

fn invalid_config(message: &str, causes: Vec<String>) -> GuestError {
  GuestError { kind: ErrorKind::InvalidConfig, message: message.to_string(), causes }
}

export!(DoupEcosystemPluginDoup);
