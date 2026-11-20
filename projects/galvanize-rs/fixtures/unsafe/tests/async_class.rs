//! The shared async class scenarios, generated from `../../shared/async_scenarios.rs` by `build.rs`.
#![allow(
  clippy::unsafe_removed_from_name,
  reason = "the shared scenarios refer to either fixture crate as `fixtures`"
)]

use galvanize_rs_fixtures_unsafe as fixtures;

include!(concat!(env!("OUT_DIR"), "/async_scenarios.rs"));
