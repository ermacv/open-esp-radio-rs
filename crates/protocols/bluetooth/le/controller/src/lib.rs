#![no_std]
#![forbid(unsafe_code)]

//! Pure pieces of a Bluetooth LE Controller: no transport, executor, clock,
//! scheduler or hardware.
//!
//! - [`timing`]: connection-event geometry over the backend's
//!   [`ConnectionAllowances`](oer_bluetooth_radio::ConnectionAllowances)
//!   (first and recurring peripheral events, sleep-clock window widening) and
//!   legacy advertising channel geometry after the vendor item geometry, with
//!   [`EpochExhausted`](timing::EpochExhausted) when a result leaves the radio
//!   epoch;
//! - [`receive`]: authentication of received data channel PDUs before Link
//!   Layer and Host dispatch;
//! - [`coexistence`]: the vendor Controller's dynamic coexistence priority
//!   rules for advertising and connection events;
//! - [`diagnostic`]: the codec of the vendor HCI command `0xFC01` that armed
//!   one received-MIC corruption, and the corruption itself.
//!
//! The Controller core that served HCI commands and ran the Link Layer roles
//! over the radio request port was removed with the Bluetooth LE
//! orchestration (#770, tag `legacy-radio-2026-10-16`).

#[cfg(test)]
extern crate std;

pub mod coexistence;
pub mod diagnostic;
pub mod receive;
pub mod timing;
