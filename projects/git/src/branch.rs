//! Naming of the branches doup pushes its dependency updates to.

use doup_config::repo::DoupRepoPlugin;
use doup_plugin_host::ecosystem_plugin::Dependency;
use gix_validate::reference::name_partial_or_sanitize;
use sha2::{Digest, Sha256};
use std::fmt;

/// Every branch doup creates starts with this prefix, any branch without it is never touched by doup.
pub const DOUP_BRANCH_PREFIX: &str = "doup/";

/// Segments longer than this are truncated, keeping well clear of the 255 byte filename limit
/// most filesystems impose on loose refs.
const MAX_SEGMENT_LEN: usize = 100;

/// Number of bytes of the SHA-256 digest kept, rendered as twice as many hex chars.
const HASH_BYTES: usize = 4;

/// The branch a dependency update is pushed to, rendered via [`fmt::Display`] as:
///
/// `doup/<plugin>/<next|latest>/<dependency>-<hash>`
///
/// - `<hash>` is a short SHA-256 of the dependency's identity: the plugin's [`DoupRepoPlugin::name`]
///   (so moving a wasm module or bumping a package version keeps the branch), the dependency's name
///   and its registry. It alone keeps distinct dependencies on distinct branches.
/// - `<plugin>` & `<dependency>` are just for humans, sanitised with Git's own rules (lossy, which the
///   hash makes safe) and with `/` replaced by `-`.
/// - Every branch is exactly this deep, as Git refuses a branch that is the prefix "directory" of another
///   (eg: `doup/go/next/a/b` vs `doup/go/next/a/b/v2`).
///
/// ```
/// use doup_config::repo::DoupRepoPlugin;
/// use doup_git::UpdateBranch;
/// use doup_plugin_host::ecosystem_plugin::Dependency;
///
/// let plugin = DoupRepoPlugin { plugin: "./plugins/cargo.wasm".into(), with: None };
/// let dependency =
///   Dependency { name: "serde".into(), registry: None, occurrences: vec![], current_version: "1.0.0".into() };
/// assert_eq!(UpdateBranch::new(&plugin, &dependency, true).to_string(), "doup/cargo/latest/serde-4ef397e7");
/// ```
#[derive(Debug, Clone, Copy)]
pub struct UpdateBranch<'a> {
  plugin: &'a DoupRepoPlugin,
  dependency: &'a Dependency,
  latest: bool,
}

impl<'a> UpdateBranch<'a> {
  /// The branch updating `dependency`, as discovered by `plugin`. Set `latest` when updating to the
  /// latest (likely breaking) version rather than the next one, these get a branch of their own.
  pub fn new(plugin: &'a DoupRepoPlugin, dependency: &'a Dependency, latest: bool) -> Self {
    Self { plugin, dependency, latest }
  }
}

impl fmt::Display for UpdateBranch<'_> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    let name = render(self.plugin.name(), &self.dependency.name, self.dependency.registry.as_deref(), self.latest);
    f.write_str(&name)
  }
}

fn render(plugin: &str, dependency: &str, registry: Option<&str>, latest: bool) -> String {
  let kind = if latest { "latest" } else { "next" };
  let hash = identity_hash(plugin, dependency, registry);
  format!("{DOUP_BRANCH_PREFIX}{}/{kind}/{}-{hash}", segment(plugin), segment(dependency))
}

/// Turns `value` into a single, nonempty, valid ref component.
fn segment(value: &str) -> String {
  let sanitise = |value: &str| name_partial_or_sanitize(value.replace('/', "-").as_bytes().into()).to_string();
  // The sanitiser only ever replaces or drops ASCII bytes, so valid UTF-8 in means valid UTF-8 out.
  let segment = sanitise(value);
  if segment.len() <= MAX_SEGMENT_LEN {
    return segment;
  }
  let end = (0..=MAX_SEGMENT_LEN).rev().find(|&i| segment.is_char_boundary(i)).unwrap_or(0);
  // Again, as truncating may leave the segment ending in `.` or `.lock`.
  sanitise(&segment[..end])
}

