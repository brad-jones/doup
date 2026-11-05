use anyhow::Result;
use clap::Args;

/// Add a new doup plugin
#[derive(Args)]
pub struct AddArgs {}

pub async fn add(_args: AddArgs) -> Result<()> {
  todo!()
}
