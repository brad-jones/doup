use clap::Subcommand;

mod init;
pub use init::init;

mod add;
pub use add::add;

mod remove;
pub use remove::remove;

mod lock;
pub use lock::lock;

mod update;
pub use update::update;

mod self_update;
pub use self_update::self_update;

#[derive(Subcommand)]
pub enum Commands {
  Init(init::InitArgs),
  Add(add::AddArgs),
  Remove(remove::RemoveArgs),
  Lock(lock::LockArgs),
  Update(update::UpdateArgs),
  SelfUpdate(self_update::SelfUpdateArgs),
}
