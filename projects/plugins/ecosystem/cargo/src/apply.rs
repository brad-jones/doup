//! Applies an update to the workspace.
//!
//! The manifests are bumped here, in Rust, as `toml_edit` keeps their formatting & it's cheap. The
//! lockfiles are then re-resolved by real cargo, run in a container by the host, rather than trying
//! to replicate cargo's resolver. The cargo commands are the same ones renovate runs.

use crate::{
  Dependency,
  bindings::doup::plugin::host_api::{self, RunRequest},
  config,
  discover::{self, LockPackage},
  manifest,
};
use semver::Version;
use std::{collections::BTreeSet, fs, path::Path};

/// Where the host mounts the guest's workspace inside the container.
const CONTAINER_WORKSPACE: &str = "/workspace";

/// Runs cargo, with any arguments given, in the container.
///
/// - The image's own toolchain is used, a `rust-toolchain.toml` in the workspace would otherwise
///   make rustup download the toolchain it names, cargo's resolver doesn't depend on it.
/// - `CARGO_HOME` lives in the container's `/container-cache`, which outlives a run, so the registry index
///   & crates are only downloaded once. The image's own `CARGO_HOME` can't be written to by the workspace's
///   owner the container runs as. Cargo locks the dir itself, so concurrent runs are safe.
const CARGO_SH: &str = r#"#!/bin/sh
set -eu
if command -v rustup >/dev/null 2>&1; then
  RUSTUP_TOOLCHAIN="$(rustup default | cut -d ' ' -f 1)"
  export RUSTUP_TOOLCHAIN
fi
export CARGO_HOME=/container-cache/cargo-home
exec cargo "$@"
"#;

/// Updates every occurrence of `dependency` beneath `root` to version `to`.
///
/// `init` is the guest's view of the `/container-init` dir mounted into the containers it runs.
pub async fn apply(root: &Path, init: &Path, dependency: &Dependency, to: &str) -> Result<(), String> {
  let to = Version::parse(to).map_err(|err| format!("invalid version `{to}` for {}: {err}", dependency.name))?;
  manifest::update(root, dependency, &to)?;

  let lock_paths = lock_paths(dependency);
  if lock_paths.is_empty() {
    return Ok(());
  }

  let script = init.join("cargo.sh");
  fs::write(&script, CARGO_SH).map_err(|err| format!("failed to write {}: {err}", script.display()))?;

  for lock_path in lock_paths {
    let manifest_path = container_manifest_path(lock_path);

    // Re-resolves just enough of the lockfile to satisfy the bumped manifests, like renovate does.
    cargo(&["update", "--manifest-path", &manifest_path, "--workspace"])
      .await
      .map_err(|err| format!("{lock_path}: {err}"))?;

    // A requirement that already allowed `to` wasn't changed by the bump, so its locked version
    // still needs moving to `to` explicitly.
    let packages = discover::parse_lock(&root.join(lock_path))?;
    for spec in precise_specs(&packages, dependency, lock_path, &to) {
      cargo(&["update", "--manifest-path", &manifest_path, "--package", &spec, "--precise", &to.to_string()])
        .await
        .map_err(|err| format!("{lock_path}: {err}"))?;
    }
  }
  Ok(())
}

/// The distinct lockfiles that lock an occurrence of `dependency`.
fn lock_paths(dependency: &Dependency) -> BTreeSet<&str> {
  dependency.occurrences.iter().filter_map(|o| o.lock_path.as_deref()).collect()
}

/// The container path of the workspace manifest that sits beside `lock_path`.
fn container_manifest_path(lock_path: &str) -> String {
  match lock_path.rsplit_once('/') {
    Some((dir, _)) => format!("{CONTAINER_WORKSPACE}/{dir}/Cargo.toml"),
    None => format!("{CONTAINER_WORKSPACE}/Cargo.toml"),
  }
}

/// Package ID specs, for `cargo update --precise`, of the versions of `dependency` that are still
/// locked by `lock_path` but are older than `to`.
///
/// Only the versions the occurrences were originally locked at are considered, other versions of
/// the same crate (eg: another major pulled in transitively) are left alone.
fn precise_specs(packages: &[LockPackage], dependency: &Dependency, lock_path: &str, to: &Version) -> Vec<String> {
  let originally_locked: BTreeSet<&str> = dependency
    .occurrences
    .iter()
    .filter(|o| o.lock_path.as_deref() == Some(lock_path))
    .filter_map(|o| o.locked_version.as_deref())
    .collect();

  let specs: BTreeSet<String> = packages
    .iter()
    .filter(|p| p.name == dependency.name && is_from_registry(p, dependency.registry.as_deref()))
    .filter(|p| originally_locked.contains(p.version.as_str()))
    .filter(|p| Version::parse(&p.version).is_ok_and(|v| v < *to))
    .map(|p| match (&dependency.registry, &p.source) {
      (Some(_), Some(source)) => format!("{source}#{}@{}", p.name, p.version),
      _ => format!("{}@{}", p.name, p.version),
    })
    .collect();
  specs.into_iter().collect()
}

