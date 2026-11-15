wit_bindgen::generate!({
    world: "platform",
    path: "../../wit",
});

use doup::plugin::host_api;
use std::sync::OnceLock;

static CONFIG: OnceLock<serde_json::Value> = OnceLock::new();

struct Plugin;

impl Guest for Plugin {
  fn configure(config: String) -> Result<(), String> {
    let value = serde_json::from_str(&config).map_err(|err| format!("invalid config: {err}"))?;
    CONFIG
      .set(value)
      .map_err(|_| "already configured".to_string())
  }

  fn hello_world() -> String {
    let version = host_api::host_version();
    let config = CONFIG
      .get()
      .map_or_else(|| "unconfigured".to_string(), |c| c.to_string());
    println!("[plugin GITHUB] Hello, {version}! config: {config}");
    "hello world".to_string()
  }
}

export!(Plugin);
