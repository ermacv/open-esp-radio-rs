use super::*;

#[test]
fn channel_fourteen_has_its_nonuniform_frequency() {
    let channel = WifiChannel::mhz20(14).unwrap();
    assert_eq!(channel.primary_frequency_mhz(), 2_484);
    assert_eq!(channel.center_frequency_mhz(), 2_484);
}

#[test]
fn forty_megahertz_geometry_is_bounded_to_real_secondary_channels() {
    let above = WifiChannel::new_2_4_ghz(9, WifiChannelWidth::Mhz40Above).unwrap();
    let below = WifiChannel::new_2_4_ghz(5, WifiChannelWidth::Mhz40Below).unwrap();
    assert_eq!(above.center_frequency_mhz(), 2_462);
    assert_eq!(below.center_frequency_mhz(), 2_422);
    assert_eq!(
        WifiChannel::new_2_4_ghz(10, WifiChannelWidth::Mhz40Above),
        Err(WifiChannelError::InvalidSecondary {
            primary: 10,
            width: WifiChannelWidth::Mhz40Above,
        })
    );
    assert_eq!(
        WifiChannel::new_2_4_ghz(4, WifiChannelWidth::Mhz40Below),
        Err(WifiChannelError::InvalidSecondary {
            primary: 4,
            width: WifiChannelWidth::Mhz40Below,
        })
    );
}
