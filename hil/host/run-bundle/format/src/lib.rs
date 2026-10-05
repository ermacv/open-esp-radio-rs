//! The HIL run bundle format: its documents and its one reader.
//!
//! - [`run`]: the bundle's typed documents, their structural validation, the
//!   integrity seal and the scenario seals;
//! - [`read`]: [`read::RunBundle`], the typed reader every consumer uses,
//!   the qualification evaluator included;
//! - [`build`]: the build provenance of a run's images;
//! - [`experiment`], [`lab`]: the A/B experiment and the laboratory cell a
//!   run records;
//! - [`observer`]: the observer's build identity a run records;
//! - [`pending`]: a checkout's clean runs whose evidence is not recorded.
//!
//! The writer, the run store and the offline verification are
//! `oer-hil-run-bundle`'s.
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

pub mod build;
pub mod experiment;
pub mod lab;
pub mod observer;
pub mod pending;
pub mod read;
pub mod run;

pub use read::RunBundle;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
