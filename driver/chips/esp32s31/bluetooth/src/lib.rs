//! ESP32-S31 Bluetooth controller hardware boundary.
//!
//! This crate is intentionally below HCI and the Bluetooth Link Layer. The
//! implemented slices establish lossless physical ownership, the platform
//! clock/reset prerequisite and the first finite scheduler-table transaction
//! of controller init. Common PHY, BT-baseband and the complete 50-operation
//! BTDM HAL-init body are implemented as later enable-stage components. The
//! two Controller interrupt sources, level/residency policies, baseline masks,
//! snapshot modes and shared-register staging are also represented, but are
//! deliberately not connected across the missing event/list, LP, BLE,
//! live-route lifecycle and interrupt-classification prerequisites. No
//! current finite state claims that the complete controller lifecycle, HCI
//! transport, task or live interrupt epoch has completed.

#![no_std]
#![deny(unsafe_code)]

#[cfg(test)]
extern crate std;

#[cfg(any(target_arch = "riscv32", test))]
mod baseband;
mod clock;
#[cfg(any(target_arch = "riscv32", test))]
mod common_phy_state;
mod interrupt;
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
pub use interrupt::{
    BluetoothCpuInterruptRoutePolicy, BluetoothCpuInterruptSource,
    BluetoothInterruptHandlerResidency,
};
#[cfg(target_arch = "riscv32")]
pub use phy::{
    BluetoothPhyInitializationConfig, BluetoothPhyInitializationError,
    BluetoothPhyInitializationFailure, BluetoothPhyPlatform,
};
pub use resources::BluetoothPhysicalResources;
#[cfg(target_arch = "riscv32")]
pub use scheduler::BluetoothSchedulerTableLowBitsCleared;
