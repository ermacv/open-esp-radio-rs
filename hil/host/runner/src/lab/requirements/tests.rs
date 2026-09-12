use super::*;

fn scenario(workload: Workload) -> Scenario {
    let mut scenario: Scenario = toml::from_str(include_str!(
        "../../../../../scenarios/system/timebase.toml"
    ))
    .unwrap();
    scenario.workload = workload;
    scenario
}

#[test]
fn system_and_ieee802154_diagnostics_do_not_need_a_network() {
    for workload in [
        Workload::BootSmoke,
        Workload::MemoryBenchmark {
            boots: 2,
            iterations: 32,
            sizes: vec![64, 4096],
            batch_sizes: vec![1],
        },
        Workload::Timebase {
            boots: 3,
            intervals: 2,
            period_millis: 10,
        },
        Workload::Ieee802154EdEvent {
            boots: 1,
            poll_limit: 10,
            timer_threshold: 2,
        },
        Workload::Ieee802154EventStatus {
            boots: 1,
            poll_limit: 10,
            timer_threshold: 2,
        },
    ] {
        assert_eq!(
            Requirements::for_scenario(&scenario(workload)),
            Requirements::default()
        );
    }
}

#[test]
fn udp_evidence_and_optional_observer_have_distinct_owners() {
    let mut scenario = scenario(Workload::Udp {
        station_pause: None,
        station_pause_after_millis: None,
        direction: Direction::Rx,
        duration_seconds: 1,
        rx_rate_bps: Some(1000),
        tx_rate_bps: None,
        payload_bytes: 64,
    });
    let required = Requirements::for_scenario(&scenario);
    assert!(required.station_network && required.station_udp_rx_capture);
    assert!(!required.local_radio() && !required.openwrt_tx_monitor);
    scenario.evidence.independent_laptop_air_monitor = true;
    scenario.evidence.openwrt_tx_monitor_rx = true;
    let required = Requirements::for_scenario(&scenario);
    assert!(required.local_radio() && required.openwrt_tx_monitor);
    assert!(!required.laptop_client);
}

#[test]
fn ap_clients_do_not_hide_the_initial_station_dependency() {
    let mut scenario = scenario(Workload::AccessPoint {
        probe_load: false,
        cycles: 1,
        boots: 1,
        timeout_seconds: 30,
        client: AccessPointClient::Laptop,
        security: open_esp_radio_hil_protocol::WifiAccessPointSecurity::Wpa2Personal,
        traffic: crate::scenario::AccessPointTraffic::None,
    });
    let required = Requirements::for_scenario(&scenario);
    assert!(required.station_network && required.laptop_client);
    assert!(!required.openwrt_client);
    scenario.criteria.minimum_concurrent_ap_clients = Some(2);
    let required = Requirements::for_scenario(&scenario);
    assert!(required.openwrt_client && required.laptop_client);
}

#[test]
fn suite_requirements_include_every_selected_owner() {
    let system = scenario(Workload::BootSmoke);
    let pair = scenario(Workload::StationAccessPointReconnect {
        timeout_seconds: 30,
    });
    let union = Requirements::union(&[&system, &pair]);
    assert!(union.station_network && union.station_control && union.laptop_client);
    assert_eq!(union, Requirements::union(&[&pair, &system, &pair]));
}

#[test]
fn bluetooth_requires_its_adapter_without_a_network() {
    let required = Requirements::for_scenario(&scenario(Workload::BluetoothDtm {
        boots: 2,
        minimum_packets: 10,
        quiet_cycles: Some(100),
    }));
    assert!(required.bluetooth_adapter);
    assert!(!required.network());
    assert!(!required.local_radio());
    assert!(
        Requirements {
            laptop_air_monitor: true,
            ..required
        }
        .network()
    );
}

#[test]
fn probe_load_requires_both_clients_and_air_observer() {
    let mut scenario: Scenario = toml::from_str(include_str!(
        "../../../../../scenarios/ieee80211/access-point/diagnostic-ap-probe-load.toml"
    ))
    .unwrap();
    scenario.validate().unwrap();
    let required = Requirements::for_scenario(&scenario);
    assert!(required.probe_load && required.laptop_client && required.openwrt_client);
    assert!(required.openwrt_tx_monitor);
    scenario.criteria.minimum_concurrent_ap_clients = Some(1);
    assert!(scenario.validate().is_err());
    scenario.criteria.minimum_concurrent_ap_clients = Some(2);
    if let Workload::AccessPoint {
        traffic:
            crate::scenario::AccessPointTraffic::UdpMultiClient {
                duration_seconds, ..
            },
        ..
    } = &mut scenario.workload
    {
        *duration_seconds = 6;
    }
    assert!(scenario.validate().is_err());
}