/// A short, stable hash of a dependency's identity, the parts NUL separated so they can't run into each other.
fn identity_hash(plugin: &str, dependency: &str, registry: Option<&str>) -> String {
  let mut hasher = Sha256::new();
  hasher.update(plugin);
  hasher.update([0]);
  hasher.update(dependency);
  if let Some(registry) = registry {
    hasher.update([0]);
    hasher.update(registry);
  }
  hasher.finalize()[..HASH_BYTES].iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
  use super::*;
  use grit_lib::refs::check_ref_format::RefName;

  fn branch(plugin: &str, dependency: &str, registry: Option<&str>, latest: bool) -> String {
    let name = render(plugin, dependency, registry, latest);
    assert!(RefName::branch(&name).is_ok(), "'{name}' is not a valid branch name");
    name
  }

  fn dependency(name: &str, registry: Option<&str>) -> Dependency {
    Dependency {
      name: name.into(),
      registry: registry.map(Into::into),
      occurrences: vec![],
      current_version: "1.0.0".into(),
    }
  }

  fn plugin(plugin: &str) -> DoupRepoPlugin {
    DoupRepoPlugin { plugin: plugin.into(), with: None }
  }

  #[test]
  fn names_the_branch_after_the_plugin_and_dependency() {
    let plugin = plugin("./target/wasm32-wasip3/release/doup_ecosystem_plugin_cargo.wasm");
    assert_eq!(
      UpdateBranch::new(&plugin, &dependency("anyhow", None), false).to_string(),
      "doup/doup_ecosystem_plugin_cargo/next/anyhow-4906a94f"
    );
    assert_eq!(
      UpdateBranch::new(&plugin, &dependency("anyhow", Some("https://my.registry/index")), true).to_string(),
      "doup/doup_ecosystem_plugin_cargo/latest/anyhow-647e9760"
    );
  }

  #[test]
  fn plugin_versions_and_paths_do_not_change_the_branch() {
    let serde = dependency("serde", None);
    let branch = |p: &str| UpdateBranch::new(&plugin(p), &serde, false).to_string();
    assert_eq!(branch("doup:cargo@1.0.0"), branch("doup:cargo@2.0.0"));
    assert_eq!(branch("./a/cargo.wasm"), branch("/b/cargo.wasm"));
  }

  #[test]
  fn next_and_latest_differ_only_by_kind() {
    let next = branch("cargo", "serde", None, false);
    assert_eq!(next.replacen("/next/", "/latest/", 1), branch("cargo", "serde", None, true));
  }

  #[test]
  fn every_part_of_the_identity_changes_the_hash() {
    let names = [
      branch("cargo", "serde", None, false),
      branch("npm", "serde", None, false),
      branch("cargo", "serde_json", None, false),
      branch("cargo", "serde", Some(""), false),
      branch("cargo", "serde", Some("https://my.registry/index"), false),
    ];
    let hashes: std::collections::HashSet<_> = names.iter().map(|n| n.rsplit('-').next().unwrap()).collect();
    assert_eq!(hashes.len(), names.len(), "{names:?}");
  }

  #[test]
  fn distinguishes_names_that_look_alike() {
    assert_ne!(branch("npm", "a/b", None, false), branch("npm", "a-b", None, false));
    assert_ne!(branch("npm", "a b", None, false), branch("npm", "a~b", None, false));
    assert_ne!(branch("ns:pkg", "a", None, false), branch("ns-pkg", "a", None, false));
    assert_ne!(branch("cargo", "foo_latest", None, false), branch("cargo", "foo", None, true));
  }

  #[test]
  fn never_nests_branches() {
    for name in [branch("go", "github.com/foo/bar", None, false), branch("go", "github.com/foo/bar/v2", None, true)] {
      assert_eq!(name.matches('/').count(), 3, "{name}");
    }
  }

  #[test]
  fn sanitises_every_segment() {
    for value in ["ns:pkg", "..", ".hidden", "trailing.", "x.lock", "a..b", "@", "@{u}", "*", "", "/", "a\\b", "ü?"] {
      branch(value, value, Some(value), true);
    }
  }

  #[test]
  fn truncates_long_segments() {
    let long = "ü".repeat(200);
    let name = branch(&long, &long, None, false);
    let mut segments = name.split('/').skip(1);
    assert!(segments.next().unwrap().len() <= MAX_SEGMENT_LEN, "{name}");
    assert!(segments.nth(1).unwrap().len() <= MAX_SEGMENT_LEN + 1 + HASH_BYTES * 2, "{name}");

    // Truncating right after a `.` must not leave an invalid segment behind.
    let dotted = format!("{}.{}", "a".repeat(MAX_SEGMENT_LEN - 1), "b".repeat(10));
    branch(&dotted, &dotted, None, false);
  }
}
