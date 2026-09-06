//! HIL-owned stack composition; all implementations share the traffic workloads.
#![forbid(unsafe_code)]
pub(super) struct Settings {
    pub ipv4: Option<open_esp_radio_hil_protocol::NetworkIpv4Configuration>,
    pub seed: u64,
    pub rx_checksum: open_esp_radio_hil_protocol::WifiRxChecksumPolicy,
    pub tx_udp_checksum: open_esp_radio_hil_protocol::WifiTxUdpChecksumPolicy,
}

#[cfg(feature = "upstream-network")]
mod upstream;
#[cfg(feature = "upstream-network")]
pub(super) use upstream::*;
#[cfg(not(feature = "upstream-network"))]
mod embassy;
#[cfg(not(feature = "upstream-network"))]
pub(super) use embassy::*;
#[cfg(feature = "upstream-network")]
mod checksum;
#[cfg(feature = "upstream-network")]
mod ipv4;
#[cfg(feature = "upstream-network")]
pub(super) use ipv4::{configure, info};
#[cfg(feature = "task-poll-telemetry")]
pub(super) mod observation;
#[cfg(feature = "task-poll-telemetry")]
pub(super) mod progress;
pub(crate) mod sockets;

#[cfg(all(feature = "task-poll-telemetry", feature = "upstream-network"))]
#[path = "network/progress/xarxa.rs"]
mod progress_adapter;
#[cfg(all(feature = "task-poll-telemetry", feature = "owned-network"))]
#[path = "network/progress/owned.rs"]
mod progress_adapter;
#[cfg(all(feature = "task-poll-telemetry", feature = "compat-network"))]
#[path = "network/progress/smoltcp.rs"]
mod progress_adapter;
