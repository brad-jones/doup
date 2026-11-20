//! Generates this crate's classes and tests from the shared ones in `../shared`, which are written for
//! `SafeRuntimeStrategy`. Tests exercising `unsafe` code only run under Miri, which checks them for
//! undefined behaviour, so every `#[test]` is ignored otherwise.

use std::path::{Path, PathBuf};
use std::{env, fs, io};

const SHARED: &[&str] = &["lib.rs", "scenarios.rs", "async_scenarios.rs"];
const MIRI_ONLY: &str = "#[test]\n#[cfg_attr(not(miri), ignore = \"runs under Miri, see `task test:miri`\")]";

fn main() -> io::Result<()> {
  let out = PathBuf::from(env::var_os("OUT_DIR").ok_or_else(|| io::Error::other("OUT_DIR isn't set"))?);
  for name in SHARED {
    let source = Path::new("../shared").join(name);
    println!("cargo::rerun-if-changed={}", source.display());
    let text = fs::read_to_string(&source)?
      .replace("SafeRuntimeStrategy", "UnsafeRuntimeStrategy")
      .replace("#[test]", MIRI_ONLY);
    fs::write(out.join(name), text)?;
  }
  Ok(())
}
