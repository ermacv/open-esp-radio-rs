use oer_ieee80211_mac::beacon::{TimAssociationId, TimVirtualBitmap};

use super::*;

const fn at(micros: u64) -> Instant {
    Instant::from_micros(micros)
}

#[test]
fn static_storage_owns_beacon_dtim_and_next_deadline() {
    let mut storage = [0; AP_BEACON_CAPACITY];
    let ssid = WifiSsid::new(b"ap").unwrap();
    let mut beacon = ApBeacon::new(
        &mut storage,
        [2; 6],
        &ssid,
        WifiChannel::mhz20(6).unwrap(),
        100,
        2,
        SequenceNumber::new(3).unwrap(),
        ApSecurityPolicy::Wpa2Personal,
    )
    .unwrap();
    assert!(beacon.publication_due(at(102_400)));
    assert_eq!(beacon.next_publication(), None);
    let mut bitmap = TimVirtualBitmap::<2>::try_new().unwrap();
    bitmap.set(TimAssociationId::new(1).unwrap(), true).unwrap();
    let frame = beacon
        .prepare(
            at(102_400),
            SequenceNumber::new(4).unwrap(),
            true,
            bitmap.partial(),
        )
        .unwrap();
    let (offset, count, period) = dtim(frame).unwrap();
    assert_eq!((count, period), (0, 2));
    assert_eq!(frame[offset + 4] & 1, 1);
    assert_eq!(frame[offset + 5], 0x02);
    assert_eq!(&frame[22..24], &0x0040_u16.to_le_bytes());
    assert_eq!(beacon.next_publication(), Some(at(204_800)));
    assert!(!beacon.publication_due(at(204_799)));
    assert!(beacon.publication_due(at(204_800)));
    assert!(beacon.publication_due(at(204_801)));
    assert_eq!(
        beacon.publication_lateness(at(204_800)),
        (0, Duration::from_micros(0))
    );
    assert_eq!(
        beacon.publication_lateness(at(204_801)),
        (0, Duration::from_micros(1))
    );
    assert_eq!(
        beacon.publication_lateness(at(307_200)),
        (1, Duration::from_micros(102_400))
    );
    assert_eq!(
        beacon.publication_lateness(at(307_201)),
        (1, Duration::from_micros(102_401))
    );
}

#[test]
fn late_publication_does_not_move_the_absolute_tbtt_schedule() {
    let mut storage = [0; AP_BEACON_CAPACITY];
    let ssid = WifiSsid::new(b"ap").unwrap();
    let mut beacon = ApBeacon::new(
        &mut storage,
        [2; 6],
        &ssid,
        WifiChannel::mhz20(6).unwrap(),
        100,
        2,
        SequenceNumber::new(3).unwrap(),
        ApSecurityPolicy::Wpa2Personal,
    )
    .unwrap();

    let bitmap = TimVirtualBitmap::<2>::try_new().unwrap();
    beacon
        .prepare(
            at(102_400),
            SequenceNumber::new(4).unwrap(),
            false,
            bitmap.partial(),
        )
        .unwrap();
    assert_eq!(beacon.next_publication(), Some(at(204_800)));
    assert_eq!(
        beacon.publication_lateness(at(204_900)),
        (0, Duration::from_micros(100))
    );

    beacon
        .prepare(
            at(204_900),
            SequenceNumber::new(5).unwrap(),
            false,
            bitmap.partial(),
        )
        .unwrap();
    assert_eq!(beacon.next_publication(), Some(at(307_200)));
}

#[test]
fn the_tbtt_schedule_survives_the_u32_microsecond_boundary() {
    // About 71.6 minutes after boot the executor clock passes 2^32 µs; the
    // schedule keeps counting on the u64 axis instead of a wrapping tick.
    let mut storage = [0; AP_BEACON_CAPACITY];
    let ssid = WifiSsid::new(b"ap").unwrap();
    let mut beacon = ApBeacon::new(
        &mut storage,
        [2; 6],
        &ssid,
        WifiChannel::mhz20(6).unwrap(),
        100,
        2,
        SequenceNumber::new(3).unwrap(),
        ApSecurityPolicy::Wpa2Personal,
    )
    .unwrap();
    let bitmap = TimVirtualBitmap::<2>::try_new().unwrap();
    let first = (1_u64 << 32) - 50_000;
    beacon
        .prepare(
            at(first),
            SequenceNumber::new(4).unwrap(),
            false,
            bitmap.partial(),
        )
        .unwrap();
    let next = first + 102_400;
    assert!(next > 1 << 32);
    assert_eq!(beacon.next_publication(), Some(at(next)));
    assert!(!beacon.publication_due(at(1 << 32)));
    assert!(beacon.publication_due(at(next)));
    assert_eq!(
        beacon.publication_lateness(at(next + 5)),
        (0, Duration::from_micros(5))
    );
}
