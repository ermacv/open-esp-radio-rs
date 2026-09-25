//! ESP32-S31 Bluetooth hardware engine.
//!
//! PAC/HAL owns MMIO, and the separate memory crate owns controller-SRAM
//! layouts and CPU/hardware ownership. This crate joins those contracts into
//! the radio engine: clocks, controller HAL and time, BLE PHY, interrupts,
//! the modem low-power timer, and the hardware scheduler with its timeline,
//! finished lists and single-item primitives. It knows no Link Layer role.
//!
//! The LE Controller and its roles (DTM, advertising, scanning, peripheral
//! connection) compose these primitives in `oer-esp32s31-bluetooth-controller`.
//! Shared single-item completion and timed preparation engines implement the
//! common hardware protocol; RX/recycle and packet policy remain role-specific.
//! The public lifecycle begins with one [`resources::BluetoothStopped`] aggregate retaining
//! the platform lease and neutral radio root. Initialization and scheduler RUN
//! are not RF evidence.
//!
//! See the chip `FEATURES.md` for implemented scopes, unsupported operations
//! and the distinction between source coverage and hardware qualification.

#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]
// `test-support` compiles the chip engine on the host for dependent crates'
// tests. Items reached only from ESP32-S31 paths are unused in that build;
// chip builds and this crate's own tests still report dead code.
#![cfg_attr(
    all(feature = "test-support", not(test), not(target_arch = "riscv32")),
    allow(dead_code)
)]

#[cfg(test)]
extern crate std;

#[cfg(any(target_arch = "riscv32", test, feature = "test-support"))]
pub mod baseband;
pub mod ble_phy;
pub mod clock;
#[cfg(target_arch = "riscv32")]
pub mod common_phy_state;
/// Controller HAL component initialization after clock setup.
#[cfg(any(target_arch = "riscv32", test, feature = "test-support"))]
pub mod controller_hal;
/// Event-driven controller-time latch and scheduler-epoch projection.
pub mod controller_time;
pub mod interrupt;
#[cfg(any(target_arch = "riscv32", test, feature = "test-support"))]
pub mod low_power;
pub mod modem_lp_timer_queue;
/// Controller modem low-power timer task over the published timer owner.
#[cfg(target_arch = "riscv32")]
pub mod modem_timer;
/// Drain-before-retire rule of the modem low-power timer.
#[cfg(any(target_arch = "riscv32", test, feature = "test-support"))]
pub mod modem_timer_retirement;
#[cfg(target_arch = "riscv32")]
pub mod phy;
pub mod resources;
pub mod runtime_resources;
pub mod scheduler;
/// Controller-time preparation shared by timed scheduler admissions.
#[cfg(any(target_arch = "riscv32", test, feature = "test-support"))]
pub mod timed_preparation;
#[cfg(feature = "validation-probes")]
#[doc(hidden)]
pub mod validation;

#[cfg(target_arch = "riscv32")]
pub use ble_phy::AlwaysAwakeTimingReady;

#[cfg(any(target_arch = "riscv32", test, feature = "test-support"))]
pub use controller_time::{ControllerSchedulerEpoch, ControllerTimeSample};

pub use scheduler::time::SchedulerInstant;

pub mod memory;
