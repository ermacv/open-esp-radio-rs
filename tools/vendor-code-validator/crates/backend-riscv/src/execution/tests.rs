use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use rv_asm::AmoOp;

use super::image::UnresolvedRelocation;
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
        symbol_sizes_by_address: BTreeMap::new(),
        local_text_symbols: BTreeSet::new(),
        call_trampoline_addresses: BTreeSet::new(),
        relocated_calls_by_address: BTreeMap::new(),
        unresolved_relocations_by_address: BTreeMap::new(),
        global_pointer: None,
    }
}

fn empty_svd() -> MmioRegisterMap {
    MmioRegisterMap {
        registers: Vec::new(),
        windows: Vec::new(),
    }
}

fn direct_call_closure_image(unrelated: [u8; 8], callee: [u8; 4]) -> ExecutableImage {
    let mut image = tiny_image(
        [
            &[0xef, 0x00, 0x00, 0x01], // jal ra, +16
            &[0x67, 0x80, 0x00, 0x00], // ret
            unrelated.as_slice(),
            callee.as_slice(),
        ]
        .concat(),
        20,
    );
    image.symbols_by_name.extend([
        ("unrelated".to_owned(), 0x1008),
        ("callee".to_owned(), 0x1010),
    ]);
    image.symbols_by_address.extend([
        (0x1008, "unrelated".to_owned()),
        (0x1010, "callee".to_owned()),
    ]);
    image.symbol_sizes_by_address = BTreeMap::from([(0x1000, 8), (0x1008, 8), (0x1010, 4)]);
    image.local_text_symbols.insert(0x1010);
    image
}

