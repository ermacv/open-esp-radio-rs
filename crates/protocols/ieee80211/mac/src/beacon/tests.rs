use crate::ap::profile::tests::TEST_ADVERTISEMENT;

use super::{
    ApBeaconBuildError, WPA2_BEACON_CAPACITY, WPA2_PERSONAL_CCMP_PSK_RSN_IE, dtim, stamp,
    write_wpa2_ht_beacon,
};
use crate::{
    channel::{WifiChannel, WifiChannelWidth},
    ht::HtDuplicateMcs32,
    ssid::WifiSsid,
};

fn beacon() -> [u8; 44] {
    let mut bytes = [0_u8; 44];
    bytes[..2].copy_from_slice(&0x0080_u16.to_le_bytes());
    bytes[32..34].copy_from_slice(&100_u16.to_le_bytes());
    // SSID with zero-length payload, followed by a complete TIM.
    bytes[36] = 0;
    bytes[37] = 0;
    bytes[38] = 5;
    bytes[39] = 4;
    bytes[40] = 1;
    bytes[41] = 2;
    bytes
}

#[test]
fn executor_tsf_drives_dtim_and_group_indication() {
    let mut bytes = beacon();

    assert_eq!(stamp(&mut bytes, 0, true), Some((1, 2)));
    assert_eq!(dtim(&bytes), Some((38, 1, 2)));
    assert_eq!(bytes[42] & 1, 0);

    assert_eq!(stamp(&mut bytes, 100 * 1_024, true), Some((0, 2)));
    assert_eq!(dtim(&bytes), Some((38, 0, 2)));
    assert_eq!(bytes[42] & 1, 1);

    assert_eq!(stamp(&mut bytes, 3 * 100 * 1_024, false), Some((0, 2)));
    assert_eq!(bytes[42] & 1, 0);
}

#[test]
fn builds_the_bounded_wpa2_ht20_beacon() {
    let ap = [0x02, 0, 0, 0, 0, 1];
    let ssid = WifiSsid::new(b"open-radio-ap").unwrap();
    let mut bytes = [0; WPA2_BEACON_CAPACITY];
    let len = write_wpa2_ht_beacon(
        &TEST_ADVERTISEMENT,
        &mut bytes,
        ap,
        &ssid,
        WifiChannel::mhz20(6).unwrap(),
        100,
        2,
        0x0abc,
    )
    .unwrap();

    assert_eq!(&bytes[..2], &0x0080_u16.to_le_bytes());
    assert_eq!(&bytes[4..10], &[0xff; 6]);
    assert_eq!(&bytes[10..16], &ap);
    assert_eq!(&bytes[16..22], &ap);
    assert_eq!(&bytes[22..24], &0xabc0_u16.to_le_bytes());
    assert_eq!(dtim(&bytes[..len]), Some((64, 1, 2)));
    assert!(
        bytes[..len]
            .windows(22)
            .any(|window| window == WPA2_PERSONAL_CCMP_PSK_RSN_IE)
    );
    assert!(bytes[..len].windows(3).any(|window| window == [3, 1, 6]));
    assert!(bytes[..len].windows(2).any(|window| window == [45, 26]));
    assert!(bytes[..len].windows(3).any(|window| window == [61, 22, 6]));
}

#[test]
fn ht40_beacon_advertises_the_validated_secondary_channel() {
    let ssid = WifiSsid::new(b"open-radio-ap").unwrap();
    let mut bytes = [0; WPA2_BEACON_CAPACITY];
    let channel = WifiChannel::new_2_4_ghz(6, WifiChannelWidth::Mhz40Above).unwrap();
    let len = write_wpa2_ht_beacon(
        &TEST_ADVERTISEMENT,
        &mut bytes,
        [2; 6],
        &ssid,
        channel,
        100,
        2,
        0,
    )
    .unwrap();
    assert!(
        bytes[..len]
            .windows(4)
            .any(|window| window == [45, 26, 0x6e, 0x10])
    );
    assert!(
        bytes[..len]
            .windows(4)
            .any(|window| window == [61, 22, 6, 0x05])
    );
    let ht_capability = bytes[..len]
        .windows(2)
        .position(|window| window == [45, 26])
        .expect("the HT40 beacon includes HT Capabilities");
    assert_eq!(
        bytes[ht_capability + HtDuplicateMcs32::CAPABILITY_IE_BYTE]
            & HtDuplicateMcs32::CAPABILITY_IE_MASK,
        0,
        "the AP must not advertise unqualified local MCS32 reception"
    );
    assert_eq!(
        bytes[ht_capability + 17],
        0x01,
        "the AP must advertise only the implemented equal MCS0..MCS7 sets"
    );
}

#[test]
fn rejects_unrepresentable_beacon_policy_before_mutation() {
    let ssid = WifiSsid::new(b"ap").unwrap();
    let mut bytes = [0xaa; WPA2_BEACON_CAPACITY];
    assert_eq!(
        write_wpa2_ht_beacon(
            &TEST_ADVERTISEMENT,
            &mut bytes,
            [0; 6],
            &ssid,
            WifiChannel::mhz20(14).unwrap(),
            100,
            2,
            0,
        ),
        Err(ApBeaconBuildError::InvalidPrimaryChannel)
    );
    assert!(bytes.iter().all(|byte| *byte == 0xaa));
}
