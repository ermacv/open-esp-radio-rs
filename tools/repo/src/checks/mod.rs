//! Repository policies, separate from process and Cargo graph mechanics.

pub mod architecture;
pub mod examples;
pub mod metadata;
pub mod network;
pub mod safety;
pub mod source_only;
pub mod standalone;
pub mod vendor;

mod artifacts;
mod common;

pub use metadata::run as metadata;
pub use network::run as network;

pub const TARGET: &str = "riscv32imafc-unknown-none-elf";
