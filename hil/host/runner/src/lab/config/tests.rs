use super::*;

#[test]
fn parses_static_ipv4() {
    let parsed = parse_ipv4(RawIpv4Config::Static {
        address: "192.168.1.182/24".into(),
        gateway: Some(Ipv4Addr::new(192, 168, 1, 1)),
    })
    .unwrap();
    assert_eq!(
        parsed,
        NetworkIpv4Configuration::Static {
            address: [192, 168, 1, 182],
            prefix_length: 24,
            gateway: Some([192, 168, 1, 1]),
        }
    );
}

#[test]
fn rejects_unsafe_fixture_tokens() {
    assert!(validate_shell_token("iface", "phy0-ap0; reboot").is_err());
}

#[test]
fn physical_identity_is_stable_and_path_safe() {
    assert!(validate_identifier("lab.id", "berlin-s31-01").is_ok());
    assert!(validate_identifier("lab.id", "Berlin/S31").is_err());
    assert!(validate_identifier("device.id", "").is_err());
}
