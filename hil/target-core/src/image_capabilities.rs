//! The capabilities a HIL runtime image reports, as a function of the Cargo
//! features it was built with.
//!
//! The runtime answers `QueryCapabilities` with this function of its own
//! enabled features, and the host derives the capabilities each image class
//! reports from the same function applied to the class's features and the
//! runtime manifest's feature graph. The two cannot drift apart: the host
//! identifies a flashed image by these capabilities.

use oer_hil_protocol::{DiagnosticFeature, DiagnosticFeatures, FeatureCapabilities};

/// The capabilities of a runtime image whose Cargo features are those for
/// which `enabled` is true.
pub fn feature_capabilities(enabled: &dyn Fn(&str) -> bool) -> FeatureCapabilities {
    if enabled("system-watchdog") {
        return FeatureCapabilities {
            system_watchdog: true,
            structured_evidence: true,
            ..FeatureCapabilities::default()
        };
    }
    if enabled("bluetooth-radio") && !enabled("open-radio-hil") {
        let base = FeatureCapabilities {
            structured_evidence: true,
            ..FeatureCapabilities::default()
        };
        return if enabled("bluetooth-secure-gatt") {
            FeatureCapabilities {
                bluetooth_secure_gatt: true,
                ..base
            }
        } else if enabled("bluetooth-gatt") {
            FeatureCapabilities {
                bluetooth_gatt: true,
                ..base
            }
        } else {
            FeatureCapabilities {
                bluetooth_dtm: true,
                bluetooth_hci: true,
                phy_rx_hot_sram: enabled("phy-rx-hot-sram"),
                ..base
            }
        };
    }
    let traffic = !enabled("memory-benchmark");
    FeatureCapabilities {
        bluetooth_gatt: enabled("wifi-ble-coex"),
        bluetooth_secure_gatt: false,
        bluetooth_dtm: false,
        bluetooth_hci: false,
        system_watchdog: false,
        phy_fault_injection: enabled("phy-fault-injection"),
        phy_register_image: enabled("open-radio-hil") && traffic,
        udp: traffic,
        tcp: traffic,
        rx: traffic,
        tx: traffic,
        bidirectional: traffic,
        runtime_initialization: traffic,
        runtime_configuration: traffic,
        structured_evidence: true,
        udp_multi_flow: traffic,
        startup_artifact: traffic,
        station_epoch_control: traffic,
        wifi_role_control: traffic,
        wifi_access_point: traffic,
        simultaneous_station_access_point: traffic,
        wifi_monitor_capture: traffic,
        station_lifecycle_events: traffic,
        driver_observation_evidence: enabled("driver-observation"),
        rx_delivery_evidence: enabled("rx-delivery-telemetry"),
        diagnostic_features: DiagnosticFeatures::empty()
            .with(
                DiagnosticFeature::RxOwnership,
                enabled("rx-ownership-telemetry"),
            )
            .with(DiagnosticFeature::PcProfile, enabled("pc-profile"))
            .with(
                DiagnosticFeature::StationExit,
                enabled("station-exit-evidence") && !enabled("driver-observation"),
            ),
        phy_rx_hot_sram: enabled("phy-rx-hot-sram"),
        task_poll_evidence: enabled("connected-datapath-poll-telemetry"),
        tx_architecture_probe: enabled("tx-architecture-probes"),
        core0_rx_cycle_evidence: enabled("core0-rx-cycle-telemetry")
            || enabled("core0-rx-coarse-telemetry"),
        mac_irq_evidence: enabled("mac-irq-telemetry"),
        network_scheduler_evidence: false,
        data_plane_placement: traffic,
        timebase_probe: true,
        memory_benchmark: !traffic,
        ieee802154_event_status_probe: enabled("ieee802154-event-status-probe"),
        ieee802154_ed_event_probe: enabled("ieee802154-ed-event-probe"),
        ieee802154_air_check: enabled("ieee802154-radio"),
        ieee802154_session: enabled("ieee802154-radio"),
        ieee802154_thread: enabled("ieee802154-thread"),
        ieee802154_route_probe: enabled("ieee802154-route-probe"),
    }
}

/// Every Cargo feature [`feature_capabilities`] reads, so the host can check
/// that the runtime manifest declares each of them.
pub const READ_FEATURES: &[&str] = &[
    "system-watchdog",
    "bluetooth-radio",
    "open-radio-hil",
    "bluetooth-secure-gatt",
    "bluetooth-gatt",
    "phy-rx-hot-sram",
    "memory-benchmark",
    "wifi-ble-coex",
    "phy-fault-injection",
    "driver-observation",
    "rx-delivery-telemetry",
    "rx-ownership-telemetry",
    "pc-profile",
    "station-exit-evidence",
    "connected-datapath-poll-telemetry",
    "tx-architecture-probes",
    "core0-rx-cycle-telemetry",
    "core0-rx-coarse-telemetry",
    "mac-irq-telemetry",
    "ieee802154-event-status-probe",
    "ieee802154-ed-event-probe",
    "ieee802154-radio",
    "ieee802154-thread",
    "ieee802154-route-probe",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn with(features: &[&str]) -> FeatureCapabilities {
        feature_capabilities(&|feature| features.contains(&feature))
    }

    #[test]
    fn the_family_follows_the_image_s_features() {
        assert!(with(&["system-watchdog"]).system_watchdog);
        let dtm = with(&["bluetooth-radio"]);
        assert!(dtm.bluetooth_dtm && dtm.bluetooth_hci && !dtm.udp);
        assert!(with(&["bluetooth-radio", "bluetooth-gatt"]).bluetooth_gatt);
        assert!(with(&["bluetooth-radio", "bluetooth-secure-gatt"]).bluetooth_secure_gatt);
        // The coexistence image is a Wi-Fi image with the GATT application.
        let coex = with(&["open-radio-hil", "bluetooth-radio", "wifi-ble-coex"]);
        assert!(coex.bluetooth_gatt && coex.udp && !coex.bluetooth_dtm);
    }

    #[test]
    fn a_memory_benchmark_image_offers_no_traffic() {
        let benchmark = with(&["open-radio-hil", "memory-benchmark"]);
        assert!(benchmark.memory_benchmark && !benchmark.udp && !benchmark.phy_register_image);
        let performance = with(&["open-radio-hil"]);
        assert!(!performance.memory_benchmark && performance.udp && performance.phy_register_image);
    }

    #[test]
    fn station_exit_evidence_is_its_own_only_without_driver_observation() {
        let exit = |features: &[&str]| {
            with(features)
                .diagnostic_features
                .contains(DiagnosticFeature::StationExit)
        };
        assert!(exit(&["open-radio-hil", "station-exit-evidence"]));
        assert!(!exit(&[
            "open-radio-hil",
            "station-exit-evidence",
            "driver-observation"
        ]));
    }
}
