//! The Wi-Fi fixtures of HIL runs, grouped by provider, and the fixture
//! provider the runner composes ([`provider::PROVIDER`]).
//!
//! `local` (the laptop radio, hostapd and wpa_supplicant, its air monitor)
//! and `openwrt` (the SSH-managed router: AP, client, captures and monitors)
//! own their provider's resources and evidence; `controlled_ap` selects
//! between them and `prepared` owns a scenario's AP lifetime. `host_network`
//! proves the host's route to a station target, `probe_load` drives the
//! probe-request load, and `hostapd` builds the pinned hostapd the Linux
//! fixture installs. The radio evidence they produce is analyzed by
//! `oer-hil-family-ieee80211-evidence`.
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

mod capture_process;
mod channel;
pub mod controlled_ap;
pub mod host_network;
pub mod hostapd;
pub mod local;
pub mod openwrt;
pub mod prepared;
pub mod probe_load;
pub mod provider;
pub mod station_fixture;
#[cfg(all(test, unix))]
mod test_support;

pub use oer_hil_lab::Error;
pub(crate) use oer_hil_workload::Result;
