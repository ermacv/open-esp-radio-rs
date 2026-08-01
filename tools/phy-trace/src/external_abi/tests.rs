use super::*;

#[test]
fn esp32s31_wifi_osi_layout_is_versioned_and_bounded() {
    let table = table_spec(Table::Esp32s31WifiOsiV9);
    assert_eq!(table.version, 9);
    assert_eq!(table.magic, 0xdead_beaf);
    assert_eq!(table.size, 0x200);
    assert_eq!(table.magic_offset, table.size - 4);
    assert!(slots(Table::Esp32s31WifiOsiV9).all(|slot| slot.offset < table.magic_offset));
}

#[test]
fn rand_and_random_are_distinct_slots() {
    let table = Table::Esp32s31WifiOsiV9;
    assert_eq!(slot(table, 0x0bc).unwrap().function, Function::Rand);
    assert_eq!(slot(table, 0x144).unwrap().function, Function::Random);
    assert!(slot(table, 0x0c0).is_none());
}
