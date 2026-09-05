use super::{
    BluetoothDtmPayloadLength, BluetoothDtmPayloadPattern, BluetoothDtmPayloadPatternError,
    BluetoothDtmPayloadPreparationError,
};

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[test]
fn every_hci_pattern_selector_roundtrips_and_bounds_the_domain() {
    let patterns = [
        BluetoothDtmPayloadPattern::Prbs9,
        BluetoothDtmPayloadPattern::Repeated11110000,
        BluetoothDtmPayloadPattern::Repeated10101010,
        BluetoothDtmPayloadPattern::Prbs15,
        BluetoothDtmPayloadPattern::RepeatedAllOnes,
        BluetoothDtmPayloadPattern::RepeatedAllZeros,
        BluetoothDtmPayloadPattern::Repeated00001111,
        BluetoothDtmPayloadPattern::Repeated01010101,
    ];

    for (selector, pattern) in patterns.into_iter().enumerate() {
        assert_eq!(pattern.hci_selector(), selector as u8);
        assert_eq!(
            BluetoothDtmPayloadPattern::from_hci_selector(selector as u8),
            Ok(pattern)
        );
    }
    assert_eq!(
        BluetoothDtmPayloadPattern::from_hci_selector(8),
        Err(BluetoothDtmPayloadPatternError::UnsupportedHciSelector)
    );
}

#[test]
fn repeated_patterns_match_all_complete_vendor_branches() {
    let cases = [
        (BluetoothDtmPayloadPattern::Repeated11110000, 0x0f),
        (BluetoothDtmPayloadPattern::Repeated10101010, 0x55),
        (BluetoothDtmPayloadPattern::RepeatedAllOnes, 0xff),
        (BluetoothDtmPayloadPattern::RepeatedAllZeros, 0x00),
        (BluetoothDtmPayloadPattern::Repeated00001111, 0xf0),
        (BluetoothDtmPayloadPattern::Repeated01010101, 0xaa),
    ];

    for (pattern, expected) in cases {
        let mut storage = [0xa5; 255];
        let prepared = pattern
            .prepare(BluetoothDtmPayloadLength::from_hci_image(255), &mut storage)
            .expect("full HCI payload fits");
        assert_eq!(prepared.bytes(), &[expected; 255]);
    }
}

#[test]
fn prbs9_matches_the_complete_cross_revision_table_fingerprint() {
    let mut storage = [0; 255];
    let prepared = BluetoothDtmPayloadPattern::Prbs9
        .prepare(BluetoothDtmPayloadLength::from_hci_image(255), &mut storage)
        .expect("full PRBS9 payload fits");

    assert_eq!(
        &prepared.bytes()[..16],
        &[
            0xff, 0xc1, 0xfb, 0xe8, 0x4c, 0x90, 0x72, 0x8b, 0xe7, 0xb3, 0x51, 0x89, 0x63, 0xab,
            0x23, 0x23
        ]
    );
    assert_eq!(fnv1a64(prepared.bytes()), 0x94db_648c_b178_dce3);
}

#[test]
fn prbs15_matches_the_complete_cross_revision_table_fingerprint() {
    let mut storage = [0; 255];
    let prepared = BluetoothDtmPayloadPattern::Prbs15
        .prepare(BluetoothDtmPayloadLength::from_hci_image(255), &mut storage)
        .expect("full PRBS15 payload fits");

    assert_eq!(
        &prepared.bytes()[..16],
        &[
            0xff, 0x7f, 0xf0, 0x3e, 0x3a, 0x13, 0xa4, 0xdc, 0xe2, 0xf9, 0x6c, 0x54, 0xe2, 0xd8,
            0xea, 0xc8
        ]
    );
    assert_eq!(fnv1a64(prepared.bytes()), 0x4655_41b8_492c_b9ba);
}

#[test]
fn preparation_is_fail_closed_and_returns_the_whole_storage() {
    let mut short = [0xa5; 3];
    assert!(matches!(
        BluetoothDtmPayloadPattern::RepeatedAllZeros
            .prepare(BluetoothDtmPayloadLength::from_hci_image(4), &mut short,),
        Err(BluetoothDtmPayloadPreparationError::StorageTooShort)
    ));
    assert_eq!(short, [0xa5; 3]);

    let mut storage = [0xa5; 8];
    let prepared = BluetoothDtmPayloadPattern::RepeatedAllZeros
        .prepare(BluetoothDtmPayloadLength::from_hci_image(3), &mut storage)
        .expect("prefix fits");
    assert_eq!(
        prepared.pattern(),
        BluetoothDtmPayloadPattern::RepeatedAllZeros
    );
    assert_eq!(prepared.length().hci_image(), 3);
    assert_eq!(prepared.bytes(), [0, 0, 0]);
    assert_eq!(prepared.release(), [0, 0, 0, 0xa5, 0xa5, 0xa5, 0xa5, 0xa5]);
}

#[test]
fn zero_length_preparation_is_defined_for_every_pattern() {
    for selector in 0..=7 {
        let pattern = BluetoothDtmPayloadPattern::from_hci_selector(selector)
            .expect("selector belongs to the complete domain");
        let mut storage = [];
        let prepared = pattern
            .prepare(BluetoothDtmPayloadLength::from_hci_image(0), &mut storage)
            .expect("zero bytes require no storage");
        assert!(prepared.bytes().is_empty());
    }
}
