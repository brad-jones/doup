wit_bindgen::generate!({
  world: "ecosystem",
  path: "../../wit",
});

mod config;
mod discover;
mod fetch;
mod http;
mod lookup;
mod registry;
mod release_notes;

use std::path::Path;

struct DoupEcosystemPluginCargo;

impl Guest for DoupEcosystemPluginCargo {
  fn configure(config: String) -> Result<(), String> {
    config::init(&config)
  }

  async fn discover_deps() -> Result<Vec<Dependency>, String> {
    discover::discover(Path::new("/workspace"))
  }

  async fn lookup_versions(dependency: Dependency) -> Result<Option<Versions>, String> {
    let url = registry::index_url(dependency.registry.as_deref(), &dependency.name);
    let index = http::get(&url, &[]).await?.into_success(&url)?;
    lookup::select(&dependency, &index)
  }

  async fn fetch_release_notes(dependency: Dependency, to: String) -> Result<Vec<ReleaseNote>, String> {
    fetch::release_notes(&dependency, &to).await
  }
}

export!(DoupEcosystemPluginCargo);
