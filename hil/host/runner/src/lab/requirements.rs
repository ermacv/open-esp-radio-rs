//! Execution prerequisites derived from workload behavior, never scenario names.

use serde::{Deserialize, Serialize};

use crate::scenario::{AccessPointClient, Direction, Scenario, Workload};

/// Serial ownership and firmware are common to every scenario. These are the
/// additional fixture services consumed by the selected workload and evidence.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Requirements {
    pub(crate) station_network: bool,
    pub(crate) station_control: bool,
    pub(crate) station_udp_rx_capture: bool,
    pub(crate) station_udp_tx_capture: bool,
    pub(crate) laptop_client: bool,
    pub(crate) openwrt_client: bool,
    pub(crate) openwrt_tx_monitor: bool,
    pub(crate) laptop_air_monitor: bool,
}

impl Requirements {
    pub(crate) fn for_scenario(scenario: &Scenario) -> Self {
        let mut required = Self::default();
        match &scenario.workload {
            Workload::BootSmoke
            | Workload::Timebase { .. }
            | Workload::MemoryBenchmark { .. }
            | Workload::Ieee802154EventStatus { .. }
            | Workload::Ieee802154EdEvent { .. } => return required,
            Workload::Udp { direction, .. } => {
                required.station_udp_rx_capture = *direction != Direction::Tx;
                required.station_udp_tx_capture = *direction != Direction::Rx;
            }
            Workload::AccessPoint { client, .. } => {
                required.laptop_client = *client == AccessPointClient::Laptop;
                required.openwrt_client = *client == AccessPointClient::OpenWrt
                    || scenario.criteria.minimum_concurrent_ap_clients.unwrap_or(1) >= 2;
            }
            Workload::StationAccessPoint { .. } | Workload::StationAccessPointReconnect { .. } => {
                required.laptop_client = true
            }
            Workload::Tcp { .. }
            | Workload::Icmp { .. }
            | Workload::StationReconnect { .. }
            | Workload::StationApLoss { .. }
            | Workload::StationApAbsence { .. }
            | Workload::WifiRole { .. }
            | Workload::MonitorCapture { .. } => {}
        }
        // AP and monitor workloads also begin by qualifying the connected STA.
        // Keep that real dependency visible until their lifecycle changes.
        required.station_network = true;
        required.station_control = matches!(
            scenario.workload,
            Workload::StationApLoss { .. }
                | Workload::StationApAbsence { .. }
                | Workload::StationAccessPointReconnect { .. }
        );
        required.openwrt_tx_monitor = scenario.evidence.openwrt_tx_monitor_rx;
        required.laptop_air_monitor = scenario.evidence.independent_laptop_air_monitor;
        required
    }

    pub(crate) fn union(scenarios: &[&Scenario]) -> Self {
        let mut required = Self::default();
        for scenario in scenarios {
            let next = Self::for_scenario(scenario);
            required.station_network |= next.station_network;
            required.station_control |= next.station_control;
            required.station_udp_rx_capture |= next.station_udp_rx_capture;
            required.station_udp_tx_capture |= next.station_udp_tx_capture;
            required.laptop_client |= next.laptop_client;
            required.openwrt_client |= next.openwrt_client;
            required.openwrt_tx_monitor |= next.openwrt_tx_monitor;
            required.laptop_air_monitor |= next.laptop_air_monitor;
        }
        required
    }

    pub(crate) fn local_radio(self) -> bool {
        self.laptop_client || self.laptop_air_monitor
    }
}

#[cfg(test)]
mod tests;
