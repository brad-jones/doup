//! Classes shared by galvanize-rs's tests and benchmarks, using `SafeRuntimeStrategy`.
//!
//! The declarations are in `../shared/lib.rs`, which `galvanize-rs-fixtures-unsafe` rewrites for
//! `UnsafeRuntimeStrategy`. This crate's integration tests extend them from another crate.
#![forbid(unsafe_code)]

include!("../../shared/lib.rs");
