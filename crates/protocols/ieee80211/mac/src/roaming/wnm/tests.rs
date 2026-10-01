use super::*;

#[test]
fn idle_and_sleep_units_are_distinct_and_reserved_bits_are_preserved() {
    let idle = BssMaxIdle::parse(Elements::parse(&[90, 3, 2, 0, 0x81]).unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(idle.period, 2);
    assert!(idle.protected_keep_alive());
    assert_eq!(idle.options, 0x81);
    assert!(BssMaxIdle::parse(Elements::parse(&[90, 3, 0, 0, 0]).unwrap()).is_err());
    let wire = [
        10, 16, 7, 93, 4, 0, 0, 9, 0, 91, 11, 8, 0, 1, 7, 14, 5, 0, 3, 0, 0, 0, 221, 2, 6, 7,
    ];
    let request = WnmSleepRequest::parse(&wire).unwrap();
    assert_eq!(request.sleep().unwrap().interval_dtim, 9);
    assert_eq!(
        super::super::TfsRequest::parse(request.elements.unique(91).unwrap().unwrap())
            .unwrap()
            .id,
        8
    );
    let mut out = [0; 32];
    let len = request.encode(&mut out).unwrap();
    assert_eq!(&out[..len], &wire);
}

fn keys() -> std::vec::Vec<u8> {
    let mut out = std::vec![0, 27, 1, 0, 16, 1, 2, 3, 4, 5, 6, 0, 0];
    out.extend_from_slice(&[0xab; 16]);
    out.extend_from_slice(&[1, 24, 4, 0, 6, 5, 4, 3, 2, 1]);
    out.extend_from_slice(&[0xcd; 16]);
    out
}
#[test]
fn complete_key_updates_round_trip_and_debug_never_prints_key_bytes() {
    let keys = keys();
    let data = SleepKeyData::parse(&keys).unwrap();
    let mut iter = data.keys();
    let SleepKey::Gtk { key_info, rsc, key } = iter.next().unwrap() else {
        panic!()
    };
    assert_eq!(key_info, 1);
    assert_eq!(rsc, [1, 2, 3, 4, 5, 6, 0, 0]);
    assert_eq!(key, [0xab; 16]);
    let SleepKey::Integrity {
        key_id,
        packet_number,
        key,
        ..
    } = iter.next().unwrap()
    else {
        panic!()
    };
    assert_eq!(key_id, 4);
    assert_eq!(packet_number, [6, 5, 4, 3, 2, 1]);
    assert_eq!(key, [0xcd; 16]);
    assert!(!std::format!("{data:?}").contains("171"));
    for key in data.keys() {
        assert!(!std::format!("{key:?}").contains("205"));
    }
    let response = WnmSleepResponse {
        dialog_token: 8,
        keys: data,
        elements: Elements::parse(&[93, 4, 1, 1, 0, 0, 92, 4, 1, 2, 0, 9]).unwrap(),
    };
    let mut out = [0; 128];
    let len = response.encode(&mut out).unwrap();
    assert_eq!(&out[..5], &[10, 17, 8, 55, 0]);
    assert_eq!(WnmSleepResponse::parse(&out[..len]).unwrap(), response);
}
#[test]
fn malformed_keys_duplicate_sleep_elements_and_short_output_change_nothing() {
    let mut bytes = keys();
    bytes[4] = 15;
    assert!(SleepKeyData::parse(&bytes).is_err());
    let duplicate = [10, 16, 1, 93, 4, 0, 0, 0, 0, 93, 4, 0, 0, 0, 0];
    assert!(WnmSleepRequest::parse(&duplicate).is_err());
    assert!(WnmSleepResponse::parse(&[10, 17, 1, 10, 0, 93, 4, 1, 0, 0, 0]).is_err());
    let response = WnmSleepResponse {
        dialog_token: 1,
        keys: SleepKeyData::EMPTY,
        elements: Elements::parse(&[93, 4, 1, 0, 0, 0]).unwrap(),
    };
    let mut out = [0xaa; 10];
    assert!(response.encode(&mut out).is_err());
    assert_eq!(out, [0xaa; 10]);
}
