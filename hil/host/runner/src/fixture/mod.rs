//! Controlled host and peer capabilities, grouped by provider.
//!
//! `local` and `openwrt` own their provider's AP, client, monitors and
//! evidence. The remaining modules own the scenario-level lifetime, shared
//! capture primitives and the Bluetooth and probe-load fixtures.
pub(crate) mod bluetooth;
mod capture_process;
mod channel;
pub(crate) mod cleanup;
pub(crate) mod controlled_ap;
pub(crate) mod install;
pub(crate) mod local;
pub(crate) mod openwrt;
pub(crate) mod preflight;
pub(crate) mod prepared;
pub(crate) mod probe_load;
pub(crate) mod software;
pub(crate) mod station_fixture;
#[cfg(all(test, unix))]
mod test_support;

pub(crate) use crate::lab::Error;
