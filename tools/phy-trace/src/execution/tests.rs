use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::Path,
};

use super::*;
use crate::MmioRegisterMap;

fn tiny_image(bytes: Vec<u8>, memory_size: u32) -> ExecutableImage {
    ExecutableImage {
        segments: vec![Segment {
            address: 0x1000,
            bytes,
            memory_size,
            writable: true,
        }],
        symbols_by_name: HashMap::from([("test".to_owned(), 0x1000)]),
        symbols_by_address: BTreeMap::from([(0x1000, "test".to_owned())]),
        call_trampoline_addresses: BTreeSet::new(),
        relocated_calls_by_address: BTreeMap::new(),
        global_pointer: None,
    }
}

fn empty_svd() -> MmioRegisterMap {
    MmioRegisterMap {
        registers: Vec::new(),
        windows: Vec::new(),
    }
}

fn tail_relocation_image(target: Option<u32>) -> ExecutableImage {
    let mut symbols_by_name = HashMap::from([("wrapper".to_owned(), 0x1000)]);
    let mut symbols_by_address = BTreeMap::from([(0x1000, "wrapper".to_owned())]);
    let mut segments = vec![Segment {
        address: 0x1000,
        bytes: vec![
            0x17, 0x03, 0x00, 0x00, // auipc t1, 0
            0x67, 0x00, 0x03, 0x00, // jalr zero, 0(t1)
            0x63, 0x00, 0x00, 0x00, // beq zero, zero, 0 (must be unreachable)
        ],
        memory_size: 12,
        writable: true,
    }];
    if let Some(target) = target {
        symbols_by_name.insert("callee".to_owned(), target);
        symbols_by_address.insert(target, "callee".to_owned());
        segments.push(Segment {
            address: target,
            bytes: vec![0x67, 0x80, 0x00, 0x00], // ret
            memory_size: 4,
            writable: true,
        });
    }
    ExecutableImage {
        segments,
        symbols_by_name,
        symbols_by_address,
        call_trampoline_addresses: BTreeSet::new(),
        relocated_calls_by_address: BTreeMap::from([(
            0x1000,
            RelocatedCall {
                name: "callee".to_owned(),
                target: None,
            },
        )]),
        global_pointer: None,
    }
}

fn oracle() -> Option<std::path::PathBuf> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("_oracles/esp32s31_rev0_rom.elf");
    path.exists().then_some(path)
}

#[test]
fn executes_frequency_band_tail_call_and_records_both_mmio_updates() {
    let Some(oracle) = oracle() else {
        eprintln!("private ROM fixture is not installed; integration test skipped");
        return;
    };
    let image = ExecutableImage::load(&oracle).unwrap();
    let svd = MmioRegisterMap::load(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("svd/esp32s31-radio.svd"),
    )
    .unwrap();
    let mut scenario = Scenario {
        arguments: vec![1],
        ..Scenario::default()
    };
    scenario.mmio_initial.insert(0x2010_7030, u32::MAX);
    scenario.mmio_initial.insert(0x2010_7ce4, 0);
    let result = execute(&image, &svd, "phy_freq_band_reg_set", scenario).unwrap();
    assert_eq!(result.events.len(), 4);
    assert_eq!(
        result.events[1],
        ExecutionEvent::Write {
            width: 32,
            address: 0x2010_7030,
            register: "PHY_AGC_ORACLE.AGC_ANTENNA_CONTROL".to_owned(),
            value: !(1 << 5),
        }
    );
    assert_eq!(
        result.events[3],
        ExecutionEvent::Write {
            width: 32,
            address: 0x2010_7ce4,
            register: "PHY_FREQUENCY_CHANNEL_ORACLE.CHANNEL_CBW_CONTROL_1".to_owned(),
            value: 1 << 5,
        }
    );
    assert!(result.calls.contains("phy_vht_support"));
}

#[test]
fn top_level_tail_delay_finishes_at_the_return_sentinel() {
    let Some(oracle) = oracle() else {
        eprintln!("private ROM fixture is not installed; integration test skipped");
        return;
    };
    let image = ExecutableImage::load(&oracle).unwrap();
    let svd = MmioRegisterMap::load(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("svd/esp32s31-radio.svd"),
    )
    .unwrap();
    let mut scenario = Scenario::default();
    scenario.mmio_initial.insert(0x2010_001c, 0);
    let result = execute(&image, &svd, "phy_dis_hw_set_freq", scenario).unwrap();
    assert!(matches!(
        result.events.last(),
        Some(ExecutionEvent::DelayMicros(2))
    ));
}

