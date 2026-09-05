use super::*;

#[test]
fn ssid_is_encoded_without_shell_or_wpa_quoting() {
    assert_eq!(encode_hex(b"lab \\\" ap"), "6c6162205c22206170");
}

#[test]
fn busybox_ping_summary_is_typed() {
    assert_eq!(
        parse_ping_summary(
            "10 packets transmitted, 10 packets received, 0% packet loss\nround-trip min/avg/max = 1.0/2.0/3.0 ms",
        ),
        Some(SecondaryClientProbeEvidence {
            transmitted: 10,
            received: 10,
        }),
    );
}

#[test]
fn forwarding_address_is_typed() {
    assert_eq!(
        tagged_ipv4("forward_address=192.168.178.2\n", "forward_address").unwrap(),
        Ipv4Addr::new(192, 168, 178, 2),
    );
}

#[test]
fn link_snapshot_parser_and_counter_delta_are_strict() {
    let output = "rx_bytes=200\nrx_packets=30\nrx_duration=88\nrx_bitrate=150.0 MBit/s MCS 7 40MHz short GI\ntx_bytes=100\ntx_packets=20\ntx_bitrate=135.0 MBit/s MCS 6 40MHz short GI\ntx_retries=3\ntx_failed=1\ntx_duration=77\ntid0_aqm_drops=0\n";
    assert_eq!(tagged_u64(output, "rx_packets").unwrap(), 30);
    assert_eq!(
        tagged_optional_u64(output, "rx_duration").unwrap(),
        Some(88)
    );
    assert_eq!(
        tagged_optional_string(output, "rx_bitrate").as_deref(),
        Some("150.0 MBit/s MCS 7 40MHz short GI"),
    );
    assert_eq!(tagged_u64(output, "tx_packets").unwrap(), 20);
    assert_eq!(
        tagged_optional_string(output, "tx_bitrate").as_deref(),
        Some("135.0 MBit/s MCS 6 40MHz short GI"),
    );
    assert_eq!(counter_delta("packets", 20, 27).unwrap(), 7);
    assert!(counter_delta("packets", 27, 20).is_err());
    assert_eq!(
        optional_counter_delta("duration", Some(20), Some(27)).unwrap(),
        Some(7),
    );
    assert!(optional_counter_delta("duration", Some(20), None).is_err());
}
