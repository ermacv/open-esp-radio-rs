//! The `cargo hil` command line: argument parsing and calls into the stand
//! packages (leases, boards, owners, jobs, the ESP-IDF firmware catalog),
//! the run store's analyses (`oer-hil-analysis`), experiments and the
//! launch of the runner (`oer-hil-experiment`) and pending evidence
//! (`oer-hil-run-bundle`).
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

pub mod board;
pub mod command;
pub mod dashboard;
pub mod evidence;
pub mod experiments;
pub mod firmware_catalog;
pub mod flash;
pub mod jobs;
pub mod stand;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
