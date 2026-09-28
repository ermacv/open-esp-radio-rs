use super::*;

#[test]
fn parses_only_an_associated_mac_identity() {
    assert_eq!(
        parse_bssid("Connected to 30:ed:a0:f3:f6:d1 (on wlan0)\n").unwrap(),
        "30:ed:a0:f3:f6:d1"
    );
    assert!(parse_bssid("Not connected.\n").is_err());
    assert!(parse_bssid("Connected to not-a-mac (on wlan0)\n").is_err());
}

#[test]
fn unknown_states_are_not_misreported_as_discovery_failures() {
    assert_eq!(connection_stage(Some("DISCONNECTED")), "unknown");
    assert_eq!(connection_stage(None), "unknown");
    assert_eq!(connection_stage(Some("4WAY_HANDSHAKE")), "key-negotiation");
}

#[test]
fn every_cycle_contributes_its_associated_station_address_once() {
    let output = tempfile::tempdir().unwrap();
    let record = |cycle: &str, address: Option<&str>| {
        let directory = output.path().join(cycle).join("linux-client");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("connection.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema": 2, "connected": true, "station_address": address,
            }))
            .unwrap(),
        )
        .unwrap();
    };
    record("cycle-00", Some("70:15:FB:A8:48:F0"));
    record("cycle-01", Some("70:15:fb:a8:48:f0"));
    record("cycle-02", None);
    assert_eq!(
        connected_station_addresses(output.path()).unwrap(),
        ["70:15:fb:a8:48:f0"]
    );
    record("cycle-03", Some("42:a1:a0:9c:dc:c3"));
    assert_eq!(
        connected_station_addresses(output.path()).unwrap(),
        ["42:a1:a0:9c:dc:c3", "70:15:fb:a8:48:f0"]
    );
}
