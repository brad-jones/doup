# Writing a doup plugin

A doup plugin is a WebAssembly component that implements one of the worlds defined by the `doup:plugin` WIT package.
That package is published as an OCI artifact at `ghcr.io/brad-jones/doup/plugin`, so you do not need a copy of this repo
to build a plugin.

| World       | Implement this to...                                                      |
| ----------- | ------------------------------------------------------------------------- |
| `ecosystem` | find & update dependencies of a package ecosystem (cargo, npm, ...)       |
| `platform`  | create & manage pull requests on a hosting platform (GitHub, GitLab, ...) |

The rest of this document walks through an `ecosystem` plugin written in Rust. A `platform` plugin is identical except
for the world name.

## Prerequisites

- Rust with the `wasm32-wasip3` target (`rustup target add wasm32-wasip3`)
- [`wkg`](https://github.com/bytecodealliance/wasm-pkg-tools), used to fetch the WIT package from the registry

## 1. Tell wkg where the `doup` namespace lives

WIT package namespaces can not contain slashes, so `doup:plugin` can not be written as a full registry path. `wkg` needs
a one-off mapping from the `doup` namespace to the GitHub Container Registry. Put this in a `wasm-pkg.toml` in your
project:

```toml
[namespace_registries]
doup = { registry = "doup", metadata = { preferredProtocol = "oci", "oci" = { registry = "ghcr.io", namespacePrefix = "brad-jones/" } } }
```

and point `wkg` at it with `WKG_CONFIG_FILE=$PWD/wasm-pkg.toml`. Alternatively, put the same content in
`~/.config/wasm-pkg/config.toml` to apply it to every project on your machine. The package is public, so no credentials
are needed to fetch it.

## 2. Create the crate

```toml
# Cargo.toml
[package]
name = "my-plugin"
version = "0.1.0"
edition = "2024"

[lib]
crate-type = ["cdylib"]

[dependencies]
serde_json = "1"
wit-bindgen = { version = "0.63", features = ["async"] }
```

Create `wit/world.wit`, which defines your own world by including one of ours. Use the version of `doup:plugin` that
matches the doup host you are targeting.

```wit
package acme:my-plugin;

world my-plugin {
  include doup:plugin/ecosystem@0.1.1;
}
```

Fetch the dependency into `wit/deps/` (this also writes a `wkg.lock`, commit both):

```sh
wkg fetch wit
```

## 3. Implement the world

`generate_all` is required because the interfaces that your world includes come from another package.

```rust
// src/lib.rs
mod bindings {
  wit_bindgen::generate!({
    world: "my-plugin",
    path: "wit",
    generate_all,
    default_bindings_module: "crate::bindings",
  });
}

use bindings::*;
use doup::plugin::types::ErrorKind;

struct MyPlugin;

impl Guest for MyPlugin {
  fn configure(config: String) -> Result<(), GuestError> {
    // Parse & validate the repo config's `with`, no I/O is allowed here.
    let _config: serde_json::Value = serde_json::from_str(&config).map_err(|err| GuestError {
      kind: ErrorKind::InvalidConfig,
      message: "invalid config".into(),
      causes: vec![err.to_string()],
    })?;
    Ok(())
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

export!(MyPlugin);
```

The doc comments in the WIT package are the contract, in particular what each export may do (`configure` is pure, only
`apply-updates` may call `host-api.run`, and so on). They are in `wit/deps/doup-plugin-<version>/package.wit` after `wkg
fetch`.

## 4. Build

```sh
cargo build --release --target wasm32-wasip3
```

The plugin is `target/wasm32-wasip3/release/my_plugin.wasm`. Reference it from `doup.yaml` with `use:`, eg: `use:
./target/wasm32-wasip3/release/my_plugin.wasm`, while developing it.

## 5. Publish

Push the component to an OCI registry, eg: `wkg oci push ghcr.io/acme/my-namespace/my-plugin:0.1.0 my_plugin.wasm` (see
`scripts/wkg-publish-plugins.ts` for how this repo does it), & make the package public. Repos then use it by reference,
telling doup where it lives with `registry` (unless the user's wkg config already maps the namespace), & run `doup lock`
to pin it in `doup.lock`:

```yaml
ecosystems:
  - use: my-namespace:my-plugin@0.1.0
    registry: ghcr.io/acme
```

## Upgrading

Change the version in your `include`, then run `wkg update` to refresh `wit/deps/` and `wkg.lock`, and fix any compile
errors from the regenerated bindings.
