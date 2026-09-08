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
fn rejects_missing_empty_and_ambiguous_profile_values() {
    assert!(required_profile_value("channel=11\n", "ssid").is_err());
    assert!(required_profile_value("ssid=\n", "ssid").is_err());
    assert!(required_profile_value("ssid=one\nssid=two\n", "ssid").is_err());
}

#[test]
fn installed_profile_path_is_absolute_and_not_checkout_specific() {
    assert!(Path::new(INSTALLED_HE20_CONFIG).is_absolute());
}
