//! Embassy time bindings for the hardware ports used by this adapter.
//!
//! PHY settling waits use absolute deadlines. Already elapsed waits complete
//! directly; future deadlines retain Embassy timer wake registration. These
//! waits impose no separate cooperative execution budget and do not change
//! the one-megahertz validation policy owned by Bluetooth composition.

mod delay;

#[cfg(target_arch = "riscv32")]
pub mod phy;
