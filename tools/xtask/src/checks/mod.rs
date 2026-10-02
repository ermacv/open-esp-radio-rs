//! Repository policies, separate from process and Cargo graph mechanics.

pub mod architecture;
pub mod changed;
pub mod docs;
pub mod feature_sets;
pub mod firmware;
pub mod images;
pub mod isa_conformance;
pub mod metadata;
pub mod network;
pub mod phy;
pub mod standalone;
pub mod tidy;
pub mod vendor;

mod artifacts;
pub(crate) mod common;

pub use metadata::run as metadata;
pub use network::run as network;

/// The chip whose production graph the architecture, network, PHY and
/// image checks audit.
pub const CHIP: &str = "esp32s31";

/// [`CHIP`]'s Rust target, as its `platform/<chip>/chip.toml` declares it.
pub fn target(root: &std::path::Path) -> crate::Result<String> {
    Ok(oer_chip_profile::Profile::load(root, CHIP)?.rust_target)
}
