//! Reads and writes `Cargo.lock` files (format versions 3 & 4) exactly as cargo would.
//!
//! Everything before the first `[[package]]` (the `@generated` comment & `version`) and anything
//! after the packages (eg: `[[patch.unused]]`) is kept verbatim, only the packages are rewritten.
//! This module only uses plain `std` / `serde` types so that it can be unit tested natively.

use semver::Version;
use serde::Deserialize;
use std::{collections::HashMap, fmt};

/// Uniquely identifies a package within a lockfile.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PackageId {
  pub name: String,
  pub version: Version,
  /// Where the package comes from, eg: `registry+https://github.com/rust-lang/crates.io-index`.
  /// `None` for workspace members & path dependencies.
  pub source: Option<String>,
}

impl fmt::Display for PackageId {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "{}@{}", self.name, self.version)
  }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Package {
  pub id: PackageId,
  pub checksum: Option<String>,
  pub dependencies: Vec<PackageId>,
  /// A `[replace]` target, kept as written.
  pub replace: Option<String>,
}

#[derive(Debug)]
pub struct Lockfile {
  header: String,
  pub packages: Vec<Package>,
  trailer: String,
}

#[derive(Deserialize)]
struct RawLock {
  version: Option<u32>,
  #[serde(default)]
  package: Vec<RawPackage>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPackage {
  name: String,
  version: String,
  source: Option<String>,
  checksum: Option<String>,
  #[serde(default)]
  dependencies: Vec<String>,
  replace: Option<String>,
}

impl Lockfile {
  pub fn parse(text: &str) -> Result<Self, String> {
    let raw: RawLock = toml::from_str(text).map_err(|err| format!("invalid Cargo.lock: {err}"))?;
    match raw.version {
      Some(3 | 4) => {}
      Some(v) => return Err(format!("unsupported Cargo.lock version {v}, only versions 3 & 4 are supported")),
      None => return Err("unsupported Cargo.lock version 1/2, only versions 3 & 4 are supported".to_string()),
    }

    let (header, trailer) = split_sections(text)?;

    let ids = raw
      .package
      .iter()
      .map(|p| {
        let version =
          Version::parse(&p.version).map_err(|err| format!("invalid version `{}` of {}: {err}", p.version, p.name))?;
        Ok(PackageId { name: p.name.clone(), version, source: p.source.clone() })
      })
      .collect::<Result<Vec<_>, String>>()?;

    let packages = raw
      .package
      .into_iter()
      .zip(&ids)
      .map(|(p, id)| {
        let dependencies = p.dependencies.iter().map(|d| resolve_reference(d, &ids)).collect::<Result<_, _>>()?;
        Ok(Package { id: id.clone(), checksum: p.checksum, dependencies, replace: p.replace })
      })
      .collect::<Result<_, String>>()?;

    Ok(Self { header: header.to_string(), packages, trailer: trailer.to_string() })
  }

  /// A copy of this lockfile holding `packages` instead.
  pub fn with_packages(&self, packages: Vec<Package>) -> Self {
    Self { header: self.header.clone(), packages, trailer: self.trailer.clone() }
  }
}

/// Splits `text` into what comes before the first `[[package]]` and what comes after the last one.
fn split_sections(text: &str) -> Result<(&str, &str), String> {
  let mut offset = 0;
  let (mut first_package, mut trailer) = (None, None);
  for line in text.split_inclusive('\n') {
    let trimmed = line.trim_end();
    if trimmed == "[[package]]" {
      if trailer.is_some() {
        return Err("unsupported Cargo.lock: packages are expected to come before any other table".to_string());
      }
      first_package.get_or_insert(offset);
    } else if trimmed.starts_with('[') && first_package.is_some() && trailer.is_none() {
      trailer = Some(offset);
    }
    offset += line.len();
  }
  let header_end = first_package.unwrap_or(text.len());
  let trailer_start = trailer.unwrap_or(text.len());
  Ok((&text[..header_end], &text[trailer_start..]))
}

/// Resolves a dependency reference, `name`, `name version` or `name version (source)`, to its package.
fn resolve_reference(reference: &str, ids: &[PackageId]) -> Result<PackageId, String> {
  let mut parts = reference.splitn(3, ' ');
  let name = parts.next().unwrap_or_default();
  let version = parts.next();
  let source = parts.next().and_then(|s| s.strip_prefix('(')).and_then(|s| s.strip_suffix(')'));

  let mut matches = ids.iter().filter(|id| {
    id.name == name
      && version.is_none_or(|v| id.version.to_string() == v)
      && source.is_none_or(|s| id.source.as_deref() == Some(s))
  });
  match (matches.next(), matches.next()) {
    (Some(id), None) => Ok(id.clone()),
    (None, _) => Err(format!("invalid Cargo.lock: dependency `{reference}` matches no package")),
    (Some(_), Some(_)) => Err(format!("invalid Cargo.lock: dependency `{reference}` is ambiguous")),
  }
}

impl fmt::Display for Lockfile {
  /// Writes the lockfile with packages sorted & dependency references encoded as cargo does.
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    let mut versions_by_name: HashMap<&str, HashMap<&Version, usize>> = HashMap::new();
    for p in &self.packages {
      *versions_by_name.entry(&p.id.name).or_default().entry(&p.id.version).or_default() += 1;
    }
    let encode = |id: &PackageId| {
      let versions = &versions_by_name[id.name.as_str()];
      match (versions.len(), versions.get(&id.version).copied().unwrap_or(0)) {
        (1, 1) => (id.name.clone(), None, None),
        (_, 1) => (id.name.clone(), Some(id.version.to_string()), None),
        _ => (id.name.clone(), Some(id.version.to_string()), id.source.clone()),
      }
    };

