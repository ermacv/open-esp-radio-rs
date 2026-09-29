//! HIL run evidence: the run writer and its seal, build provenance and the
//! content-addressed object store, verification of a recorded run, reports,
//! and the records a run keeps of its experiment and laboratory cell.
//!
//! Evidence depends on no stand, board or image builder code: the image
//! builder produces firmware records for it, and verification asks the
//! builder's recipe through [`verify::FirmwareRecipe`].
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

pub mod build;
pub mod experiment;
pub mod lab;
pub mod reporting;
pub mod run;
pub mod verify;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
