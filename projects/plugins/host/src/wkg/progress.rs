//! Reports the progress of a download: as a progress bar when the CLI renders them (see `tracing-indicatif`),
//! & always as throttled log events, so that JSON logs & CI output show it too.

use std::time::{Duration, Instant};
use tracing::{Span, debug, info};
use tracing_indicatif::span_ext::IndicatifSpanExt;
use tracing_indicatif::style::ProgressStyle;

/// How often a `debug` event reports progress.
const LOG_EVERY: Duration = Duration::from_secs(1);

/// For downloads whose size is known, ie: those in a lock file.
const BAR_TEMPLATE: &str = "{span_child_prefix}{spinner:.green} downloading {span_fields} {wide_bar:.cyan/blue} {bytes}/{total_bytes} ({bytes_per_sec}, {eta})";

/// For downloads whose size isn't known yet, ie: while locking.
const SPINNER_TEMPLATE: &str =
  "{span_child_prefix}{spinner:.green} downloading {span_fields} {bytes} ({bytes_per_sec})";

/// Creates the span a download runs in, it's what the CLI turns into a progress bar.
macro_rules! download_span {
  ($package:expr, $version:expr) => {
    tracing::info_span!(
      "download",
      package = %$package,
      version = %$version,
      indicatif.pb_show = tracing::field::Empty,
    )
  };
}
pub(super) use download_span;

/// Tracks one download, call it from within the span given to [`Self::start`].
pub(super) struct DownloadProgress {
  span: Span,
  total: Option<u64>,
  done: u64,
  started: Instant,
  logged: Instant,
}

impl DownloadProgress {
  /// Starts tracking a download of `total` bytes, if known, that runs in `span`.
  pub(super) fn start(span: Span, total: Option<u64>) -> Self {
    let template = if total.is_some() { BAR_TEMPLATE } else { SPINNER_TEMPLATE };
    if let Ok(style) = ProgressStyle::with_template(template) {
      span.pb_set_style(&style);
    }
    if let Some(total) = total {
      span.pb_set_length(total);
    }
    info!(parent: &span, size = total, "downloading");
    let now = Instant::now();
    Self { span, total, done: 0, started: now, logged: now }
  }

  /// The bytes downloaded so far.
  pub(super) fn done(&self) -> u64 {
    self.done
  }

  /// Records `bytes` more downloaded.
  pub(super) fn advance(&mut self, bytes: u64) {
    self.done += bytes;
    self.span.pb_inc(bytes);
    if self.logged.elapsed() >= LOG_EVERY {
      self.logged = Instant::now();
      match self.total {
        Some(total) => {
          debug!(parent: &self.span, done = self.done, total, percent = self.done * 100 / total.max(1), "downloading")
        }
        None => debug!(parent: &self.span, done = self.done, "downloading"),
      }
    }
  }

  /// Records that the download completed.
  pub(super) fn finish(self) {
    info!(parent: &self.span, size = self.done, elapsed_ms = self.started.elapsed().as_millis() as u64, "downloaded");
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn templates_are_valid() {
    for template in [BAR_TEMPLATE, SPINNER_TEMPLATE] {
      assert!(ProgressStyle::with_template(template).is_ok(), "{template}");
    }
  }
}
