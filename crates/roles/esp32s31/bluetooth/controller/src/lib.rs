//! ESP32-S31 Bluetooth LE Controller and its Link Layer roles.
//!
//! The Controller lifecycle and HCI composition (`controller`) and the DTM,
//! advertising, scanning and peripheral-connection roles (`le`) compose the
//! hardware engine of `oer-esp32s31-bluetooth`: its scheduler primitives,
//! interrupt ports, BLE PHY and resources. This crate holds no `unsafe` and
//! no PAC access; the engine discharges every hardware contract.
//!
//! DTM, advertising and scanning have event lifecycles. Peripheral connection
//! has a causal first-event path, active completion/recycle/recurrence and
//! Host-visible establishment/teardown events. The Host-to-Controller ACL path
//! owns one HCI packet through legacy LL fragmentation, retransmission and
//! completed-packet credit return; Controller-to-Host ACL remains unavailable.
//! Its HCI composition preserves command order while radio events progress;
//! the Embassy adapter owns executor waits and task storage.

#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

pub mod controller;
pub mod le;

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
