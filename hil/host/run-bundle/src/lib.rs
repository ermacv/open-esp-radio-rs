//! The HIL run bundle: its format, its one writer and its one reader.
//!
//! - [`run`]: the bundle's typed documents (manifest, plan, suite, scenario
//!   and repetition results with their measurements, cleanup and USB
//!   records, integrity and scenario seals), the writer [`run::RunSession`]
//!   that creates, seals and interrupts a run, and the structural
//!   [`run::validation`] both its writer and its readers apply.
//! - [`read`]: [`read::RunBundle`], the typed reader every consumer uses,
//!   the qualification evaluator included.
//! - [`store`]: [`store::RunStore`], the run store every checkout shares, its
//!   sidecars (pins, quarantine, performance baselines, caches) and a
//!   checkout's pending evidence.
//! - [`receipt`]: the receipt a runner process names its runs in.
//! - [`build`]: the build provenance of a run's images and the
//!   content-addressed object store, the one definition of a build's
//!   identity; the image builder fills it.
//! - [`verify`]: the offline verification of a whole bundle, its build
//!   records and its replays against the image builder's recipe.
//! - [`experiment`], [`lab`]: the A/B experiment and the laboratory cell a
//!   run records.
//!
//! The bundle depends on no stand, board or image builder code: the image
//! builder hands it firmware records and implements
//! [`verify::FirmwareRecipe`], and the runner hands [`run::RunSession::finish`]
//! the renderer of its derived views.
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

pub mod build;
pub mod experiment;
pub mod lab;
pub mod read;
pub mod receipt;
pub mod run;
pub mod store;
pub mod verify;

pub use read::RunBundle;
pub use receipt::RunId;
pub use store::RunStore;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
