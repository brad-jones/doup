wit_bindgen::generate!({
    world: "ecosystem",
    path: "../../wit",
});

mod discover;
mod lookup;
mod registry;

use std::{path::Path, sync::OnceLock};
use wstd::http::{Body, Client, Request};

static CONFIG: OnceLock<serde_json::Value> = OnceLock::new();

struct Plugin;

impl Guest for Plugin {
  fn configure(config: String) -> Result<(), String> {
    let value = serde_json::from_str(&config).map_err(|err| format!("invalid config: {err}"))?;
    CONFIG.set(value).map_err(|_| "already configured".to_string())
  }

  fn discover_deps() -> Result<Vec<Dependency>, String> {
    discover::discover(Path::new("/workspace"))
  }

  fn lookup_versions(dependency: Dependency) -> Result<Option<Versions>, String> {
    let url = registry::index_url(Path::new("/workspace"), &dependency)?;
    let index = wstd::runtime::block_on(fetch(&url))?;
    lookup::select(&dependency, &index)
  }
}

/// Fetches `url`, the host only allows this for `lookup-versions`.
async fn fetch(url: &str) -> Result<String, String> {
  let request = Request::get(url)
    .header("User-Agent", concat!("doup/", env!("CARGO_PKG_VERSION")))
    .body(Body::empty())
    .map_err(|err| format!("invalid request for {url}: {err}"))?;

  let mut response = Client::new().send(request).await.map_err(|err| format!("request to {url} failed: {err}"))?;
  if !response.status().is_success() {
    return Err(format!("request to {url} returned {}", response.status()));
  }

  let body = response.body_mut().str_contents().await.map_err(|err| format!("failed to read {url}: {err}"))?;
  Ok(body.to_string())
}

export!(Plugin);
