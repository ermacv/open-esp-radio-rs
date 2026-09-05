use super::*;
use open_esp_radio_ieee80211::management::MAX_SSID_LEN;

#[test]
fn ssid_is_binary_and_length_checked() {
    let ssid = WifiSsid::new(&[0xff, 0, b'a']).unwrap();
    assert_eq!(ssid.as_bytes(), &[0xff, 0, b'a']);
    assert_eq!(WifiSsid::new(&[]), Err(WifiSsidError::Empty));
    assert!(matches!(
        WifiSsid::new(&[0; MAX_SSID_LEN + 1]),
        Err(WifiSsidError::TooLong { .. })
    ));
}

#[test]
fn scan_channels_are_bounded_and_deduplicated() {
    let channels = StationScanChannels::from_primary_channels(&[1, 6, 6, 11]).unwrap();
    assert_eq!(channels.count(), 3);
    assert!(channels.contains(1));
    assert!(channels.contains(6));
    assert!(channels.contains(11));
    assert!(!channels.contains(14));
    assert_eq!(
        channels.primary_channels().collect::<std::vec::Vec<_>>(),
        [1, 6, 11]
    );
    assert_eq!(channels.primary_channels().len(), 3);
    assert_eq!(
        StationScanChannels::from_primary_channels(&[0]),
        Err(StationScanChannelsError::InvalidPrimary(0))
    );
}

#[test]
fn discovery_keeps_protocol_policy_chip_independent() {
    let ssid = WifiSsid::new(b"portable-station").unwrap();
    let scan = StationScanPolicy::new(
        StationScanChannels::from_primary_channels(&[1, 6, 11]).unwrap(),
        NonZeroU16::new(40).unwrap(),
        StaAssociationPreference::Automatic,
    );
    let discovery = StationDiscovery::new(ssid, scan);
    assert_eq!(discovery.ssid(), ssid);
    assert_eq!(discovery.scan(), scan);
}

#[test]
fn preferred_scan_order_preserves_the_exact_selected_set() {
    let channels = StationScanChannels::from_primary_channels(&[1, 6, 11, 14]).unwrap();
    assert_eq!(
        channels
            .primary_channels_preferred(Some(11))
            .collect::<std::vec::Vec<_>>(),
        [11, 1, 6, 14]
    );
    assert_eq!(
        channels
            .primary_channels_preferred(Some(9))
            .collect::<std::vec::Vec<_>>(),
        [1, 6, 11, 14]
    );
}
