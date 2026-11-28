//! The heart of doup: updates the dependencies of every repo the bot is configured for.
//!
//! For each repo, [`Updater`] clones it, loads the plugins its `doup.yaml` asks for, discovers its
//! dependencies & looks up their versions. It then inspects the repo's existing doup branches & pull
//! requests, decides what to do with [`reconcile`](reconcile::reconcile) & does it: opening, updating
//! & closing pull requests, and deleting branches that are no longer needed.
//!
//! ```no_run
//! use doup_config::{bot::DoupBotConfigFile, file::ConfigFile};
//! use doup_core::Updater;
//!
//! # async fn run() -> Result<(), doup_config::ConfigError> {
//! let config = DoupBotConfigFile::load(None, None)?.merged;
//! let report = Updater::from_config(&config).run().await;
//! if !report.is_success() {
//!   eprintln!("{} repo(s) failed to update", report.failed_repos().count());
//! }
//! # Ok(())
//! # }
//! ```
//!
//! Progress is reported through [`tracing`], see the README for the spans & fields used.

pub mod content;
mod error;
pub mod lock;
pub mod plugins;
pub mod reconcile;
mod release_notes;
mod repo;
mod report;
mod updater;

pub use error::{ActionError, DependencyError, RepoError};
pub use reconcile::Action;
pub use report::{ActionOutcome, ActionReport, RepoReport, UpdateReport};
pub use updater::Updater;
