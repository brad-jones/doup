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

mod apply;
mod config;
mod discover;
mod error;
mod fetch;
mod http;
mod lookup;
mod manifest;
mod registry;
mod release_notes;
mod requirement;

use std::path::Path;

struct DoupEcosystemPluginCargo;

impl Guest for DoupEcosystemPluginCargo {
  fn configure(config: String) -> Result<(), GuestError> {
    Ok(config::init(&config)?)
  }

  async fn discover_deps() -> Result<Vec<Dependency>, GuestError> {
    Ok(discover::discover(Path::new("/workspace"))?)
  }

  async fn lookup_versions(dependency: Dependency) -> Result<Option<Versions>, GuestError> {
    let url = registry::index_url(dependency.registry.as_deref(), &dependency.name);
    let index = http::get(&url, &[]).await?.into_success(&url)?;
    Ok(lookup::select(&dependency, &index)?)
  }

  async fn fetch_release_notes(dependency: Dependency, to: String) -> Result<Vec<ReleaseNote>, GuestError> {
    Ok(fetch::release_notes(&dependency, &to).await?)
  }

  async fn apply_updates(dependency: Dependency, to: String) -> Result<(), GuestError> {
    Ok(apply::apply(Path::new("/workspace"), Path::new("/container-init"), &dependency, &to).await?)
  }
}

export!(DoupEcosystemPluginCargo);
