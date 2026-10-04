#![no_std]
#![forbid(unsafe_code)]

//! Executor- and chip-independent Wi-Fi station MLME and policy.
//!
//! Protocol crates own scan records, IEEE 802.11 framing and WPA state.
//! Chip/runtime adapters own concrete hardware and timer operations. This crate
//! owns Authentication/Association state, the candidate-scan and lifecycle
//! values and the outer attempt, reconnect and backoff policy, and declares
//! the asynchronous ports that preserve one caller-defined resource owner
//! across every asynchronous edge. It never waits: the drivers that await
//! those ports live in `oer-ieee80211-sta-service`.

#[cfg(test)]
extern crate std;

pub mod attempt;
pub mod block_ack;
pub mod ftm;
pub mod join;
pub mod link_monitor;
pub mod modem_sleep;
pub mod pmksa;
pub mod rate_control;
pub mod request;
pub mod sa_query;
pub mod scan;
pub mod station;
pub mod time;
pub mod twt;

pub mod association;
