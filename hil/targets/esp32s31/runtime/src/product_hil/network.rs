//! HIL-owned stack composition; all implementations share the traffic workloads.
#![forbid(unsafe_code)]
pub(super) struct Settings {
    pub ipv4: Option<oer_hil_protocol::NetworkIpv4Configuration>,
    pub seed: u64,
    pub rx_checksum: oer_hil_protocol::WifiRxChecksumPolicy,
    pub tx_udp_checksum: oer_hil_protocol::WifiTxUdpChecksumPolicy,
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
pub(super) use ipv4::{configure, info};
#[cfg(feature = "upstream-network")]
use oer_hil_target_core::network::{checksum, ipv4};
#[cfg(feature = "task-poll-telemetry")]
pub(super) mod observation;
#[cfg(feature = "task-poll-telemetry")]
pub(super) use oer_hil_target_core::network::progress;
pub(crate) use oer_hil_target_core::network::sockets;

#[cfg(all(feature = "upstream-network", feature = "driver-observation"))]
mod arp;
