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
