//! HIL workload execution: the context a repetition runs in, the
//! classification of its failures as scenario or infrastructure failures,
//! the fixture cleanup evidence it records, profile reports and the
//! workload operations shared by radio families.
//!
//! Radio-family workloads and their fixtures live in the domain packages that
//! depend on this crate; the `oer-hil-runner` binary composes them.
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

pub mod context;
pub mod failure;
pub mod fixture;
pub mod output;
pub mod profile;
pub mod workload;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
pub use output::emit_json;
