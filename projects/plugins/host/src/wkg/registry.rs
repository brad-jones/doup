//! The wasm-pkg [`Config`]s that decide which registry a package is downloaded from.

use serde_json::json;
use wasm_pkg_client::{Config, CustomConfig, Error, PackageRef, Registry, RegistryMapping, RegistryMetadata};

/// Names a wkg config file that is merged over every other, the same env var `wkg` itself reads.
pub const CONFIG_FILE_ENV: &str = "WKG_CONFIG_FILE";

/// Where doup's own plugins are published, the same mapping as `wasm-pkg.toml` at the root of the doup repo.
/// Built in so that `doup:*` packages work without any wkg config.
const BUILT_IN: &str = r#"
[namespace_registries]
doup = { registry = "doup", metadata = { preferredProtocol = "oci", oci = { registry = "ghcr.io", namespacePrefix = "brad-jones/" } } }
"#;

/// The config for packages without a `registry` of their own. In order, later sources winning:
/// wasm-pkg's defaults, doup's built-in namespace, the user's global wkg config & the file named by
/// [`CONFIG_FILE_ENV`], so users can still redirect (eg: mirror) any namespace.
pub(super) async fn default_config() -> Result<Config, Error> {
  let mut config = Config::default();
  config.merge(Config::from_toml(BUILT_IN)?);
  if let Some(global) = Config::read_global_config().await? {
    config.merge(global);
  }
  if let Some(path) = std::env::var_os(CONFIG_FILE_ENV).filter(|path| !path.is_empty()) {
    config.merge(Config::from_file(path).await?);
  }
  Ok(config)
}

/// A config that resolves `package`'s namespace to `registry` alone, see `DoupRepoPlugin::registry`:
///
/// - `host/prefix`: an OCI registry at `host`, with packages at `prefix/<namespace>/<name>`.
/// - `host`: a wasm-pkg registry, whose metadata is fetched from `host`, falling back to OCI.
///
/// It starts empty, rather than from `base`, so that no namespace mapping in `base` can leak in.
/// Only `base`'s settings for the registry itself (eg: credentials) are kept.
pub(super) fn explicit_config(base: &Config, package: &PackageRef, registry: &str) -> Result<Config, Error> {
  let (host, mapping) = match registry.split_once('/') {
    Some((host, prefix)) => {
      let metadata: RegistryMetadata = serde_json::from_value(json!({
        "preferredProtocol": "oci",
        "oci": { "registry": host, "namespacePrefix": format!("{prefix}/") },
      }))
      .map_err(|err| Error::InvalidRegistryMetadata(err.into()))?;
      let host: Registry = host.parse()?;
      (host.clone(), RegistryMapping::Custom(CustomConfig { registry: host, metadata }))
    }
    None => {
      let host: Registry = registry.parse()?;
      (host.clone(), RegistryMapping::Registry(host))
    }
  };

  let mut config = Config::empty();
  if let Some(registry_config) = base.registry_config(&host) {
    *config.get_or_insert_registry_config_mut(&host) = registry_config.clone();
  }
  config.set_namespace_registry(package.namespace().clone(), mapping);
  Ok(config)
}

#[cfg(test)]
mod tests {
  use super::*;

  fn package(name: &str) -> PackageRef {
    name.parse().unwrap()
  }

  #[test]
  fn built_in_config_maps_the_doup_namespace() {
    let mut config = Config::empty();
    config.merge(Config::from_toml(BUILT_IN).unwrap());
    let registry = config.resolve_registry(&package("doup:ecosystem-plugin-cargo")).unwrap();
    assert_eq!(registry.to_string(), "doup");
  }

  #[test]
  fn explicit_registries_map_only_the_packages_namespace() {
    let base = Config::from_toml(BUILT_IN).unwrap();
    for (registry, host) in
      [("ghcr.io/acme", "ghcr.io"), ("localhost:5000/a/b", "localhost:5000"), ("wa.dev", "wa.dev")]
    {
      let config = explicit_config(&base, &package("doup:cargo"), registry).unwrap();
      assert_eq!(config.resolve_registry(&package("doup:cargo")).unwrap().to_string(), host, "{registry}");
      assert_eq!(config.resolve_registry(&package("other:cargo")), None, "{registry}");
    }
    let Some(RegistryMapping::Custom(custom)) = explicit_config(&base, &package("doup:cargo"), "ghcr.io/acme/x")
      .unwrap()
      .namespace_registry(&"doup".parse().unwrap())
      .cloned()
    else {
      panic!("expected a custom mapping");
    };
    let oci = custom.metadata.protocol_config::<serde_json::Value>("oci").unwrap().unwrap();
    assert_eq!(oci, json!({ "registry": "ghcr.io", "namespacePrefix": "acme/x/" }));
  }

  #[test]
  fn invalid_registries_are_rejected() {
    assert!(explicit_config(&Config::empty(), &package("doup:cargo"), "not a host").is_err());
  }
}
