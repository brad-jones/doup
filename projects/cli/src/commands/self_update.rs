use anyhow::Result;
use clap::Args;

/// Checks for updates to the doup command it's self
#[derive(Args)]
pub struct SelfUpdateArgs {}

pub fn self_update(args: SelfUpdateArgs) -> Result<()> {
  todo!()
}
