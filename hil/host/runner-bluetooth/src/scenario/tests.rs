use super::*;

fn parse(text: &str) -> BluetoothScenario {
    toml::from_str(text).unwrap()
}

#[test]
fn maintenance_choices_select_the_image() {
    for (text, image) in [
        ("kind = 'gatt'", ImageClass::BluetoothGatt),
        (
            "kind = 'secure-gatt'\nshutdown = 'hci-read-failure'",
            ImageClass::BluetoothSecureGatt,
        ),
        ("kind = 'phy-watchdog'", ImageClass::BluetoothWatchdogReset),
        (
            "kind = 'watchdog-reset'",
            ImageClass::BluetoothWatchdogReset,
        ),
        (
            "kind = 'maintenance-deadline'",
            ImageClass::BluetoothPhyMaintenance,
        ),
        ("kind = 'acl-backpressure'", ImageClass::BluetoothDtm),
        (
            "kind = 'acl-backpressure'\nactive_maintenance = true",
            ImageClass::BluetoothPhyMaintenance,
        ),
        (
            "kind = 'encrypted-acl'\nexercise = 'key-refresh'",
            ImageClass::BluetoothDtm,
        ),
        (
            "kind = 'encrypted-acl'\nexercise = 'active-maintenance'",
            ImageClass::BluetoothPhyMaintenance,
        ),
        (
            "kind = 'peripheral'\nboots = 1\nconnections = 2\nhold_millis = 0\nphy_maintenance = { kind = 'automatic' }",
            ImageClass::BluetoothPhyMaintenance,
        ),
        (
            "kind = 'peripheral'\nboots = 1\nconnections = 2\nhold_millis = 0\nphy_maintenance = { kind = 'between-connections', calibration_threshold = 0 }",
            ImageClass::BluetoothDtm,
        ),
    ] {
        let scenario = parse(text);
        scenario.validate().unwrap();
        let plan = scenario.plan();
        assert_eq!(plan.image, image, "{text}");
        assert!(plan.requirements.bluetooth_adapter && !plan.requirements.network());
    }
}

#[test]
fn contradictory_or_out_of_range_workloads_are_rejected() {
    for text in [
        "kind = 'security-failure'\nfailure = 'wrong-key'\nread_version_before_disconnect = true",
        "kind = 'acl-calibration'\nduration_millis = 9999\nminimum_calibrations = 2",
        "kind = 'dtm'\nboots = 0\nminimum_packets = 1",
        "kind = 'peripheral'\nboots = 1\nconnections = 1\nhold_millis = 0\nrestart_between_connections = true",
        "kind = 'peripheral'\nboots = 1\nconnections = 1\nhold_millis = 0\nphy_maintenance = { kind = 'between-connections' }",
    ] {
        assert!(parse(text).validate().is_err(), "{text}");
    }
    for text in [
        "kind = 'gatt'\nimage = 'bluetooth-gatt'",
        "kind = 'encrypted-acl'\nkey_refresh = true",
        "kind = 'peripheral'\nboots = 1\nconnections = 2\nhold_millis = 0\nphy_maintenance = { kind = 'automatic', calibration_threshold = 1 }",
    ] {
        assert!(toml::from_str::<BluetoothScenario>(text).is_err(), "{text}");
    }
}
