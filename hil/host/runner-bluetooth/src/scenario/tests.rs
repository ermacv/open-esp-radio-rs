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
            ImageClass::BluetoothDtm,
        ),
        ("kind = 'scannable-advertising'", ImageClass::BluetoothDtm),
        ("kind = 'directed-advertising'", ImageClass::BluetoothDtm),
    ] {
        let scenario = parse(text);
        scenario.validate().unwrap();
        let plan = scenario.plan();
        assert_eq!(plan.image, image, "{text}");
        assert!(plan.requirements.bluetooth_adapter && !plan.requirements.network());
    }
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