#[test]
fn static_branch_inventory_includes_reachable_child_control_flow() {
    let Some(oracle) = oracle() else {
        eprintln!("private ROM fixture is not installed; integration test skipped");
        return;
    };
    let image = ExecutableImage::load(&oracle).unwrap();
    assert!(
        !image
            .coverage_inventory("phy_bb_bss_cbw40")
            .unwrap()
            .branch_sites
            .is_empty()
    );
}

#[test]
fn branch_inventory_removes_child_outcomes_infeasible_from_fixed_arguments() {
    let Some(oracle) = oracle() else {
        eprintln!("private ROM fixture is not installed; integration test skipped");
        return;
    };
    let image = ExecutableImage::load(&oracle).unwrap();
    let wrapper = image.coverage_inventory("phy_pbus_debugmode").unwrap();
    assert_eq!(wrapper.branch_outcomes.len(), 1);
    assert!(wrapper.branch_outcomes.iter().all(|(_, taken)| !taken));

    let child = image.coverage_inventory("phy_pbus_force_mode").unwrap();
    assert!(child.branch_outcomes.iter().any(|(_, taken)| *taken));
    assert!(child.branch_outcomes.iter().any(|(_, taken)| !*taken));
}

#[test]
fn companion_symbol_resolves_external_tail_relocation_without_fallthrough() {
    let mut image = tail_relocation_image(Some(0x2000));
    image.resolve_external_relocations();
    assert_eq!(
        image.relocated_call_at(0x1000).and_then(|call| call.target),
        Some(0x2000)
    );
    let inventory = image.coverage_inventory("wrapper").unwrap();
    assert!(inventory.unresolved_edges.is_empty());
    assert!(inventory.branch_sites.is_empty());

    let svd = MmioRegisterMap {
        registers: Vec::new(),
        windows: vec![crate::Window { start: 0, end: 1 }],
    };
    let result = execute(&image, &svd, "wrapper", Scenario::default()).unwrap();
    assert!(result.calls.contains("callee"));
    assert_eq!(result.ordered_calls.len(), 1);
    assert_eq!(result.ordered_calls[0].symbol, "callee");
    assert!(result.events.is_empty());
}

#[test]
fn call_trampoline_does_not_duplicate_the_ordered_target_call() {
    let image = ExecutableImage {
        segments: vec![
            Segment {
                address: 0x1000,
                bytes: vec![
                    0x97, 0x02, 0x00, 0x00, // auipc t0, 0
                    0x67, 0x80, 0x02, 0x00, // jalr zero, 0(t0)
                ],
                memory_size: 8,
                writable: true,
            },
            Segment {
                address: 0x2000,
                bytes: [0x6f, 0x00, 0x00, 0x01]
                    .into_iter()
                    .chain([0; 12])
                    .chain([0x67, 0x80, 0x00, 0x00])
                    .collect(),
                memory_size: 20,
                writable: true,
            },
        ],
        symbols_by_name: HashMap::from([
            ("wrapper".to_owned(), 0x1000),
            ("__call_callee".to_owned(), 0x2000),
            ("callee".to_owned(), 0x2010),
        ]),
        symbols_by_address: BTreeMap::from([
            (0x1000, "wrapper".to_owned()),
            (0x2000, "__call_callee".to_owned()),
            (0x2010, "callee".to_owned()),
        ]),
        call_trampoline_addresses: BTreeSet::from([0x2000]),
        relocated_calls_by_address: BTreeMap::from([(
            0x1000,
            RelocatedCall {
                name: "callee".to_owned(),
                target: Some(0x2000),
            },
        )]),
        global_pointer: None,
    };
    let result = execute(&image, &empty_svd(), "wrapper", Scenario::default()).unwrap();
    assert_eq!(result.ordered_calls.len(), 1);
    assert_eq!(result.ordered_calls[0].symbol, "callee");
}

