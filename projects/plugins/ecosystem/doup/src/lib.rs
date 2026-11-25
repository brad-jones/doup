wit_bindgen::generate!({
    world: "ecosystem",
    path: "../../wit",
});

use std::sync::OnceLock;

static CONFIG: OnceLock<serde_json::Value> = OnceLock::new();

struct Plugin;

impl Guest for Plugin {
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
}

export!(Plugin);
