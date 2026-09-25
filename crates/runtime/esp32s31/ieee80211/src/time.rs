//! Embassy time bindings for the hardware ports used by this adapter.
//!
//! The direct RX-gain transaction uses the ESP32-S31 ROM `ets_delay_us` loop for
//! its recovered short hardware settles and polls bounded readiness directly,
//! matching the vendor execution shape. Other PHY waits use absolute deadlines:
//! already elapsed waits complete directly and future deadlines retain Embassy
//! timer wake registration. Neither path changes the one-megahertz validation
//! policy owned by Bluetooth composition.

mod delay;

#[cfg(target_arch = "riscv32")]
pub mod phy;
