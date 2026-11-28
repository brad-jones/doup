use anyhow::Result;
use clap::Args;
use doup_core::lock::{LockStatus, PackageClient};
use std::path::PathBuf;

/// Locks the package plugins of the repo's doup config into doup.lock, which should be committed
///
/// Plugins already locked keep their version, new ones are resolved to the highest published version
/// matching their requirement & entries no plugin uses anymore are removed. `doup update` only ever
/// runs the exact plugin versions in doup.lock.
#[derive(Args)]
pub struct LockArgs {
  /// Look for the doup config in this dir (& its parents) instead of the current dir.
  #[arg(long, value_name = "DIR")]
  dir: Option<PathBuf>,
}

pub async fn lock(args: LockArgs) -> Result<()> {
  let report = doup_core::lock::lock(args.dir.as_deref(), &PackageClient::new()).await?;
  for plugin in report.plugins() {
    let status = match plugin.status() {
      LockStatus::Added => "added",
      _ => "locked",
    };
    println!("{status:<8} {} = {}", plugin.plugin(), plugin.version());
  }
  for removed in report.removed() {
    println!("{:<8} {removed}", "removed");
  }
  let path = report.lock_file().display();
  match (report.written(), report.plugins().is_empty()) {
    (true, _) => println!("wrote {path}"),
    (false, true) => println!("no package plugins to lock"),
    (false, false) => println!("{path} is up to date"),
  }
  Ok(())
}
