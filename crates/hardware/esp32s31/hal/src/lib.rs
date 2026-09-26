#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

//! ESP32-S31 register transactions and affine radio ownership.
//!
//! [`root`] owns the protocol-neutral radio root and composes the restricted
//! PAC partitions into exclusive protocol routes. [`owner`] retains the Wi-Fi
//! route and grants bounded capabilities.
//! [`phy`], [`ieee80211`], [`bluetooth`] and [`ieee802154`] implement domain
//! operations through those capabilities. [`types`] exposes value contracts
//! without granting PAC access. Protocol policy and executor waits belong to
//! their respective protocol and runtime crates.

#[cfg(test)]
extern crate std;

use core::future::Future;

use oer_esp32s31_pac::{
    MacInterruptRegisters as PacMacInterruptRegisters, MacInterruptSetup as PacMacInterruptSetup,
    MacPowerInterruptRegisters as PacMacPowerInterruptRegisters, RadioPhyRegisters,
};

use root::{RadioHardware, RadioPhyReleaseError};

pub mod bluetooth;

mod clock;
mod route_registers;

pub mod coex;

pub mod power;
pub mod root;
pub mod types;
#[cfg(feature = "validation-probes")]
#[doc(hidden)]
pub mod validation;

pub mod owner;

pub(crate) use owner::{phy_pac, phy_pac_mut, sealed};

pub mod ieee80211;
pub mod ieee802154;
pub mod phy;