/// Whether `package` came from `registry`, an index URL as resolved by discover, `None` being crates.io.
fn is_from_registry(package: &LockPackage, registry: Option<&str>) -> bool {
  match registry {
    None => package.is_crates_io(),
    Some(registry) => {
      package.is_alternate_registry()
        && package.source.as_deref().and_then(|s| s.strip_prefix("sparse+")).map(|s| s.trim_end_matches('/'))
          == Some(registry.trim_end_matches('/'))
    }
  }
}

/// Runs `cargo` with `args` in a container.
async fn cargo(args: &[&str]) -> Result<(), String> {
  let mut command = vec!["sh".to_string(), "/container-init/cargo.sh".to_string()];
  command.extend(args.iter().map(ToString::to_string));
  host_api::run(RunRequest {
    image: config::image().to_string(),
    entrypoint: None,
    command,
    env: vec![],
    working_dir: None,
  })
  .await?;
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::Occurrence;

  fn occurrence(lock_path: Option<&str>, locked_version: Option<&str>) -> Occurrence {
    Occurrence {
      manifest_path: "Cargo.toml".to_string(),
      lock_path: lock_path.map(ToString::to_string),
      current_version: "1".to_string(),
      locked_version: locked_version.map(ToString::to_string),
    }
  }

  fn dependency(registry: Option<&str>, occurrences: Vec<Occurrence>) -> Dependency {
    Dependency {
      name: "serde".to_string(),
      registry: registry.map(ToString::to_string),
      occurrences,
      current_version: "1.0.0".to_string(),
    }
  }

  fn package(name: &str, version: &str, source: &str) -> LockPackage {
    LockPackage { name: name.to_string(), version: version.to_string(), source: Some(source.to_string()) }
  }

  const CRATES_IO: &str = "registry+https://github.com/rust-lang/crates.io-index";

  #[test]
  fn manifest_paths_sit_beside_the_lockfile() {
    assert_eq!(container_manifest_path("Cargo.lock"), "/workspace/Cargo.toml");
    assert_eq!(container_manifest_path("tools/x/Cargo.lock"), "/workspace/tools/x/Cargo.toml");
  }

  #[test]
  fn lock_paths_are_distinct_and_skip_unlocked_occurrences() {
    let dependency = dependency(
      None,
      vec![occurrence(Some("Cargo.lock"), None), occurrence(None, None), occurrence(Some("Cargo.lock"), None)],
    );
    assert_eq!(lock_paths(&dependency).into_iter().collect::<Vec<_>>(), ["Cargo.lock"]);
  }

  #[test]
  fn only_originally_locked_older_versions_are_updated_precisely() {
    let dependency = dependency(
      None,
      vec![
        occurrence(Some("Cargo.lock"), Some("1.0.1")),
        occurrence(Some("Cargo.lock"), Some("1.0.1")),
        occurrence(Some("other/Cargo.lock"), Some("1.0.2")),
      ],
    );
    let packages = [
      package("serde", "1.0.1", CRATES_IO),
      package("serde", "0.9.0", CRATES_IO),
      package("serde", "1.0.2", CRATES_IO),
      package("serde", "1.0.1", "sparse+https://example.com/index/"),
      package("serde_json", "1.0.1", CRATES_IO),
    ];
    let to = Version::new(1, 0, 5);
    assert_eq!(precise_specs(&packages, &dependency, "Cargo.lock", &to), ["serde@1.0.1"]);
    assert_eq!(precise_specs(&packages, &dependency, "other/Cargo.lock", &to), ["serde@1.0.2"]);
    assert!(precise_specs(&packages, &dependency, "Cargo.lock", &Version::new(1, 0, 1)).is_empty());
  }

  #[test]
  fn alternate_registries_use_fully_qualified_specs() {
    let dependency =
      dependency(Some("https://example.com/index/"), vec![occurrence(Some("Cargo.lock"), Some("1.0.1"))]);
    let packages = [
      package("serde", "1.0.1", CRATES_IO),
      package("serde", "1.0.1", "sparse+https://other.com/index/"),
      package("serde", "1.0.1", "sparse+https://example.com/index/"),
    ];
    assert_eq!(
      precise_specs(&packages, &dependency, "Cargo.lock", &Version::new(1, 1, 0)),
      ["sparse+https://example.com/index/#serde@1.0.1"]
    );
  }
}
