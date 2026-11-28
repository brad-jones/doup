//! Sets up `tracing` so the logs of doup (& its crates) are written to stderr.
//!
//! When stderr is a terminal & logs are human readable, spans marked with `indicatif.pb_show` (eg: plugin
//! downloads) are also rendered as progress bars, see `tracing-indicatif`.

use clap::{ArgAction, Args, ValueEnum};
use std::io::IsTerminal;
use tracing_indicatif::IndicatifLayer;
use tracing_indicatif::filter::{IndicatifFilter, hide_indicatif_span_fields};
use tracing_subscriber::fmt::format::DefaultFields;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer, fmt};

/// Overrides the log filter entirely, using `tracing_subscriber`'s `EnvFilter` syntax, eg: `doup_core=trace,info`.
pub const LOG_ENV: &str = "DOUP_LOG";

/// The crates whose logs are shown at the chosen verbosity, every other crate only logs warnings & errors.
const CRATES: &[&str] = &["doup", "doup_core", "doup_config", "doup_git", "doup_plugin_host", "wasm_pkg_client"];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub enum LogFormat {
  /// Human readable lines.
  #[default]
  Human,

  /// One JSON object per line, for log aggregators.
  Json,
}

#[derive(Debug, Args)]
pub struct LogArgs {
  /// Log more: -v for debug & -vv for trace logs. Ignored when DOUP_LOG is set.
  #[arg(short, long, action = ArgAction::Count, global = true, conflicts_with = "quiet")]
  verbose: u8,

  /// Log less: -q for only warnings & -qq for only errors. Ignored when DOUP_LOG is set.
  #[arg(short, long, action = ArgAction::Count, global = true)]
  quiet: u8,

  /// How logs are written to stderr.
  #[arg(long, value_enum, default_value_t, global = true)]
  log_format: LogFormat,
}

impl LogArgs {
  /// The default filter for the chosen verbosity.
  fn filter(&self) -> String {
    let level = match (self.verbose, self.quiet) {
      (0, 0) => "info",
      (1, _) => "debug",
      (_, 0) => "trace",
      (_, 1) => "warn",
      _ => "error",
    };
    let crates = CRATES.iter().map(|name| format!("{name}={level}"));
    std::iter::once("warn".to_string()).chain(crates).collect::<Vec<_>>().join(",")
  }
}

/// Installs the global subscriber, using the filter in [`LOG_ENV`] when set & valid.
pub fn init(args: &LogArgs) {
  let (filter, invalid_env) = match std::env::var(LOG_ENV) {
    Ok(directives) => match EnvFilter::try_new(&directives) {
      Ok(filter) => (filter, None),
      Err(err) => (EnvFilter::new(args.filter()), Some(err)),
    },
    Err(_) => (EnvFilter::new(args.filter()), None),
  };

  let registry = tracing_subscriber::registry().with(filter);
  let installed = match args.log_format {
    LogFormat::Human if std::io::stderr().is_terminal() => {
      let progress = IndicatifLayer::new().with_span_field_formatter(hide_indicatif_span_fields(DefaultFields::new()));
      let logs = fmt::layer().with_target(false).with_ansi(true).with_writer(progress.get_stderr_writer());
      registry.with(logs).with(progress.with_filter(IndicatifFilter::new(false))).try_init()
    }
    LogFormat::Human => {
      registry.with(fmt::layer().with_target(false).with_ansi(false).with_writer(std::io::stderr)).try_init()
    }
    LogFormat::Json => registry
      .with(fmt::layer().json().with_current_span(true).with_span_list(true).with_writer(std::io::stderr))
      .try_init(),
  };
  if installed.is_ok()
    && let Some(err) = invalid_env
  {
    tracing::warn!(error = &err as &dyn std::error::Error, "ignoring the invalid filter in {LOG_ENV}");
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use clap::Parser;

  #[derive(Parser)]
  struct Cli {
    #[command(flatten)]
    log: LogArgs,
  }

  fn filter(args: &[&str]) -> String {
    Cli::parse_from(std::iter::once("doup").chain(args.iter().copied())).log.filter()
  }

  #[test]
  fn verbosity_sets_the_level_of_doups_crates_only() {
    assert_eq!(
      filter(&[]),
      "warn,doup=info,doup_core=info,doup_config=info,doup_git=info,doup_plugin_host=info,wasm_pkg_client=info"
    );
    assert!(filter(&["-v"]).contains("doup_core=debug"));
    assert!(filter(&["-vvv"]).contains("doup_core=trace"));
    assert!(filter(&["-q"]).contains("doup_core=warn"));
    assert!(filter(&["-qqq"]).contains("doup_core=error"));
    assert!(filter(&["-v"]).starts_with("warn,"));
  }

  #[test]
  fn verbose_and_quiet_conflict() {
    assert!(Cli::try_parse_from(["doup", "-v", "-q"]).is_err());
  }
}
