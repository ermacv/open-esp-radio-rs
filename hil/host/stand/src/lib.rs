//! HIL stand operation: the laboratory configuration and the locks a run
//! holds, the pre-run observation of the cell, fixture software leases, the
//! recovery of a board that stopped answering and the post-mortem of a failed
//! repetition. The stand implements the link's `Dut` and `StationNetwork`
//! ports for the board it leased.
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

pub mod config;
mod dut;
mod error;
pub use error::Error;
pub mod lock;
pub mod post_mortem;
pub mod provenance;
pub mod recovery;
mod repository;
pub mod software;
pub mod usb_events;

/// The root-owned laptop radio helper installed by
/// `cargo hil fixture install --provider linux-net`.
pub const NETWORK_HELPER: &str = "/usr/local/sbin/open-radio-net";

pub use repository::repository_root;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
