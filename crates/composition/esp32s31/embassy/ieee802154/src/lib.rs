#![no_std]
#![forbid(unsafe_code)]
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

#[cfg(target_arch = "riscv32")]
mod system;

#[cfg(target_arch = "riscv32")]
pub use system::{
    IEEE802154_EVENT_CAPACITY, Ieee802154FailStop, Ieee802154MaintenanceError, Ieee802154Parked,
    Ieee802154PhyMaintenance, Ieee802154StartError, Ieee802154StartFailure, Ieee802154StopError,
    Ieee802154StopFailure, Ieee802154System, Ieee802154SystemRuntime, start,
};
