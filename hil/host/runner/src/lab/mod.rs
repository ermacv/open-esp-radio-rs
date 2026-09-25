pub(crate) mod config;
mod error;
pub(crate) use error::Error;
pub(crate) mod lock;
pub(crate) mod provenance;
pub(crate) mod requirements;

/// The root-owned laptop radio helper installed by
/// `cargo hil fixture install --provider linux-net`.
pub(crate) const NETWORK_HELPER: &str = "/usr/local/sbin/open-radio-net";
