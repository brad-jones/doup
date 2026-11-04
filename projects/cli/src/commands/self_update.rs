use anyhow::Result;
use clap::Args;

/// Checks for updates to the doup command it's self
#[derive(Args)]
pub struct SelfUpdateArgs {}

pub async fn self_update(_args: SelfUpdateArgs) -> Result<()> {
  todo!()
}
