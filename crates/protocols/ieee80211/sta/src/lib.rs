#![no_std]
#![forbid(unsafe_code)]
#![deny(
    clippy::let_underscore_must_use,
    clippy::let_underscore_untyped,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::disallowed_macros
)]
#![cfg_attr(
    test,
    allow(
        clippy::let_underscore_must_use,
        clippy::let_underscore_untyped,
        clippy::arithmetic_side_effects,
        clippy::indexing_slicing,
        clippy::panic,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::unreachable,
        clippy::todo,
        clippy::unimplemented,
        clippy::disallowed_macros,
    )
)]

//! Executor- and chip-independent Wi-Fi station MLME and policy.
//!
//! Protocol crates own scan records, IEEE 802.11 framing and WPA state. This
//! crate owns the station: [`machine`], one sans-IO machine for the whole
//! lifecycle, its Authentication/Association state machines ([`join`]), its
//! power-save decisions and the application's station vocabulary
//! ([`station`]). It never waits: a thin service loop over the port, the
//! channel arbiter and time drives it.
//!
//! The SAE PMKSA cache remains a unit-tested prototype until the station's
//! SAE path owns it. Production callers cannot construct an unused cache:
//!
//! ```compile_fail,E0432
//! use oer_ieee80211_sta::pmksa::StaPmksaCache;
//! let cache = StaPmksaCache::new();
//! ```

#[cfg(test)]
extern crate std;

pub mod ftm;
pub mod join;
pub mod link_monitor;
pub mod machine;
pub mod modem_sleep;
#[cfg(test)]
mod pmksa;
mod pool;
pub mod power_save;
pub mod request;
pub mod sa_query;
pub mod station;
pub mod time;
pub mod twt;

pub mod association;
pub mod data;
pub mod epoch;
