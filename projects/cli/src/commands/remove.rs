use anyhow::Result;
use clap::Args;

/// Remove a doup plugin
#[derive(Args)]
pub struct RemoveArgs {}

pub async fn remove(_args: RemoveArgs) -> Result<()> {
  todo!()
}
