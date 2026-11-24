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

  fn discover_deps() -> Result<Vec<Dependency>, String> {
    todo!()
  }
}

export!(Plugin);
