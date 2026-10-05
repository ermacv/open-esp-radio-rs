//! Repository policies, separate from process and Cargo graph mechanics.

pub mod architecture;
pub mod changed;
pub mod docs;
pub mod feature_sets;
pub mod isa_conformance;
pub mod metadata;
pub mod network;
pub mod phy;
pub mod standalone;

pub(crate) mod common;

pub use metadata::run as metadata;
pub use network::run as network;
