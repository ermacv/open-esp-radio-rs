//! Shared HIL runner core: laboratory configuration and locks, the UART
//! session and target protocol, workload context and measurements.
//!
//! Radio-family workloads and their fixtures live in the domain packages that
//! depend on this crate; the `oer-hil-runner` binary composes them.
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

pub mod context;
pub mod failure;
pub mod fixture;
pub mod lab;
pub mod output;
pub mod post_mortem;
pub mod profile;
pub mod recovery;
mod repository;
pub mod usb_events;
pub mod workload;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
pub use output::emit_json;
pub use repository::repository_root;
