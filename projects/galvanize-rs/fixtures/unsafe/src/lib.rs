//! Classes shared by galvanize-rs's tests and benchmarks, using `UnsafeRuntimeStrategy`.
//!
//! The declarations are generated from `../shared/lib.rs` by `build.rs`. This crate's integration tests
//! extend them from another crate, and only run under Miri (`task test:miri`).
#![forbid(unsafe_code)]

include!(concat!(env!("OUT_DIR"), "/lib.rs"));
