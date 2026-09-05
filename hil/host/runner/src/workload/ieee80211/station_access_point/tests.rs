use super::*;

#[test]
fn fairness_is_symmetric_and_bounded() {
    assert!(validate_fairness("rx", 10_000, 12_000, 20).is_ok());
    assert!(validate_fairness("rx", 12_001, 10_000, 20).is_err());
    assert!(validate_fairness("rx", 10_000, 12_001, 20).is_err());
}

#[test]
fn access_point_epoch_rejects_one_beacon_period_lateness() {
    let evidence = open_esp_radio_hil_protocol::WifiAccessPointEvidence {
        beacons_transmitted: 1,
        maximum_beacon_lateness_micros: 102_400,
        ..Default::default()
    };
    assert!(validate_access_point_epoch(evidence).is_err());
}
