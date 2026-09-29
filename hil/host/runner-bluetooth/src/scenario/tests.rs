use super::*;

fn parse(text: &str) -> BluetoothScenario {
    toml::from_str(text).unwrap()
}

#[test]
fn each_workload_selects_its_image() {
    for (text, image) in [
        ("kind = 'gatt'", ImageClass::BluetoothGatt),
        (
            "kind = 'secure-gatt'\nshutdown = 'hci-read-failure'",
            ImageClass::BluetoothSecureGatt,
        ),
        (
            "kind = 'dtm'\nboots = 1\nminimum_packets = 10",
            ImageClass::BluetoothHci,
        ),
        ("kind = 'scannable-advertising'", ImageClass::BluetoothHci),
        ("kind = 'directed-advertising'", ImageClass::BluetoothHci),
        ("kind = 'acl-backpressure'", ImageClass::BluetoothHci),
        ("kind = 'active-scanning'", ImageClass::BluetoothHci),
        (
            "kind = 'dtm-peer'\nminimum_packets = 100",
            ImageClass::BluetoothHci,
        ),
        (
            "kind = 'peripheral'\nconnections = 2\nhold_millis = 100\ntermination = 'peer-reset'",
            ImageClass::BluetoothHci,
        ),
        (
            "kind = 'peripheral'\nconnections = 2\nhold_millis = 100\ntermination = 'peer-reset'\nsecurity = 'key-refresh'",
            ImageClass::BluetoothHci,
        ),
        (
            "kind = 'security-failure'\nfailure = 'missing-key'\nread_version_before_disconnect = true",
            ImageClass::BluetoothHci,
        ),
        (
            "kind = 'peripheral'\nconnections = 3\nhold_millis = 100\ntermination = 'peer-reset'\nrestart_between_connections = true\nretire_after = true",
            ImageClass::BluetoothHciDiagnostics,
        ),
    ] {
        let scenario = parse(text);
        scenario.validate().unwrap();
        let plan = scenario.plan();
        assert_eq!(plan.image, image, "{text}");
        assert!(!plan.requirements.network());
        assert_eq!(
            plan.requirements.bluetooth_adapter,
            scenario.adapter_preflight().is_some(),
            "{text}"
        );
    }
}

#[test]
fn the_peer_scenario_claims_the_peer_board_and_no_linux_adapter() {
    let scenario = parse("kind = 'dtm-peer'\nminimum_packets = 100");
    let plan = scenario.plan();
    assert!(plan.requirements.peer && !plan.requirements.bluetooth_adapter);
    assert_eq!(
        scenario.peer_image(),
        Some(crate::fixture::dtm_peer::DTM_PEER_IMAGE)
    );
    assert!(
        !parse("kind = 'dtm'\nboots = 1\nminimum_packets = 10")
            .plan()
            .requirements
            .peer
    );
    assert!(
        parse("kind = 'dtm-peer'\nminimum_packets = 0")
            .validate()
            .is_err()
    );
}

#[test]
fn out_of_range_or_retired_workloads_are_rejected() {
    assert!(
        parse("kind = 'dtm'\nboots = 0\nminimum_packets = 1")
            .validate()
            .is_err()
    );
    for text in [
        "kind = 'gatt'\nimage = 'bluetooth-gatt'",
        "kind = 'peripheral'\nboots = 1\nconnections = 2\nhold_millis = 0",
        "kind = 'maintenance-deadline'",
        "kind = 'watchdog-reset'",
    ] {
        assert!(toml::from_str::<BluetoothScenario>(text).is_err(), "{text}");
    }
}

#[test]
fn peripheral_scenarios_bound_their_cycles_and_name_the_termination() {
    let valid =
        "kind = 'peripheral'\nconnections = 100\nhold_millis = 0\ntermination = 'target-reset'";
    parse(valid).validate().unwrap();
    for invalid in [
        "kind = 'peripheral'\nconnections = 0\nhold_millis = 0\ntermination = 'peer-reset'",
        "kind = 'peripheral'\nconnections = 101\nhold_millis = 0\ntermination = 'peer-reset'",
        "kind = 'peripheral'\nconnections = 1\nhold_millis = 5001\ntermination = 'peer-reset'",
    ] {
        assert!(parse(invalid).validate().is_err(), "{invalid}");
    }
    assert!(
        toml::from_str::<BluetoothScenario>(
            "kind = 'peripheral'\nconnections = 1\nhold_millis = 0\ntermination = 'peer-sleep'"
        )
        .is_err()
    );
}

#[test]
fn the_active_data_mic_failure_needs_the_diagnostic_image() {
    let scenario = parse("kind = 'security-failure'\nfailure = 'active-data-mic'");
    scenario.validate().unwrap();
    assert_eq!(scenario.image(), ImageClass::BluetoothHciDiagnostics);
    let plain = ImageClass::BluetoothHci
        .capabilities_on("esp32s31")
        .unwrap();
    assert!(!scenario.served_by(&plain));
    let diagnostic = ImageClass::BluetoothHciDiagnostics
        .capabilities_on("esp32s31")
        .unwrap();
    assert!(scenario.served_by(&diagnostic));
}
