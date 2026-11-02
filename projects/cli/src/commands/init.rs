use anyhow::Result;
use clap::Args;

/// Guided onboarding, helps one write their first doup config file
#[derive(Args)]
pub struct InitArgs {}

pub fn init(args: InitArgs) -> Result<()> {
  todo!()
}