    let mut packages: Vec<&Package> = self.packages.iter().collect();
    packages.sort_by(|a, b| a.id.cmp(&b.id));

    f.write_str(&self.header)?;
    for (i, package) in packages.iter().enumerate() {
      if i > 0 {
        f.write_str("\n")?;
      }
      writeln!(f, "[[package]]")?;
      writeln!(f, "name = {}", quote(&package.id.name))?;
      writeln!(f, "version = {}", quote(&package.id.version.to_string()))?;
      if let Some(source) = &package.id.source {
        writeln!(f, "source = {}", quote(source))?;
      }
      if let Some(checksum) = &package.checksum {
        writeln!(f, "checksum = {}", quote(checksum))?;
      }
      if !package.dependencies.is_empty() {
        let mut dependencies: Vec<_> = package.dependencies.iter().map(encode).collect();
        dependencies.sort();
        dependencies.dedup();
        writeln!(f, "dependencies = [")?;
        for (name, version, source) in dependencies {
          let mut reference = name;
          if let Some(version) = version {
            reference = format!("{reference} {version}");
          }
          if let Some(source) = source {
            reference = format!("{reference} ({source})");
          }
          writeln!(f, " {},", quote(&reference))?;
        }
        writeln!(f, "]")?;
      }
      if let Some(replace) = &package.replace {
        writeln!(f, "replace = {}", quote(replace))?;
      }
    }
    if !self.trailer.is_empty() {
      f.write_str("\n")?;
      f.write_str(&self.trailer)?;
    }
    Ok(())
  }
}

fn quote(value: &str) -> String {
  format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
  use super::*;

  const HEADER: &str =
    "# This file is automatically @generated by Cargo.\n# It is not intended for manual editing.\nversion = 4\n\n";

  #[test]
  fn round_trips_this_repos_lockfile() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../../Cargo.lock");
    let text = std::fs::read_to_string(path).unwrap();
    assert_eq!(Lockfile::parse(&text).unwrap().to_string(), text);
  }

  #[test]
  fn references_are_only_as_specific_as_needed() {
    let text = format!(
      "{HEADER}[[package]]
name = \"app\"
version = \"0.1.0\"
dependencies = [
 \"dup 1.0.0 (registry+https://a)\",
 \"dup 1.0.0 (registry+https://b)\",
 \"rand 0.7.3\",
 \"rand 0.8.5\",
 \"serde\",
]

[[package]]
name = \"dup\"
version = \"1.0.0\"
source = \"registry+https://a\"

[[package]]
name = \"dup\"
version = \"1.0.0\"
source = \"registry+https://b\"

[[package]]
name = \"rand\"
version = \"0.7.3\"
source = \"registry+https://a\"

[[package]]
name = \"rand\"
version = \"0.8.5\"
source = \"registry+https://a\"

[[package]]
name = \"serde\"
version = \"1.0.0\"
source = \"registry+https://a\"
checksum = \"abc\"

[[patch.unused]]
name = \"unused\"
version = \"0.1.0\"
"
    );
    let lock = Lockfile::parse(&text).unwrap();
    assert_eq!(lock.packages[0].dependencies.len(), 5);
    assert_eq!(lock.to_string(), text);
  }

  #[test]
  fn packages_are_written_sorted() {
    let text = format!(
      "{HEADER}[[package]]\nname = \"b\"\nversion = \"0.10.0\"\n\n[[package]]\nname = \"b\"\nversion = \"0.9.0\"\n\n[[package]]\nname = \"a\"\nversion = \"1.0.0\"\n"
    );
    let expected = format!(
      "{HEADER}[[package]]\nname = \"a\"\nversion = \"1.0.0\"\n\n[[package]]\nname = \"b\"\nversion = \"0.9.0\"\n\n[[package]]\nname = \"b\"\nversion = \"0.10.0\"\n"
    );
    assert_eq!(Lockfile::parse(&text).unwrap().to_string(), expected);
  }

  #[test]
  fn old_formats_are_rejected() {
    let err = Lockfile::parse("[[package]]\nname = \"a\"\nversion = \"1.0.0\"\n").unwrap_err();
    assert!(err.contains("version"), "{err}");
  }
}
