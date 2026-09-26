#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! The shared ESP32-S31 radio on the chip, as ESP-IDF's `esp_phy` component.
//!
//! [`RadioSystem`] owns the radio arbiter with its shared PHY domain, the
//! platform token the PHY target port borrows and the platform clock sources.
//! Protocol compositions are clients: they take the arbiter lease together
//! with those resources through [`RadioSystem::lock`], prepare the shared PHY
//! when they join ([`RadioGuard::prepare_phy`], the first-client half of
//! `esp_phy_enable`) and close RF after the last client left
//! ([`RadioGuard::close_phy_if_idle`], the last-client half of
//! `esp_phy_disable`).
//!
//! [`RadioSystem::run_tracking`] is the vendor periodic `phy_track_pll`
//! timer. It runs one tracking tick every tracking period under the domain's
//! admission policy; the default follows ESP-IDF and tracks with protocols
//! running, protected by the grant-protect brackets of the tracking graph.

#[cfg(target_arch = "riscv32")]
mod system;

#[cfg(target_arch = "riscv32")]
pub use system::{RadioGuard, RadioPhyError, RadioResources, RadioSystem};
