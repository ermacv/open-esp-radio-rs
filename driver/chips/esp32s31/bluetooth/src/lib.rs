//! ESP32-S31 Bluetooth controller hardware boundary.
//!
//! This crate is intentionally below HCI and the Bluetooth Link Layer. The
//! implemented slices establish lossless physical ownership, the platform
//! clock/reset prerequisite and the first finite scheduler-table transaction
//! of controller init. That path stops before software event/list, LP,
//! BLE-stack and HCI initialization. Common PHY and BT-baseband transitions are
//! implemented as later enable-stage components but are deliberately not
//! reachable from the incomplete init frontier. No current finite state claims
//! that the complete controller lifecycle, HCI transport, task or CPU
//! interrupt routing has completed.

#![no_std]
#![deny(unsafe_code)]

#[cfg(test)]
extern crate std;

#[cfg(any(target_arch = "riscv32", test))]
mod baseband;
mod clock;
#[cfg(any(target_arch = "riscv32", test))]
mod common_phy_state;
#[cfg(target_arch = "riscv32")]
mod phy;
mod resources;
#[cfg(any(target_arch = "riscv32", test))]
mod scheduler;
#[cfg(feature = "validation-probes")]
#[doc(hidden)]
pub mod validation;

#[cfg(target_arch = "riscv32")]
pub use baseband::{BluetoothBasebandInitializationReport, BluetoothBasebandInitialized};
pub use clock::{
    BluetoothClockCheckpoint, BluetoothClockControl, BluetoothClockEnableFailure,
    BluetoothClockError, BluetoothClockState, BluetoothClockedResources,
};
#[cfg(target_arch = "riscv32")]
pub use common_phy_state::{BluetoothPhyInitializationReport, BluetoothPhyInitialized};
#[cfg(target_arch = "riscv32")]
pub use phy::{
    BluetoothPhyInitializationConfig, BluetoothPhyInitializationError,
    BluetoothPhyInitializationFailure, BluetoothPhyPlatform,
};
pub use resources::BluetoothPhysicalResources;
#[cfg(target_arch = "riscv32")]
pub use scheduler::BluetoothSchedulerTableLowBitsCleared;
