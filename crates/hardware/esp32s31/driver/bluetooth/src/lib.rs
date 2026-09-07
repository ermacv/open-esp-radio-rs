//! ESP32-S31 Bluetooth hardware backend and executor-neutral radio sessions.
//!
//! PAC/HAL owns MMIO, and the separate memory crate owns controller-SRAM
//! layouts and CPU/hardware ownership. This crate joins those contracts to
//! portable LL policy, controller time, scheduler admission and publication.
//! Its HCI composition preserves command order while radio events progress;
//! the Embassy adapter owns executor waits and task storage.
//!
//! DTM, advertising and scanning have event lifecycles. Peripheral connection
//! has a causal first-event path, completion/recycle and lower recurrence
//! operations, but the active connection loop and reliable ACL dataplane are
//! not yet integrated. Initialization and scheduler RUN are not RF evidence.
//!
//! Shared single-item completion and timed preparation engines implement the
//! common hardware protocol; RX/recycle and packet policy remain role-specific.
//! The public lifecycle begins with one [`resources::BluetoothStopped`] aggregate retaining
//! the platform lease and neutral radio root. Complete powered teardown and
//! long-running PHY maintenance remain separate requirements.
//!
//! See the chip `FEATURES.md` for implemented scopes, unsupported operations
//! and the distinction between source coverage and hardware qualification.

#![no_std]
#![deny(unsafe_code)]

#[cfg(test)]
extern crate std;

#[cfg(any(target_arch = "riscv32", test))]
pub mod baseband;
pub mod ble_phy;
pub mod clock;
#[cfg(target_arch = "riscv32")]
pub mod common_phy_state;
pub mod controller;
pub mod interrupt;
pub mod le;
#[cfg(any(target_arch = "riscv32", test))]
pub mod low_power;
pub mod modem_lp_timer_queue;
#[cfg(target_arch = "riscv32")]
pub mod phy;
pub mod resources;
pub mod runtime_resources;
pub mod scheduler;
#[cfg(feature = "validation-probes")]
#[doc(hidden)]
pub mod validation;

#[cfg(target_arch = "riscv32")]
pub(crate) use ble_phy::AlwaysAwakeTimingReady;

#[cfg(any(target_arch = "riscv32", test))]
pub(crate) use controller::time::{ControllerSchedulerEpoch, ControllerTimeSample};

#[cfg(target_arch = "riscv32")]
pub(crate) use le::advertising::legacy::LegacyAdvertisingCancelledRestoreOutcome;

#[cfg(any(target_arch = "riscv32", test))]
pub(crate) use le::advertising::legacy::timing::{
    LegacyAdvertisingEventWindow, LegacyAdvertisingRecurringTimingObservation,
};

pub(crate) use le::dtm::event::timing::{
    DtmRxInitialEventWindow, DtmRxRecurringEventWindow, DtmTxEventWindow,
};
#[cfg(test)]
pub(crate) use le::dtm::session::DtmSessionStopping;
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) use le::dtm::{
    link_state::DtmLinkStateReset,
    scheduler::reservation::{DtmSchedulerReservation, DtmSchedulerSequenceAuthorizationFailure},
};

pub(crate) use scheduler::time::SchedulerInstant;

pub mod memory;
