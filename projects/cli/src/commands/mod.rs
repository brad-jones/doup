use clap::Subcommand;

mod init;
pub use init::init;

mod update;
pub use update::update;

mod self_update;
pub use self_update::self_update;

#[derive(Subcommand)]
pub enum Commands {
  Init(init::InitArgs),
  Update(update::UpdateArgs),
  SelfUpdate(self_update::SelfUpdateArgs),
}
