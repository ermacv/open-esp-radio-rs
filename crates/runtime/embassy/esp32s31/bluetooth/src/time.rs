//! Embassy time bindings for the hardware ports used by this adapter.
//!
//! PHY time is shared hardware infrastructure. This binding retains the
//! microsecond timebase and failure policy selected by Bluetooth composition.

#[cfg(any(test, target_arch = "riscv32"))]
pub mod phy;