#[test]
fn ordered_control_flow_retains_call_multiplicity_and_loop_iterations() {
    let calls = ExecutableImage {
        segments: vec![Segment {
            address: 0x1000,
            bytes: vec![
                0x13, 0x84, 0x00, 0x00, // addi s0, ra, 0
                0xef, 0x00, 0x00, 0x01, // jal ra, 16
                0xef, 0x00, 0xc0, 0x00, // jal ra, 12
                0x93, 0x00, 0x04, 0x00, // addi ra, s0, 0
                0x67, 0x80, 0x00, 0x00, // ret
                0x67, 0x80, 0x00, 0x00, // callee: ret
            ],
            memory_size: 24,
            writable: true,
        }],
        symbols_by_name: HashMap::from([
            ("wrapper".to_owned(), 0x1000),
            ("callee".to_owned(), 0x1014),
        ]),
        symbols_by_address: BTreeMap::from([
            (0x1000, "wrapper".to_owned()),
            (0x1014, "callee".to_owned()),
        ]),
        call_trampoline_addresses: BTreeSet::new(),
        relocated_calls_by_address: BTreeMap::new(),
        global_pointer: None,
    };
    let result = execute(&calls, &empty_svd(), "wrapper", Scenario::default()).unwrap();
    assert_eq!(result.calls.len(), 1);
    assert_eq!(result.ordered_calls.len(), 2);
    assert!(
        result
            .ordered_calls
            .iter()
            .all(|call| call.symbol == "callee")
    );

    let loop_image = tiny_image(
        vec![
            0x13, 0x05, 0x30, 0x00, // addi a0, zero, 3
            0x13, 0x05, 0xf5, 0xff, // addi a0, a0, -1
            0xe3, 0x1e, 0x05, 0xfe, // bne a0, zero, -4
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        16,
    );
    let result = execute(&loop_image, &empty_svd(), "test", Scenario::default()).unwrap();
    assert_eq!(result.branches.len(), 2);
    assert_eq!(
        result.ordered_branches,
        vec![(0x1008, true), (0x1008, true), (0x1008, false)]
    );
}

#[test]
fn ordered_timeline_retains_intermediate_ram_values() {
    let image = tiny_image(vec![0x67, 0x80, 0x00, 0x00], 4);
    let address = 0x3fff_0000;
    let mut scenario = Scenario::default();
    scenario
        .memory_initial
        .extend((0..4).map(|offset| (address + offset, 0)));
    let svd = empty_svd();
    let mut machine = Machine::new(&image, &svd, 0x1000, scenario);

    machine.write(address, 32, 0x1122_3344).unwrap();
    assert_eq!(machine.read(address, 32).unwrap(), 0x1122_3344);
    machine.write(address, 32, 0x5566_7788).unwrap();
    assert_eq!(machine.read(address, 32).unwrap(), 0x5566_7788);

    assert_eq!(
        machine.timeline,
        vec![
            ExecutionTimelineEvent::RamWrite {
                width: 32,
                address,
                value: 0x1122_3344,
            },
            ExecutionTimelineEvent::RamRead {
                width: 32,
                address,
                value: 0x1122_3344,
            },
            ExecutionTimelineEvent::RamWrite {
                width: 32,
                address,
                value: 0x5566_7788,
            },
            ExecutionTimelineEvent::RamRead {
                width: 32,
                address,
                value: 0x5566_7788,
            },
        ]
    );
}

#[test]
fn execution_session_retains_elf_and_declared_ram_but_not_stack() {
    let mut image = tiny_image(
        vec![
            0x03, 0xa5, 0x05, 0x00, // lw a0, 0(a1)
            0x13, 0x05, 0x15, 0x00, // addi a0, a0, 1
            0x23, 0xa0, 0xa5, 0x00, // sw a0, 0(a1)
            0x67, 0x80, 0x00, 0x00, // ret
            0, 0, 0, 0, // ELF-backed mutable word
            0x13, 0x01, 0xc1, 0xff, // stack_writer: addi sp, sp, -4
            0x23, 0x20, 0xa1, 0x00, // stack_writer: sw a0, 0(sp)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        32,
    );
    image
        .symbols_by_name
        .insert("stack_writer".to_owned(), 0x1014);
    image
        .symbols_by_address
        .insert(0x1014, "stack_writer".to_owned());
    let svd = empty_svd();
    let mut session = ExecutionSession::default();

    for expected in [1, 2] {
        let scenario = Scenario {
            arguments: vec![0, 0x1010],
            ..Scenario::default()
        };
        let result = session.execute(&image, &svd, "test", scenario).unwrap();
        assert_eq!(result.return_value, expected);
    }
    assert_eq!(session.byte(&image, 0x1010), Some(2));

    let external = 0x2000;
    let first = Scenario {
        arguments: vec![0, external],
        memory_initial: (0..4).map(|offset| (external + offset, 0)).collect(),
        persistent_memory: vec![MemoryRange {
            start: external,
            length: 4,
        }],
        ..Scenario::default()
    };
    assert_eq!(
        session
            .execute(&image, &svd, "test", first)
            .unwrap()
            .return_value,
        1
    );
    let second = Scenario {
        arguments: vec![0, external],
        ..Scenario::default()
    };
    assert_eq!(
        session
            .execute(&image, &svd, "test", second)
            .unwrap()
            .return_value,
        2
    );

    let stack = session
        .execute(
            &image,
            &svd,
            "stack_writer",
            Scenario {
                arguments: vec![0xdead_beef],
                ..Scenario::default()
            },
        )
        .unwrap();
    assert!(
        stack
            .persistent_memory
            .keys()
            .all(|address| !execution_stack_contains(*address))
    );
}

#[test]
fn execution_session_invalidates_externally_mutable_ram_between_calls() {
    let image = tiny_image(
        vec![
            0x03, 0xa5, 0x05, 0x00, // lw a0, 0(a1)
            0x13, 0x05, 0x15, 0x00, // addi a0, a0, 1
            0x23, 0xa0, 0xa5, 0x00, // sw a0, 0(a1)
            0x67, 0x80, 0x00, 0x00, // ret
            0, 0, 0, 0,
        ],
        20,
    );
    let range = MemoryRange {
        start: 0x1010,
        length: 4,
    };
    let ownership = MemoryOwnership {
        range,
        owner: MemoryOwner::SharedUnknown,
    };
    let mut session = ExecutionSession::default();
    let unseeded = Scenario {
        arguments: vec![0, range.start],
        memory_ownership: vec![ownership],
        ..Scenario::default()
    };
    let error = session
        .execute(&image, &empty_svd(), "test", unseeded)
        .unwrap_err();
    assert!(error.to_string().contains("externally mutable RAM"));

    let seeded = Scenario {
        arguments: vec![0, range.start],
        memory_initial: (0..4)
            .map(|offset| (range.start + offset, u8::from(offset == 0) * 9))
            .collect(),
        ..Scenario::default()
    };
    assert_eq!(
        session
            .execute(&image, &empty_svd(), "test", seeded)
            .unwrap()
            .return_value,
        10
    );
    assert_eq!(session.byte(&image, range.start), None);
}

#[test]
fn execution_session_distinguishes_cold_and_warm_reset() {
    let image = tiny_image(
        vec![
            0x03, 0xa5, 0x05, 0x00, // lw a0, 0(a1)
            0x13, 0x05, 0x15, 0x00, // addi a0, a0, 1
            0x23, 0xa0, 0xa5, 0x00, // sw a0, 0(a1)
            0x67, 0x80, 0x00, 0x00, // ret
            0, 0, 0, 0,
        ],
        20,
    );
    let mut session = ExecutionSession::default();
    for (reset_policy, expected) in [
        (ResetPolicy::Continue, 1),
        (ResetPolicy::Continue, 2),
        (ResetPolicy::ColdBoot, 1),
    ] {
        let result = session
            .execute(
                &image,
                &empty_svd(),
                "test",
                Scenario {
                    arguments: vec![0, 0x1010],
                    reset_policy,
                    ..Scenario::default()
                },
            )
            .unwrap();
        assert_eq!(result.return_value, expected);
    }

    let external = 0x2000;
    let first = Scenario {
        arguments: vec![0, external],
        memory_initial: (0..4).map(|offset| (external + offset, 0)).collect(),
        persistent_memory: vec![MemoryRange {
            start: external,
            length: 4,
        }],
        ..Scenario::default()
    };
    assert_eq!(
        session
            .execute(&image, &empty_svd(), "test", first)
            .unwrap()
            .return_value,
        1
    );
    let warm = Scenario {
        arguments: vec![0, external],
        reset_policy: ResetPolicy::WarmReset,
        ..Scenario::default()
    };
    assert_eq!(
        session
            .execute(&image, &empty_svd(), "test", warm)
            .unwrap()
            .return_value,
        2
    );
    assert_eq!(session.byte(&image, 0x1010), Some(0));
}

#[test]
fn ownership_conflicts_and_immutable_writes_fail_closed() {
    let image = tiny_image(
        vec![
            0x23, 0xa0, 0xa5, 0x00, // sw a0, 0(a1)
            0x67, 0x80, 0x00, 0x00, // ret
            0, 0, 0, 0,
        ],
        12,
    );
    let range = MemoryRange {
        start: 0x1008,
        length: 4,
    };
    let immutable = Scenario {
        arguments: vec![1, range.start],
        memory_ownership: vec![MemoryOwnership {
            range,
            owner: MemoryOwner::Immutable,
        }],
        ..Scenario::default()
    };
    let error = execute(&image, &empty_svd(), "test", immutable).unwrap_err();
    assert!(error.to_string().contains("immutable RAM"));

    let mut session = ExecutionSession::default();
    session
        .execute(
            &image,
            &empty_svd(),
            "test",
            Scenario {
                arguments: vec![1, range.start],
                memory_ownership: vec![MemoryOwnership {
                    range,
                    owner: MemoryOwner::Cpu,
                }],
                ..Scenario::default()
            },
        )
        .unwrap();
    let conflict = session
        .execute(
            &image,
            &empty_svd(),
            "test",
            Scenario {
                arguments: vec![1, range.start],
                memory_ownership: vec![MemoryOwnership {
                    range,
                    owner: MemoryOwner::Dma,
                }],
                ..Scenario::default()
            },
        )
        .unwrap_err();
    assert!(conflict.to_string().contains("conflicting RAM ownership"));
}

#[test]
fn unresolved_external_tail_call_fails_closed() {
    let image = tail_relocation_image(None);
    let inventory = image.coverage_inventory("wrapper").unwrap();
    assert_eq!(inventory.unresolved_edges.len(), 1);
    assert!(inventory.branch_sites.is_empty());

    let svd = MmioRegisterMap {
        registers: Vec::new(),
        windows: vec![crate::Window { start: 0, end: 1 }],
    };
    let error = execute(&image, &svd, "wrapper", Scenario::default()).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("unresolved external call callee")
    );
}

#[test]
fn poison_memory_and_unseeded_mmio_fail_closed() {
    let image = tiny_image(vec![0x67, 0x80, 0x00, 0x00], 4);
    let empty_svd = empty_svd();
    let mut machine = Machine::new(&image, &empty_svd, 0x1000, Scenario::default());
    assert!(
        machine
            .read(0x4000_0000, 32)
            .unwrap_err()
            .to_string()
            .contains("poison/unmapped")
    );

    let mmio_svd = MmioRegisterMap {
        registers: Vec::new(),
        windows: vec![crate::Window {
            start: 0x2010_0000,
            end: 0x2020_0000,
        }],
    };
    let mut machine = Machine::new(&image, &mmio_svd, 0x1000, Scenario::default());
    assert!(
        machine
            .read(0x2010_0010, 32)
            .unwrap_err()
            .to_string()
            .contains("no explicit seed or response")
    );
}

#[test]
fn mmio_write_does_not_create_a_generic_readback_value() {
    let image = tiny_image(vec![0x67, 0x80, 0x00, 0x00], 4);
    let mmio_svd = MmioRegisterMap {
        registers: Vec::new(),
        windows: vec![crate::Window {
            start: 0x2010_0000,
            end: 0x2020_0000,
        }],
    };
    let address = 0x2010_0010;

    let mut seeded = Scenario::default();
    seeded.mmio_initial.insert(address, 0x1122_3344);
    let mut machine = Machine::new(&image, &mmio_svd, 0x1000, seeded);
    machine.write(address, 32, 0xaabb_ccdd).unwrap();
    assert_eq!(machine.read(address, 32).unwrap(), 0x1122_3344);

    let mut machine = Machine::new(&image, &mmio_svd, 0x1000, Scenario::default());
    machine.write(address, 32, 0xaabb_ccdd).unwrap();
    assert!(machine.read(address, 32).is_err());
}

#[test]
fn bss_tail_is_known_zero() {
    let image = tiny_image(vec![0x67, 0x80, 0x00, 0x00], 8);
    assert_eq!(image.byte(0x1004), Some(0));
    assert_eq!(image.byte(0x1008), None);
}

#[test]
fn writes_to_read_only_elf_memory_fail_closed() {
    let mut image = tiny_image(vec![0x67, 0x80, 0x00, 0x00], 8);
    image.segments[0].writable = false;
    let svd = empty_svd();
    let mut machine = Machine::new(&image, &svd, 0x1000, Scenario::default());

    let error = machine.write(0x1004, 8, 0x5a).unwrap_err();
    assert!(error.to_string().contains("read-only ELF memory"));
    assert!(machine.persistent_memory().is_empty());
}

#[test]
fn execution_rejects_extra_arguments_and_unconsumed_mmio_reads() {
    let image = tiny_image(vec![0x67, 0x80, 0x00, 0x00], 4);
    let too_many = Scenario {
        arguments: vec![0; 9],
        ..Scenario::default()
    };
    assert!(
        execute(&image, &empty_svd(), "test", too_many)
            .unwrap_err()
            .to_string()
            .contains("stack arguments are not implemented")
    );

    let mut unconsumed = Scenario::default();
    unconsumed
        .mmio_reads
        .entry(0x2010_0010)
        .or_default()
        .push_back(1);
    assert!(
        execute(&image, &empty_svd(), "test", unconsumed)
            .unwrap_err()
            .to_string()
            .contains("unconsumed MMIO read responses")
    );
}

#[test]
fn fence_is_an_ordered_execution_event() {
    let image = tiny_image(
        vec![
            0x0f, 0x00, 0x30, 0x03, // fence rw, rw
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        8,
    );
    let result = execute(&image, &empty_svd(), "test", Scenario::default()).unwrap();
    assert_eq!(
        result.events,
        vec![ExecutionEvent::Fence {
            fm: 0,
            predecessor: 3,
            successor: 3,
        }]
    );
}

#[test]
fn reports_only_observed_memory_mutations() {
    let image = tiny_image(vec![0x67, 0x80, 0x00, 0x00], 4);
    let svd = MmioRegisterMap::load(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("svd/esp32s31-radio.svd"),
    )
    .unwrap();
    let address = 0x3fff_0000;
    let mut scenario = Scenario::default();
    scenario.memory_initial.insert(address, 0xaa);
    scenario.memory_initial.insert(address + 1, 0);
    scenario.observed_memory.push(MemoryRange {
        start: address,
        length: 2,
    });
    let mut machine = Machine::new(&image, &svd, 0, scenario);
    machine.write(address, 16, 0x55aa).unwrap();
    machine.write(address + 8, 8, 0xff).unwrap();
    assert_eq!(
        machine.memory_changes().unwrap(),
        vec![MemoryChange {
            address: address + 1,
            before: 0,
            after: 0x55,
        }]
    );
}

#[test]
fn observed_memory_alias_reports_normalized_addresses() {
    let image = tiny_image(vec![0x67, 0x80, 0x00, 0x00], 4);
    let svd = MmioRegisterMap::load(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("svd/esp32s31-radio.svd"),
    )
    .unwrap();
    let actual = 0x3fff_0120;
    let mut scenario = Scenario::default();
    scenario.memory_initial.insert(actual, 0);
    scenario.memory_initial.insert(actual + 1, 0);
    scenario.memory_aliases.push(MemoryAlias {
        start: actual,
        length: 2,
        comparison_start: 0,
    });
    let mut machine = Machine::new(&image, &svd, 0, scenario);
    machine.write(actual + 1, 8, 0x5a).unwrap();
    assert_eq!(
        machine.memory_changes().unwrap(),
        vec![MemoryChange {
            address: 1,
            before: 0,
            after: 0x5a,
        }]
    );
}
