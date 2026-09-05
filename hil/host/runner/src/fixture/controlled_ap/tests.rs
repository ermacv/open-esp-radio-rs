use super::*;
use std::path::Path;

#[test]
fn reads_exact_profile_values_without_accepting_comments_or_prefixes() {
    let profile =
        "# ssid=ignored\nssid=open-radio\nssid_suffix=ignored\nwpa_passphrase=test-passphrase\n";
    assert_eq!(
        required_profile_value(profile, "ssid").unwrap(),
        "open-radio"
    );
    assert_eq!(
        required_profile_value(profile, "wpa_passphrase").unwrap(),
        "test-passphrase"
    );
}

#[test]
fn openwrt_readiness_requires_phy_geometry_and_enabled_hostapd_owner() {
    let config = OpenWrtConfig {
        ssh_target: String::from("fixture"),
        wireless_interface: String::from("phy0-ap0"),
        ingress_interface: String::from("lan1"),
        monitor_interface: None,
        phys: vec![PhyExpectation::Ht40],
        independent_laptop_monitor: false,
    };
    let script = openwrt_ready_script(&config, PhyExpectation::Ht40);
    assert!(script.contains("iw dev phy0-ap0 info"));
    assert!(script.contains("width: 40 MHz"));
    assert!(script.contains("hostapd.phy0-ap0 get_status"));
    assert!(script.contains("grep -Fxq ENABLED"));
}

#[test]
fn rejects_missing_empty_and_ambiguous_profile_values() {
    assert!(required_profile_value("channel=11\n", "ssid").is_err());
    assert!(required_profile_value("ssid=\n", "ssid").is_err());
    assert!(required_profile_value("ssid=one\nssid=two\n", "ssid").is_err());
}

#[test]
fn installed_profile_path_is_absolute_and_not_checkout_specific() {
    assert!(Path::new(INSTALLED_HE20_CONFIG).is_absolute());
}
