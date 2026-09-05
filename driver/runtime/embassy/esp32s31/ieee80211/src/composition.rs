//! Compatibility namespace for the former PHY binding location.
//!
//! New consumers use `time::phy`. Product resource profiles belong
//! to the integration crate.

#[cfg(target_arch = "riscv32")]
pub mod phy;
