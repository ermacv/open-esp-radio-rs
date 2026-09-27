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
//!
//! [`RadioSystem::run_coex_schedule`] is the coexistence schedule's phase
//! timer. Protocols publish their coexistence status and program their
//! requests through [`RadioGuard`], and await the phases the schedule
//! notifies them of through [`RadioSystem::wifi_coex_phase`] and
//! [`RadioSystem::bluetooth_coex_phase`].

#[cfg(target_arch = "riscv32")]
mod system;

#[cfg(target_arch = "riscv32")]
pub use system::{
    CoexPreemptionEnd, CoexWifiChannel, Ieee802154Asleep, Ieee802154JoinError, Ieee802154Left,
    Ieee802154WakeError, Ieee802154WakeFailure, RadioGuard, RadioPhyError, RadioPhyPrepared,
    RadioResources, RadioSystem, WifiAsleep, WifiWakeError, WifiWakeFailure,
};
