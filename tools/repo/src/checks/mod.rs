//! Repository policies, separate from process and Cargo graph mechanics.

pub mod architecture;
pub mod docs;
pub mod images;
pub mod metadata;
pub mod network;
pub mod phy;
pub mod standalone;
pub mod vendor;

mod artifacts;
pub(crate) mod common;

pub use metadata::run as metadata;
pub use network::run as network;

pub const TARGET: &str = "riscv32imafc-unknown-none-elf";
