//! ESP32-S31 Bluetooth controller hardware boundary.
//!
//! This crate is intentionally below HCI and the Bluetooth Link Layer. The
//! implemented slices establish lossless physical ownership, the platform
//! clock/reset prerequisite, and a full common-PHY transition shared with
//! Wi-Fi. Individually reviewed controller/interrupt transactions remain
//! isolated verification leaves until the intervening BT-baseband typestate
//! exists. No current finite state claims that the complete controller
//! lifecycle, HCI transport, or CPU interrupt routing has completed.

#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

mod clock;
#[cfg(target_arch = "riscv32")]
mod phy;
mod resources;
#[cfg(feature = "validation-probes")]
#[doc(hidden)]
pub mod validation;

pub use clock::{
    BluetoothClockCheckpoint, BluetoothClockControl, BluetoothClockEnableFailure,
    BluetoothClockError, BluetoothClockState, BluetoothClockedResources,
};
#[cfg(target_arch = "riscv32")]
pub use phy::{
    BluetoothPhyInitializationConfig, BluetoothPhyInitializationError,
    BluetoothPhyInitializationFailure, BluetoothPhyInitializationReport, BluetoothPhyInitialized,
    BluetoothPhyPlatform,
};
pub use resources::BluetoothPhysicalResources;
