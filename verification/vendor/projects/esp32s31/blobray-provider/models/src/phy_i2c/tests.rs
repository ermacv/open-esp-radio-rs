use super::*;

#[test]
fn scripted_bank_keeps_blocks_distinct_and_measurements_read_only() {
    let bank = RegisterBank {
        bbpll_control: None,
        registers: BTreeMap::from([((0x62, 4), 0x55), ((0x63, 4), 0xaa)]),
        reads: BTreeMap::from([((0x62, 17), vec![0x31, 0x32])]),
        busy_reads: 0,
    };
    let mut port = bank.instantiate().unwrap();
    for (command, expected) in [
        (0x0400_0462, 0x55),
        (0x0400_0463, 0xaa),
        (0x0400_1162, 0x31),
        (0x0400_1162, 0x32),
    ] {
        port.write(PORT_BASE, 32, command).unwrap();
        assert_eq!((port.read(PORT_BASE, 32).unwrap() >> 16) as u8, expected);
    }
    assert!(port.finish().unwrap().complete);
    assert!(port.write(PORT_BASE, 32, 0x0400_1162).is_err());
    assert!(port.write(PORT_BASE, 32, 0x055a_1162).is_err());
    let mut fresh = bank.instantiate().unwrap();
    assert!(!fresh.finish().unwrap().complete);
}

#[test]
fn ambiguous_register_ownership_is_rejected() {
    let bank = RegisterBank {
        bbpll_control: None,
        registers: BTreeMap::from([((0x62, 17), 0)]),
        reads: BTreeMap::from([((0x62, 17), vec![1])]),
        busy_reads: 0,
    };
    assert!(bank.instantiate().is_err());
}

#[test]
fn status_is_consumed_by_command_not_by_busy_polls() {
    let mut port = Rfpll::new(100, vec![1, 2], 1)
        .unwrap()
        .instantiate()
        .unwrap();
    port.write(PORT_BASE, 32, 0x0400_0c62).unwrap();
    assert!(port.write(PORT_BASE, 32, 0x0400_0c62).is_err());
    assert_ne!(port.read(PORT_BASE, 32).unwrap() & 0x0200_0000, 0);
    let ready = port.read(PORT_BASE, 32).unwrap();
    assert_eq!(ready, port.read(PORT_BASE, 32).unwrap());
    assert_eq!((ready >> 18) & 3, 1);
    assert!(!port.finish().unwrap().complete);
    port.write(PORT_BASE, 32, 0x0400_0c62).unwrap();
    port.read(PORT_BASE, 32).unwrap();
    assert_eq!((port.read(PORT_BASE, 32).unwrap() >> 18) & 3, 2);
    assert!(port.finish().unwrap().complete);
    assert!(port.write(PORT_BASE, 32, 0x0400_0c62).is_err());
}

#[test]
fn hosts_share_the_analog_bank_but_have_independent_completion() {
    let model = Rfpll::new(100, vec![], 0).unwrap();
    let mut port = model.instantiate().unwrap();
    port.write(PORT_BASE, 32, 0x055a_0162).unwrap();
    port.write(PORT_BASE + 4, 32, 0x0400_0162).unwrap();
    assert_eq!((port.read(PORT_BASE + 4, 32).unwrap() >> 16) as u8, 0x5a);
    let mut fresh = model.instantiate().unwrap();
    fresh.write(PORT_BASE, 32, 0x0400_0162).unwrap();
    assert_eq!((fresh.read(PORT_BASE, 32).unwrap() >> 16) as u8, 100);
}

#[test]
fn invalid_commands_and_unmodeled_accesses_fail_closed() {
    assert!(Rfpll::new(512, vec![], 0).is_err());
    assert!(Rfpll::new(100, vec![4], 0).is_err());
    let mut port = Rfpll::new(100, vec![], 0).unwrap().instantiate().unwrap();
    assert!(port.write(PORT_BASE, 32, 0x0400_0563).is_err());
    assert!(port.write(PORT_BASE, 32, 0x0400_ff62).is_err());
    assert!(port.read(PORT_BASE, 8).is_err());
    assert!(port.read(PORT_BASE + 8, 32).is_err());
}

#[test]
fn bbpll_control_is_independent_from_i2c_commands_and_requires_a_seed() {
    let mut bank = RegisterBank {
        bbpll_control: Some(0x5a5a5a5a),
        registers: BTreeMap::from([((0x69, 6), 5)]),
        reads: BTreeMap::new(),
        busy_reads: 1,
    };
    let mut port = bank.instantiate().unwrap();
    port.write(PORT_BASE, 32, 0x04000669).unwrap();
    port.write(PORT_BASE + 0x18, 32, 0xa5a5a5a5).unwrap();
    assert_eq!(port.read(PORT_BASE + 0x18, 32).unwrap(), 0xa5a5a5a5);
    assert_ne!(port.read(PORT_BASE, 32).unwrap() & 0x02000000, 0);
    assert_eq!(port.read(PORT_BASE, 32).unwrap() & 0x00ff0000, 0x00050000);
    assert!(port.finish().unwrap().complete);
    bank.bbpll_control = None;
    let mut port = bank.instantiate().unwrap();
    assert!(port.read(PORT_BASE + 0x18, 32).is_err());
    assert!(port.write(PORT_BASE + 0x18, 32, 0).is_err());
}