#[test]
fn code_closure_identity_ignores_unrelated_linked_code_and_binds_direct_callees() {
    let first = direct_call_closure_image(
        [0x67, 0x80, 0x00, 0x00, 0x67, 0x80, 0x00, 0x00],
        [0x67, 0x80, 0x00, 0x00],
    );
    let unrelated_changed = direct_call_closure_image(
        [0x73, 0x00, 0x10, 0x00, 0x67, 0x80, 0x00, 0x00],
        [0x67, 0x80, 0x00, 0x00],
    );
    let callee_changed = direct_call_closure_image(
        [0x67, 0x80, 0x00, 0x00, 0x67, 0x80, 0x00, 0x00],
        [0x73, 0x00, 0x10, 0x00],
    );

    let identity = first.code_closure_identity("test").unwrap();
    assert_eq!(
        identity,
        unrelated_changed.code_closure_identity("test").unwrap()
    );
    assert_ne!(
        identity,
        callee_changed.code_closure_identity("test").unwrap()
    );
    assert!(identity.contains("target=1"));
    assert!(identity.contains("node 1 size=4"));

    let mut global_callee = first;
    global_callee.local_text_symbols.clear();
    let mut changed_global_callee = callee_changed;
    changed_global_callee.local_text_symbols.clear();
    let global_identity = global_callee.code_closure_identity("test").unwrap();
    assert_eq!(
        global_identity,
        changed_global_callee.code_closure_identity("test").unwrap()
    );
    assert!(global_identity.contains("external-symbol=callee"));
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
        symbol_sizes_by_address: BTreeMap::new(),
        local_text_symbols: BTreeSet::new(),
        call_trampoline_addresses: BTreeSet::new(),
        relocated_calls_by_address: BTreeMap::from([(
            0x1000,
            RelocatedCall {
                name: "callee".to_owned(),
                target: None,
            },
        )]),
        unresolved_relocations_by_address: BTreeMap::new(),
        global_pointer: None,
    }
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
fn argument_constraints_prune_a_resolved_auipc_jalr_child_and_its_fallthrough() {
    let mut image = tiny_image(
        vec![
            0x63, 0x06, 0x05, 0x00, // beq a0, zero, +12 (valid return)
            0x97, 0x00, 0x00, 0x00, // auipc ra, 0
            0xe7, 0x80, 0x00, 0x01, // jalr ra, 16(ra) (panic-like child)
            0x67, 0x80, 0x00, 0x00, // valid return
            0x00, 0x00, 0x00, 0x00, // padding
            0x63, 0x00, 0x00, 0x00, // child: beq zero, zero, 0
        ],
        24,
    );
    image
        .symbols_by_name
        .insert("panic_child".to_owned(), 0x1014);
    image
        .symbols_by_address
        .insert(0x1014, "panic_child".to_owned());

    let unconstrained = image.coverage_inventory("test").unwrap();
    assert_eq!(unconstrained.branch_sites, BTreeSet::from([0x1000, 0x1014]));
    assert!(unconstrained.unresolved_edges.is_empty());

    let mut zero = [None; 8];
    zero[0] = Some(0);
    let constrained = image
        .coverage_inventory_with_argument_constraints("test", &zero)
        .unwrap();
    assert_eq!(constrained.branch_sites, BTreeSet::from([0x1000]));
    assert_eq!(
        constrained.branch_outcomes,
        BTreeSet::from([(0x1000, true)])
    );
    assert!(constrained.unresolved_edges.is_empty());
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
        symbol_sizes_by_address: BTreeMap::new(),
        local_text_symbols: BTreeSet::new(),
        call_trampoline_addresses: BTreeSet::from([0x2000]),
        relocated_calls_by_address: BTreeMap::from([(
            0x1000,
            RelocatedCall {
                name: "callee".to_owned(),
                target: Some(0x2000),
            },
        )]),
        unresolved_relocations_by_address: BTreeMap::new(),
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
        symbol_sizes_by_address: BTreeMap::new(),
        local_text_symbols: BTreeSet::new(),
        call_trampoline_addresses: BTreeSet::new(),
        relocated_calls_by_address: BTreeMap::new(),
        unresolved_relocations_by_address: BTreeMap::new(),
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
fn reviewed_call_model_intercepts_linked_code_and_is_fully_consumed() {
    let mut image = tiny_image(
        vec![
            0x13, 0x84, 0x00, 0x00, // addi s0, ra, 0
            0xef, 0x00, 0x00, 0x01, // jal ra, 16
            0x93, 0x00, 0x04, 0x00, // addi ra, s0, 0
            0x67, 0x80, 0x00, 0x00, // ret
            0, 0, 0, 0, // padding
            0x73, 0x00, 0x10, 0x00, // callee: ebreak (must not execute)
        ],
        24,
    );
    image
        .symbols_by_name
        .insert("platform_service".to_owned(), 0x1014);
    image
        .symbols_by_address
        .insert(0x1014, "platform_service".to_owned());
    let scenario = Scenario {
        call_returns: BTreeMap::from([(
            "platform_service".to_owned(),
            VecDeque::from([0x1234_5678]),
        )]),
        ..Scenario::default()
    };

    let result = execute(&image, &empty_svd(), "test", scenario).unwrap();
    assert_eq!(result.return_value, 0x1234_5678);
    assert_eq!(result.ordered_calls.len(), 1);
    assert_eq!(result.ordered_calls[0].symbol, "platform_service");
}

#[test]
fn reviewed_call_model_rejects_missing_and_unused_responses() {
    let mut image = tiny_image(
        vec![
            0x13, 0x84, 0x00, 0x00, // addi s0, ra, 0
            0xef, 0x00, 0x00, 0x01, // jal ra, 16
            0x93, 0x00, 0x04, 0x00, // addi ra, s0, 0
            0x67, 0x80, 0x00, 0x00, // ret
            0, 0, 0, 0, // padding
            0x67, 0x80, 0x00, 0x00, // callee: ret
        ],
        24,
    );
    image
        .symbols_by_name
        .insert("platform_service".to_owned(), 0x1014);
    image
        .symbols_by_address
        .insert(0x1014, "platform_service".to_owned());

    let missing = Scenario {
        call_returns: BTreeMap::from([("platform_service".to_owned(), VecDeque::new())]),
        ..Scenario::default()
    };
    assert!(
        execute(&image, &empty_svd(), "test", missing)
            .unwrap_err()
            .to_string()
            .contains("without a remaining response")
    );

    let unused = Scenario {
        call_returns: BTreeMap::from([("platform_service".to_owned(), VecDeque::from([1, 2]))]),
        ..Scenario::default()
    };
    assert!(
        execute(&image, &empty_svd(), "test", unused)
            .unwrap_err()
            .to_string()
            .contains("unconsumed modeled call responses")
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
fn unreachable_unresolved_relocation_does_not_block_an_independent_function() {
    let mut image = tiny_image(
        vec![
            0x67, 0x80, 0x00, 0x00, // ret
            0, 0, 0, 0, // unrelated relocated data
        ],
        8,
    );
    image.unresolved_relocations_by_address.insert(
        0x1004,
        UnresolvedRelocation {
            name: "unrelated_global".to_owned(),
            r_type: object::elf::R_RISCV_32,
            width: 4,
        },
    );

    assert!(image.coverage_inventory("test").is_ok());
    assert!(execute(&image, &empty_svd(), "test", Scenario::default()).is_ok());
    assert_eq!(image.loaded_byte(0x1004), None);
}

#[test]
fn instruction_fetch_through_unresolved_relocation_fails_closed() {
    let mut image = tiny_image(vec![0x67, 0x80, 0x00, 0x00], 4);
    image.unresolved_relocations_by_address.insert(
        0x1000,
        UnresolvedRelocation {
            name: "instruction_target".to_owned(),
            r_type: object::elf::R_RISCV_HI20,
            width: 4,
        },
    );

    let inventory_error = image.coverage_inventory("test").unwrap_err().to_string();
    assert!(inventory_error.contains("instruction fetch reached unresolved ELF relocation"));
    assert!(inventory_error.contains("instruction_target"));
    let execution_error = execute(&image, &empty_svd(), "test", Scenario::default())
        .unwrap_err()
        .to_string();
    assert!(execution_error.contains("instruction fetch reached unresolved ELF relocation"));
}

#[test]
fn ram_read_through_unresolved_relocated_word_requires_an_explicit_seed() {
    let mut image = tiny_image(
        vec![
            0x03, 0xa5, 0x05, 0x00, // lw a0, 0(a1)
            0x67, 0x80, 0x00, 0x00, // ret
            0, 0, 0, 0, // unresolved relocated word
        ],
        12,
    );
    image.unresolved_relocations_by_address.insert(
        0x1008,
        UnresolvedRelocation {
            name: "global_pointer".to_owned(),
            r_type: object::elf::R_RISCV_32,
            width: 4,
        },
    );
    let scenario = Scenario {
        arguments: vec![0, 0x1008],
        ..Scenario::default()
    };
    let error = execute(&image, &empty_svd(), "test", scenario)
        .unwrap_err()
        .to_string();
    assert!(error.contains("RAM read reached unresolved ELF relocation"));
    assert!(error.contains("global_pointer"));

    let mut seeded = Scenario {
        arguments: vec![0, 0x1008],
        ..Scenario::default()
    };
    seeded.memory_initial.extend(
        0x1234_5678_u32
            .to_le_bytes()
            .into_iter()
            .enumerate()
            .map(|(offset, byte)| (0x1008 + offset as u32, byte)),
    );
    assert_eq!(
        execute(&image, &empty_svd(), "test", seeded)
            .unwrap()
            .return_value,
        0x1234_5678
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
fn atomic_word_operations_preserve_rv32_wrapping_and_comparison_semantics() {
    assert_eq!(atomic_word_result(AmoOp::Swap, 7, 11), 11);
    assert_eq!(atomic_word_result(AmoOp::Add, u32::MAX, 2), 1);
    assert_eq!(atomic_word_result(AmoOp::Xor, 0xaa, 0x0f), 0xa5);
    assert_eq!(atomic_word_result(AmoOp::And, 0xaa, 0x0f), 0x0a);
    assert_eq!(atomic_word_result(AmoOp::Or, 0xa0, 0x0f), 0xaf);
    assert_eq!(atomic_word_result(AmoOp::Min, u32::MAX, 1), u32::MAX);
    assert_eq!(atomic_word_result(AmoOp::Max, u32::MAX, 1), 1);
    assert_eq!(atomic_word_result(AmoOp::Minu, u32::MAX, 1), 1);
    assert_eq!(atomic_word_result(AmoOp::Maxu, u32::MAX, 1), u32::MAX);
}

#[test]
fn executes_atomic_or_on_private_stack_memory() {
    let image = tiny_image(
        [
            0xffc1_0293_u32, // addi t0, sp, -4
            0x0002_a023,     // sw zero, 0(t0)
            0x0550_0313,     // addi t1, zero, 0x55
            0x4662_a52f,     // amoor.w.aqrl a0, t1, (t0)
            0x0002_a583,     // lw a1, 0(t0)
            0x00b5_6533,     // or a0, a0, a1
            0x0000_8067,     // ret
        ]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect(),
        28,
    );
    let result = execute(&image, &empty_svd(), "test", Scenario::default()).unwrap();
    assert_eq!(result.return_value, 0x55);
    assert!(
        result
            .timeline
            .iter()
            .any(|event| matches!(event, ExecutionTimelineEvent::RamWrite { value: 0x55, .. }))
    );
}

#[test]
fn private_stack_fill_is_explicit_and_default_stack_remains_poison() {
    let image = tiny_image(
        [
            0xfff1_0293_u32, // addi t0, sp, -1
            0x0002_c503,     // lbu a0, 0(t0)
            0x0000_8067,     // ret
        ]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect(),
        12,
    );
    let error = execute(&image, &empty_svd(), "test", Scenario::default()).unwrap_err();
    assert!(error.to_string().contains("poison/unmapped memory"));

    let scenario = Scenario {
        private_stack_fill: Some(0xa5),
        ..Scenario::default()
    };
    let result = execute(&image, &empty_svd(), "test", scenario).unwrap();
    assert_eq!(result.return_value, 0xa5);
}

#[test]
fn reports_only_observed_memory_mutations() {
    let image = tiny_image(vec![0x67, 0x80, 0x00, 0x00], 4);
    let svd = empty_svd();
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
    let svd = empty_svd();
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
