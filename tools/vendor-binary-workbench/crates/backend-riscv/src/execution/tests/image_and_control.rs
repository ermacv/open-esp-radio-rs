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
