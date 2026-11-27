use anyhow::{Result, bail};
use clap::Args;
use doup_config::{bot::DoupBotConfigFile, file::ConfigFile};
use doup_core::{ActionOutcome, RepoReport, UpdateReport, Updater};
use std::error::Error;
use std::fmt::Write;

/// Runs the doup update process
#[derive(Args)]
pub struct UpdateArgs {
  /// Does not make any changes or create any PRs, only reports what actions would be taken.
  #[arg(long)]
  dry_run: bool,
}

pub async fn update(args: UpdateArgs) -> Result<()> {
  // Load the bot config. This controls settings that affect the
  // overall operation of doup - eg: which git backend to use.
  let config = DoupBotConfigFile::load(None, None)?.merged;
  let report = Updater::from_config(&config).dry_run(args.dry_run).run().await;
  print!("{}", render(&report));

  let failed = report.failed_repos().count();
  if failed > 0 {
    bail!("{failed} repo(s) failed to update");
  }
  Ok(())
}

/// A summary of `report` for humans: every action taken (or planned) in each repo, why, & any errors.
fn render(report: &UpdateReport) -> String {
  let mut out = String::new();
  for repo in report.repos() {
    render_repo(&mut out, repo);
  }
  out
}

fn render_repo(out: &mut String, repo: &RepoReport) {
  let _ = writeln!(out, "\n{}", repo.origin());
  for err in repo.dependency_errors() {
    let _ = writeln!(out, "  {:<13} {}", "error", chain(err));
  }
  for report in repo.actions() {
    let (status, detail) = match report.outcome() {
      ActionOutcome::Planned => ("planned", None),
      ActionOutcome::Skipped => ("skipped", None),
      ActionOutcome::Done { url: Some(url) } => ("done", Some(url.clone())),
      ActionOutcome::Done { url: None } => ("done", None),
      ActionOutcome::Failed(err) => ("failed", Some(chain(err))),
      ActionOutcome::NotAttempted => ("not attempted", None),
      _ => ("unknown", None),
    };
    let _ = writeln!(out, "  {status:<13} {}", report.action());
    let _ = writeln!(out, "  {:<13} {}", "", report.action().reason());
    if let Some(detail) = detail {
      let _ = writeln!(out, "  {:<13} {detail}", "");
    }
  }
  if let Some(err) = repo.error() {
    let _ = writeln!(out, "  {:<13} {}", "error", chain(err));
  } else if repo.actions().is_empty() && repo.dependency_errors().is_empty() {
    let _ = writeln!(out, "  nothing to do");
  }
}

/// `err` and its sources, joined like anyhow's `{:#}`.
fn chain(err: &dyn Error) -> String {
  let mut out = err.to_string();
  let mut source = err.source();
  while let Some(err) = source {
    let _ = write!(out, ": {err}");
    source = err.source();
  }
  out
}
