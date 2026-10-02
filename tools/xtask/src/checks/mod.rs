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

pub const TARGET: &str = "riscv32imafc-unknown-none-elf";
