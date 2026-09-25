use super::*;

#[test]
fn gro_payload_accounting_keeps_packet_count_distinct() {
    let result = parse_units("1208\tFalse\t0\n3608\t0\t0\n2408\tFalse\t0\n", 1200).unwrap();
    assert_eq!(result.captured_packets, 3);
    assert_eq!(result.payload_units, 6);
    assert_eq!(result.coalesced_packets, 2);
    assert_eq!(result.maximum_payload_units_per_packet, 3);
}

#[test]
fn invalid_and_fragmented_capture_is_not_rounded_into_delivered_datagrams() {
    for fields in [
        "1207\t0\t0",
        "8\t0\t0",
        "7\t0\t0",
        "1208\t1\t0",
        "1208\t0\t8",
        "1208\t0",
        "\t0\t0",
    ] {
        assert!(parse_units(fields, 1200).is_err(), "{fields}");
    }
}

#[test]
fn observed_host_delivery_is_a_lower_bound_even_without_capture_overflow() {
    assert!(validate_coverage(100, 100, Some(100)).is_ok());
    assert!(validate_coverage(100, 100, Some(99)).is_ok());
    assert!(validate_coverage(99, 100, Some(100)).is_err());
    assert!(validate_coverage(100, 99, Some(100)).is_err());
    assert!(validate_coverage(100, 100, None).is_err());
}

#[test]
fn filter_rejects_control_sized_and_fragmented_payloads() {
    for length in [0, 32, 63, 1473, usize::MAX] {
        assert!(data_filter(Ipv4Addr::LOCALHOST, Ipv4Addr::LOCALHOST, 4324, 9002, length).is_err());
    }
    assert!(data_filter(Ipv4Addr::LOCALHOST, Ipv4Addr::LOCALHOST, 4324, 9002, 1200).is_ok());
}
