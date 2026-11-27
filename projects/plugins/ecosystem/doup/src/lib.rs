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

use std::sync::OnceLock;

static CONFIG: OnceLock<serde_json::Value> = OnceLock::new();

struct DoupEcosystemPluginDoup;

impl Guest for DoupEcosystemPluginDoup {
  fn configure(config: String) -> Result<(), String> {
    let value = serde_json::from_str(&config).map_err(|err| format!("invalid config: {err}"))?;
    CONFIG.set(value).map_err(|_| "already configured".to_string())
  }

  async fn discover_deps() -> Result<Vec<Dependency>, String> {
    Ok(Vec::new())
  }

  async fn lookup_versions(_dependency: Dependency) -> Result<Option<Versions>, String> {
    Ok(None)
  }

  async fn fetch_release_notes(_dependency: Dependency, _to: String) -> Result<Vec<ReleaseNote>, String> {
    Ok(Vec::new())
  }

  async fn apply_updates(_dependency: Dependency, _to: String) -> Result<(), String> {
    Ok(())
  }
}

export!(DoupEcosystemPluginDoup);
