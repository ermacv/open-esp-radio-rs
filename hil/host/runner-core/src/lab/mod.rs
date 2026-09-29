pub mod config;
mod error;
pub use error::Error;
pub mod lock;
pub mod provenance;

/// The root-owned laptop radio helper installed by
/// `cargo hil fixture install --provider linux-net`.
pub const NETWORK_HELPER: &str = "/usr/local/sbin/open-radio-net";
