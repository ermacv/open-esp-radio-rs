#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]
#![deny(missing_docs)]

//! ESP32-S31 IEEE 802.15.4 as a client of the shared radio arbiter.
//!
//! [`start`] follows ESP-IDF's `esp_ieee802154_enable` over the concurrent
//! radio split: common radio power and the IEEE 802.15.4 module clocks, the
//! shared PHY domain (registered, woken or joined), the shared BTBB baseband
//! and transmit-on delay, the MAC reset and masked foundation, then the
//! interrupt owner, the MAC engine's `mac_init` inside the runtime, and
//! finally the CPU route of modem source 132. [`Ieee802154System::stop`]
//! reverses every step. A step that fails before touching shared state rolls
//! the earlier steps back and returns the partition; a started PHY or clock
//! transaction that fails is fail-stop.
//!
//! The runtime is a process singleton: modem source 132 has one handler and
//! the MAC has one set of owners. Commands and events go through
//! [`Ieee802154System::runtime`].

mod maintenance;
#[cfg(target_arch = "riscv32")]
mod system;

pub use maintenance::{
    BUSY_RETRY_MICROS, Ieee802154PhyMaintenance, MAINTENANCE_PERIOD_MICROS, next_attempt_micros,
};

// Inputs of `start` and `Ieee802154Parked::new`, and the shared radio the
// client joins, so an application can construct it through this crate (or
// the `oer` facade) alone.
#[cfg(target_arch = "riscv32")]
pub use oer_esp32s31_hal::{ieee802154::ll::Ieee802154MacOwners, root::RadioHardware};
#[cfg(target_arch = "riscv32")]
pub use oer_esp32s31_radio_esp_hal::{EspHalRadioClocks, EspHalRadioPlatform};
pub use oer_espressif_ieee802154_engine::pib::Ieee802154PibDefaults;

/// The shared ESP32-S31 radio IEEE 802.15.4 joins: the arbiter with the
/// esp-hal platform and clock sources. The application creates it once and
/// runs its periodic PHY tracking
/// ([`RadioSystem::run_tracking`](oer_esp32s31_radio_runtime::RadioSystem::run_tracking)).
#[cfg(target_arch = "riscv32")]
pub type SharedRadio =
    oer_esp32s31_radio_runtime::RadioSystem<EspHalRadioPlatform, EspHalRadioClocks>;

#[cfg(target_arch = "riscv32")]
pub use system::{
    IEEE802154_EVENT_CAPACITY, Ieee802154FailStop, Ieee802154MaintenanceError, Ieee802154Parked,
    Ieee802154StartError, Ieee802154StartFailure, Ieee802154StopError, Ieee802154StopFailure,
    Ieee802154System, Ieee802154SystemRuntime, start,
};
