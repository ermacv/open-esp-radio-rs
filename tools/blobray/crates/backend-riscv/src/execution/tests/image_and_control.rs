//! Executable-image identity, relocations and ordered-control regressions.

use super::*;

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

    let svd = MmioMap {
        registers: Vec::new(),
        regions: vec![crate::MmioRegion {
            name: "sentinel".to_owned(),
            start: 0,
            end: 1,
            readable: true,
            writable: true,
        }],
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
fn coverage_widens_a_changing_loop_constant_instead_of_enumerating_values() {
    let image = tiny_image(
        vec![
            0x13, 0x05, 0x15, 0x00, // addi a0, a0, 1
            0xe3, 0x1e, 0x05, 0xfe, // bne a0, zero, -4
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        12,
    );
    let mut arguments = [None; 8];
    arguments[0] = Some(0);

    let inventory = image
        .coverage_inventory_with_argument_constraints("test", &arguments)
        .unwrap();

    assert_eq!(inventory.branch_sites, BTreeSet::from([0x1004]));
    assert_eq!(
        inventory.branch_outcomes,
        BTreeSet::from([(0x1004, false), (0x1004, true)])
    );
}

#[test]
fn coverage_preserves_riscv_callee_saved_constants_across_calls() {
    let image = tiny_image(
        vec![
            0x13, 0x04, 0x05, 0x00, // mv s0, a0
            0xef, 0x00, 0x00, 0x01, // jal ra, +16
            0x63, 0x04, 0x04, 0x00, // beq s0, zero, +8
            0x67, 0x80, 0x00, 0x00, // false return
            0x67, 0x80, 0x00, 0x00, // true return
            0x67, 0x80, 0x00, 0x00, // callee return
        ],
        24,
    );
    let mut arguments = [None; 8];
    arguments[0] = Some(0);

    let inventory = image
        .coverage_inventory_with_argument_constraints("test", &arguments)
        .unwrap();

    assert_eq!(inventory.branch_sites, BTreeSet::from([0x1008]));
    assert_eq!(inventory.branch_outcomes, BTreeSet::from([(0x1008, true)]));
}

#[test]
fn coverage_resolves_a_concrete_jump_target_loaded_from_immutable_elf_data() {
    let mut image = tiny_image(
        vec![
            0xb7, 0x22, 0x00, 0x00, // lui t0, 0x2 (0x2000)
            0x83, 0xa2, 0x02, 0x00, // lw t0, 0(t0)
            0x67, 0x80, 0x02, 0x00, // jalr zero, 0(t0)
            0x67, 0x80, 0x00, 0x00, // selected case: ret
        ],
        16,
    );
    image.segments.push(Segment {
        address: 0x2000,
        bytes: 0x100c_u32.to_le_bytes().to_vec(),
        memory_size: 4,
        writable: false,
    });

    let inventory = image.coverage_inventory("test").unwrap();
    assert!(inventory.unresolved_edges.is_empty());

    let result = execute(&image, &empty_svd(), "test", Scenario::default()).unwrap();
    assert_eq!(result.return_value, 0);
}

#[test]
fn coverage_does_not_treat_mutable_elf_data_as_a_jump_table_constant() {
    let mut image = tiny_image(
        vec![
            0xb7, 0x22, 0x00, 0x00, // lui t0, 0x2 (0x2000)
            0x83, 0xa2, 0x02, 0x00, // lw t0, 0(t0)
            0x67, 0x80, 0x02, 0x00, // jalr zero, 0(t0)
            0x67, 0x80, 0x00, 0x00, // possible case: ret
        ],
        16,
    );
    image.segments.push(Segment {
        address: 0x2000,
        bytes: 0x100c_u32.to_le_bytes().to_vec(),
        memory_size: 4,
        writable: true,
    });

    let inventory = image.coverage_inventory("test").unwrap();
    assert_eq!(inventory.unresolved_edges.len(), 1);
}

#[test]
fn coverage_treats_compiler_arithmetic_runtime_as_an_atomic_operation() {
    let mut image = direct_call_closure_image(
        [0x67, 0x80, 0x00, 0x00, 0x67, 0x80, 0x00, 0x00],
        [0x63, 0x00, 0x00, 0x00], // implementation loop branch
    );
    image.symbols_by_name.remove("callee");
    image.symbols_by_name.insert("__udivdi3".to_owned(), 0x1010);
    image
        .symbols_by_address
        .insert(0x1010, "__udivdi3".to_owned());

    let inventory = image.coverage_inventory("test").unwrap();

    assert!(inventory.branch_sites.is_empty());
    assert!(inventory.unresolved_edges.is_empty());
}

fn arithmetic_runtime_result(symbol: &str, dividend: u64, divisor: u64) -> crate::Result<u64> {
    let mut image = tiny_image(
        vec![
            0xef, 0x00, 0x00, 0x01, // jal ra, +16
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        8,
    );
    image.symbols_by_name.insert(symbol.to_owned(), 0x1010);
    image.symbols_by_address.insert(0x1010, symbol.to_owned());
    let scenario = Scenario {
        arguments: vec![
            dividend as u32,
            (dividend >> 32) as u32,
            divisor as u32,
            (divisor >> 32) as u32,
        ],
        ..Scenario::default()
    };
    let svd = empty_svd();
    let mut machine = Machine::new(&image, &svd, 0x1000, scenario);

    assert!(machine.step()?);
    assert!(machine.step()?);
    Ok(u64::from(machine.register(rv_asm::Reg::A0))
        | (u64::from(machine.register(rv_asm::Reg::A1)) << 32))
}

#[test]
fn compiler_arithmetic_runtime_executes_unsigned_division_and_remainder() {
    let dividend = 0x1234_5678_9abc_def0;
    let divisor = 0x0000_0001_0000_0011;

    assert_eq!(
        arithmetic_runtime_result("__udivdi3", dividend, divisor).unwrap(),
        dividend / divisor
    );
    assert_eq!(
        arithmetic_runtime_result("__umoddi3", dividend, divisor).unwrap(),
        dividend % divisor
    );
}

#[test]
fn compiler_arithmetic_runtime_executes_signed_division_and_remainder() {
    let dividend = (-0x0000_0002_0000_0003_i64) as u64;
    let divisor = 0x0000_0001_0000_0001_i64 as u64;

    assert_eq!(
        arithmetic_runtime_result("__divdi3", dividend, divisor).unwrap(),
        ((dividend as i64) / (divisor as i64)) as u64
    );
    assert_eq!(
        arithmetic_runtime_result("__moddi3", dividend, divisor).unwrap(),
        ((dividend as i64) % (divisor as i64)) as u64
    );
}

#[test]
fn compiler_arithmetic_runtime_wraps_signed_overflow_like_rv32_helpers() {
    assert_eq!(
        arithmetic_runtime_result("__divdi3", i64::MIN as u64, (-1_i64) as u64).unwrap(),
        i64::MIN as u64
    );
    assert_eq!(
        arithmetic_runtime_result("__moddi3", i64::MIN as u64, (-1_i64) as u64).unwrap(),
        0
    );
}

#[test]
fn compiler_arithmetic_runtime_fails_closed_on_zero_divisor() {
    for symbol in ["__divdi3", "__moddi3", "__udivdi3", "__umoddi3"] {
        let error = arithmetic_runtime_result(symbol, 1, 0).unwrap_err();
        assert!(error.to_string().contains("zero divisor"));
        assert!(error.to_string().contains(symbol));
    }
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
        diagnostic_calls: BTreeMap::new(),
        global_pointer: None,
    };
    let result = execute(&image, &empty_svd(), "wrapper", Scenario::default()).unwrap();
    assert_eq!(result.ordered_calls.len(), 1);
    assert_eq!(result.ordered_calls[0].symbol, "callee");
}

#[test]
fn rom_call_vector_without_bytes_dispatches_named_delay() {
    let mut image = tiny_image(
        vec![
            0x6f, 0x00, 0x00, 0x01, // jal zero, +16
        ],
        4,
    );
    image
        .symbols_by_name
        .insert("__call_ets_delay_us".to_owned(), 0x1010);
    image
        .symbols_by_address
        .insert(0x1010, "__call_ets_delay_us".to_owned());
    image.call_trampoline_addresses.insert(0x1010);

    let scenario = Scenario {
        arguments: vec![2],
        ..Scenario::default()
    };
    let result = execute(&image, &empty_svd(), "test", scenario).unwrap();
    assert_eq!(result.events, [ExecutionEvent::DelayMicros(2)]);
}

#[test]
fn unresolved_rom_call_vector_fails_with_named_remedy() {
    let mut image = tiny_image(vec![0x6f, 0x00, 0x00, 0x01], 4);
    image
        .symbols_by_name
        .insert("__call_missing_service".to_owned(), 0x1010);
    image
        .symbols_by_address
        .insert(0x1010, "__call_missing_service".to_owned());
    image.call_trampoline_addresses.insert(0x1010);

    let error = execute(&image, &empty_svd(), "test", Scenario::default()).unwrap_err();
    let message = error.to_string();
    assert!(message.contains("unresolved call trampoline missing_service at 0x00001010"));
    assert!(message.contains("target image or an explicit call model"));
}

#[test]
fn absolute_memcpy_without_linked_bytes_has_concrete_memory_effects() {
    let mut image = tiny_image(vec![0x6f, 0x00, 0x00, 0x01], 4);
    image.symbols_by_name.insert("memcpy".to_owned(), 0x1010);
    image.symbols_by_address.insert(0x1010, "memcpy".to_owned());

    let source = 0x3000;
    let destination = 0x4000;
    let mut scenario = Scenario {
        arguments: vec![destination, source, 3],
        observed_memory: vec![MemoryRange {
            start: destination,
            length: 3,
        }],
        ..Scenario::default()
    };
    scenario
        .memory_initial
        .extend([(source, 0x11), (source + 1, 0x22), (source + 2, 0x33)]);
    scenario
        .memory_initial
        .extend([(destination, 0), (destination + 1, 0), (destination + 2, 0)]);

    let result = execute(&image, &empty_svd(), "test", scenario).unwrap();
    assert_eq!(result.return_value, destination);
    assert_eq!(result.memory_changes.len(), 3);
    assert_eq!(
        result
            .memory_changes
            .iter()
            .map(|change| change.after)
            .collect::<Vec<_>>(),
        [0x11, 0x22, 0x33]
    );
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
        diagnostic_calls: BTreeMap::new(),
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
