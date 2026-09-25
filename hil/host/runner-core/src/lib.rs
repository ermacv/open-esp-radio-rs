//! Shared HIL runner core: scenarios, laboratory configuration and locks, the
//! UART session and target protocol, workload context, image build and flash,
//! and sealed run evidence.
//!
//! Radio-family workloads and their fixtures live in the domain packages that
//! depend on this crate; the `open-esp-radio-hil-runner` binary composes them.
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

pub mod archive;
pub mod campaign;
pub mod context;
pub mod device;
pub mod durable;
pub mod error;
pub mod evidence;
pub mod failure;
pub mod fixture;
pub mod image;
pub mod lab;
pub mod output;
mod repository;
pub mod scenario;
pub mod session;
pub mod transport;
pub mod workload;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
pub use output::emit_json;
pub use repository::repository_root;
