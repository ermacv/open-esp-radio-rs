use super::*;

#[test]
fn esp32s31_wifi_osi_layout_is_versioned_and_bounded() {
    let table = WIFI_OSI_V9.spec();
    assert_eq!(table.version, 9);
    assert_eq!(table.magic, 0xdead_beaf);
    assert_eq!(table.size, 0x200);
    assert_eq!(table.magic_offset, table.size - 4);
    assert!(slots(WIFI_OSI_V9).all(|slot| slot.spec().offset < table.magic_offset));
}

#[test]
fn rand_and_random_are_distinct_slots() {
    assert_eq!(WIFI_OSI_V9.function_at(0x0bc), Some(RAND));
    assert_eq!(WIFI_OSI_V9.function_at(0x144), Some(RANDOM));
    assert!(WIFI_OSI_V9.function_at(0x0c0).is_none());
}
