//! Chip-neutral engine of the typed vendor-comparison scenarios.
//!
//! A chip's scenario crate installs its [`chip::Chip`] and builds its
//! scenarios on these modules: authenticated artifacts, Blobray sessions,
//! the comparison harness, coverage, observation and state analysis, and
//! the evidence shards. Chip addresses, pins and reviewed decisions stay in
//! the chip's crate.
pub mod artifacts;
pub mod chip;
pub mod coverage;
pub mod evidence;
pub mod harness;
pub mod observation;
pub mod phy;
pub mod session;
pub mod setup_cache;
pub mod state;

pub use chip::{Chip, chip, install};
