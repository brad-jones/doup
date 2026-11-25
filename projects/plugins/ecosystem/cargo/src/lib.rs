wit_bindgen::generate!({
    world: "ecosystem",
    path: "../../wit",
});

mod discover;
mod lookup;
mod registry;

use std::{path::Path, sync::OnceLock};
use wasi_fetch::Client;

static CONFIG: OnceLock<serde_json::Value> = OnceLock::new();

struct DoupEcosystemPluginCargo;

impl Guest for DoupEcosystemPluginCargo {
  fn configure(config: String) -> Result<(), String> {
    let value = serde_json::from_str(&config).map_err(|err| format!("invalid config: {err}"))?;
    CONFIG.set(value).map_err(|_| "already configured".to_string())
  }

  async fn discover_deps() -> Result<Vec<Dependency>, String> {
    discover::discover(Path::new("/workspace"))
  }

  async fn lookup_versions(dependency: Dependency) -> Result<Option<Versions>, String> {
    let url = registry::index_url(dependency.registry.as_deref(), &dependency.name);

    let response = Client::new()
      .get(&url)
      .header("User-Agent", concat!("doup/", env!("CARGO_PKG_VERSION")))
      .send()
      .await
      .map_err(|err| format!("request to {url} failed: {err}"))?;

    if !response.status().is_success() {
      return Err(format!("request to {url} returned {}", response.status()));
    }

    let index = response.into_body().text().await.map_err(|err| format!("failed to read {url}: {err}"))?;

    lookup::select(&dependency, &index)
  }
}

export!(DoupEcosystemPluginCargo);
