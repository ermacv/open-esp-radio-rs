#![no_std]
#![forbid(unsafe_code)]

//! Executor- and chip-independent Wi-Fi station MLME and policy.
//!
//! Protocol crates own scan records, IEEE 802.11 framing and WPA state. This
//! crate owns the station: [`machine`], one sans-IO machine for the whole
//! lifecycle, its Authentication/Association state machines ([`join`]), its
//! power-save decisions and the application's station vocabulary
//! ([`station`]). It never waits: a thin service loop over the port, the
//! channel arbiter and time drives it.

#[cfg(test)]
extern crate std;

pub mod ftm;
pub mod join;
pub mod link_monitor;
pub mod machine;
pub mod modem_sleep;
pub mod pmksa;
pub mod power_save;
pub mod request;
pub mod sa_query;
pub mod station;
pub mod time;
pub mod twt;

pub mod association;
