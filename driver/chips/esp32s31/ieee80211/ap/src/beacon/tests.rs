use super::*;
use open_esp_radio_ieee80211::beacon::{TimAssociationId, TimVirtualBitmap};

#[test]
fn static_storage_owns_beacon_dtim_and_next_deadline() {
    let mut storage = [0; WPA2_BEACON_CAPACITY];
    let ssid = WifiSsid::new(b"ap").unwrap();
    let mut beacon = Esp32s31ApBeacon::new(
        &mut storage,
        [2; 6],
        &ssid,
        WifiChannel::mhz20(6).unwrap(),
        100,
        2,
        3,
    )
    .unwrap();
    assert!(beacon.publication_due(102_400));
    assert_eq!(beacon.next_delay(102_400), None);
    let mut bitmap = TimVirtualBitmap::<2>::try_new().unwrap();
    bitmap.set(TimAssociationId::new(1).unwrap(), true).unwrap();
    let frame = beacon.prepare(102_400, 4, true, bitmap.partial()).unwrap();
    let (offset, count, period) = dtim(frame).unwrap();
    assert_eq!((count, period), (0, 2));
    assert_eq!(frame[offset + 4] & 1, 1);
    assert_eq!(frame[offset + 5], 0x02);
    assert_eq!(&frame[22..24], &0x0040_u16.to_le_bytes());
    assert_eq!(beacon.next_delay(102_400), Some((204_800, 103)));
    assert!(!beacon.publication_due(204_799));
    assert!(beacon.publication_due(204_800));
    assert!(beacon.publication_due(204_801));
    assert_eq!(beacon.publication_lateness(204_800), (0, 0));
    assert_eq!(beacon.publication_lateness(204_801), (0, 1));
    assert_eq!(beacon.publication_lateness(307_200), (1, 102_400));
    assert_eq!(beacon.publication_lateness(307_201), (1, 102_401));
}

#[test]
fn late_publication_does_not_move_the_absolute_tbtt_schedule() {
    let mut storage = [0; WPA2_BEACON_CAPACITY];
    let ssid = WifiSsid::new(b"ap").unwrap();
    let mut beacon = Esp32s31ApBeacon::new(
        &mut storage,
        [2; 6],
        &ssid,
        WifiChannel::mhz20(6).unwrap(),
        100,
        2,
        3,
    )
    .unwrap();

    let bitmap = TimVirtualBitmap::<2>::try_new().unwrap();
    beacon.prepare(102_400, 4, false, bitmap.partial()).unwrap();
    assert_eq!(beacon.next_delay(102_400), Some((204_800, 103)));
    assert_eq!(beacon.publication_lateness(204_900), (0, 100));

    beacon.prepare(204_900, 5, false, bitmap.partial()).unwrap();
    assert_eq!(beacon.next_delay(204_900), Some((307_200, 103)));
}
