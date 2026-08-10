use super::super::*;

#[test]
fn call_summary_substitutes_arguments_and_remaps_read_tokens() {
    let prefix = vec![DraftReferenceEvent::Observable(ObservableEvent::Memory {
        access: MemoryAccess::Read,
        width: 32,
        address: 0x2010_7030,
        register: "AGC.FIRST".to_owned(),
        value: None,
    })];
    let callee = FunctionAnalysis {
        symbol: "child".to_owned(),
        events: Vec::new(),
        located_events: Vec::new(),
        reference_events: vec![
            DraftReferenceEvent::Observable(ObservableEvent::Memory {
                access: MemoryAccess::Read,
                width: 32,
                address: 0x2010_7034,
                register: "AGC.SECOND".to_owned(),
                value: None,
            }),
            DraftReferenceEvent::Observable(ObservableEvent::Memory {
                access: MemoryAccess::Write,
                width: 32,
                address: 0x2010_7038,
                register: "AGC.THIRD".to_owned(),
                value: Some(SymbolicValue::input(0)),
            }),
        ],
        reference_dependencies: Vec::new(),
        blockers: Vec::new(),
        reference_blockers: Vec::new(),
        return_value: SymbolicValue::RegisterImage {
            read_token: 0,
            address: 0x2010_7034,
            and_mask: u32::MAX,
            or_mask: 0,
        },
        reference_flow: None,
        unresolved_branch: None,
    };
    let arguments: Rv32CallArguments = core::array::from_fn(|index| {
        if index == 0 {
            SymbolicValue::input(1)
        } else {
            SymbolicValue::Unknown
        }
    });

    let (events, return_value) =
        inline_reference_summary(&prefix, &callee, &arguments, None).unwrap();

    assert_eq!(events.len(), 3);
    let DraftReferenceEvent::Observable(ObservableEvent::Memory {
        value: Some(write_value),
        ..
    }) = &events[2]
    else {
        panic!("expected substituted write");
    };
    assert!(write_value.canonical().contains("arg1"));
    assert_eq!(
        return_value.canonical(),
        "rmw:read1[0x20107034]&0xffffffff|0x00000000"
    );
}

#[test]
fn call_summary_substitutes_indexed_mmio_and_preserves_read_identity() {
    let prefix = vec![DraftReferenceEvent::Observable(ObservableEvent::Memory {
        access: MemoryAccess::Read,
        width: 32,
        address: 0x2010_7030,
        register: "AGC.FIRST".to_owned(),
        value: None,
    })];
    let callee = FunctionAnalysis {
        symbol: "indexed_child".to_owned(),
        events: Vec::new(),
        located_events: Vec::new(),
        reference_events: vec![DraftReferenceEvent::IndexedMmio {
            access: MemoryAccess::Read,
            width: 32,
            address: SymbolicValue::input(0)
                .shift_left(2)
                .add_constant(0x2010_4000),
            registers: vec![
                IndexedMmioRegister {
                    address: 0x2010_4000,
                    name: "WIFI.QUEUE0".to_owned(),
                },
                IndexedMmioRegister {
                    address: 0x2010_4004,
                    name: "WIFI.QUEUE1".to_owned(),
                },
            ],
            guard: Some(IndexedMmioGuard {
                selector: SymbolicValue::input(0),
                maximum: 1,
            }),
            value: None,
        }],
        reference_dependencies: Vec::new(),
        blockers: Vec::new(),
        reference_blockers: Vec::new(),
        return_value: SymbolicValue::IndexedRegisterImage {
            read_token: 0,
            and_mask: u32::MAX,
            or_mask: 0,
        },
        reference_flow: None,
        unresolved_branch: None,
    };
    let arguments: Rv32CallArguments = core::array::from_fn(|index| {
        if index == 0 {
            SymbolicValue::input(1)
        } else {
            SymbolicValue::Unknown
        }
    });

    let (events, return_value) =
        inline_reference_summary(&prefix, &callee, &arguments, None).unwrap();
    let DraftReferenceEvent::IndexedMmio {
        address,
        guard: Some(guard),
        ..
    } = &events[1]
    else {
        panic!("expected indexed MMIO read");
    };
    assert!(address.canonical().contains("arg1"));
    assert!(guard.selector.canonical().contains("arg1"));
    assert_eq!(
        return_value.canonical(),
        "indexed-rmw:read1&0xffffffff|0x00000000"
    );
}

#[test]
fn call_summary_substitutes_caller_owned_memory_addresses() {
    let callee = FunctionAnalysis {
        symbol: "memory_child".to_owned(),
        events: Vec::new(),
        located_events: Vec::new(),
        reference_events: vec![DraftReferenceEvent::Memory {
            access: MemoryAccess::Read,
            width: 32,
            address: SymbolicValue::input(0).add_constant(4),
            region: "caller-owned ABI argument RAM".to_owned(),
            value: None,
        }],
        reference_dependencies: Vec::new(),
        blockers: Vec::new(),
        reference_blockers: Vec::new(),
        return_value: SymbolicValue::MemoryImage {
            read_token: 0,
            and_mask: u32::MAX,
            or_mask: 0,
        },
        reference_flow: None,
        unresolved_branch: None,
    };
    let arguments: Rv32CallArguments = core::array::from_fn(|index| {
        if index == 0 {
            SymbolicValue::input(2).add_constant(8)
        } else {
            SymbolicValue::Unknown
        }
    });

    let (events, return_value) = inline_reference_summary(&[], &callee, &arguments, None).unwrap();
    let [
        DraftReferenceEvent::Memory {
            access: MemoryAccess::Read,
            address,
            ..
        },
    ] = events.as_slice()
    else {
        panic!("expected one substituted caller-memory read");
    };
    assert!(address.canonical().contains("arg2"));
    assert_eq!(
        return_value,
        SymbolicValue::MemoryImage {
            read_token: 0,
            and_mask: u32::MAX,
            or_mask: 0,
        }
    );
}

#[test]
fn private_stack_round_trips_symbolic_values_and_sign_extension() {
    let mut stack = SymbolicStack::default();
    stack.store(-8, 32, &SymbolicValue::input(2));
    assert_eq!(
        stack.load(-8, 32, false).unwrap().canonical(),
        SymbolicValue::input(2).canonical()
    );

    stack.store(-1, 8, &SymbolicValue::Constant(0x80));
    assert_eq!(
        stack.load(-1, 8, true).unwrap(),
        SymbolicValue::Constant(0xffff_ff80)
    );
    assert!(stack.load(-12, 32, false).is_none());
}

#[test]
fn unused_callee_write_to_caller_private_stack_is_internal_scratch() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "private_stack_parent".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x13, 0x05, 0xc1, 0x00, // addi a0, sp, 12
            0xef, 0x00, 0xd0, 0x7f, // jal ra, 0x2000
            0x13, 0x05, 0x00, 0x00, // li a0, 0
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let child = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "private_stack_writer".to_owned(),
        address: 0x2000,
        bytes: vec![
            0x93, 0x07, 0x20, 0x01, // li a5, 0x12
            0x23, 0x00, 0xf5, 0x00, // sb a5, 0(a0)
            0x13, 0x05, 0x00, 0x00, // li a0, 0
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let symbols = BTreeMap::from([(0x2000, child)]);
    let mut visiting = BTreeSet::from([0x1000]);

    let trace = resolve_reference_trace(
        &parent,
        &symbols,
        &BTreeMap::new(),
        &StructuralPointerContext::default(),
        None,
        &map(),
        &mut visiting,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert!(trace.reference_events.is_empty(), "{trace:#?}");
    assert_eq!(trace.reference_dependencies, ["private_stack_writer"]);
    assert_eq!(trace.return_value, SymbolicValue::Constant(0));
}

#[test]
fn consumed_callee_write_to_caller_private_stack_is_composed() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "private_stack_reader".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x13, 0x05, 0xc1, 0x00, // addi a0, sp, 12
            0xef, 0x00, 0xd0, 0x7f, // jal ra, 0x2000
            0x03, 0x45, 0xc1, 0x00, // lbu a0, 12(sp)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let child = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "private_stack_writer".to_owned(),
        address: 0x2000,
        bytes: vec![
            0x93, 0x07, 0x20, 0x01, // li a5, 0x12
            0x23, 0x00, 0xf5, 0x00, // sb a5, 0(a0)
            0x13, 0x05, 0x00, 0x00, // li a0, 0
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let symbols = BTreeMap::from([(0x2000, child)]);
    let mut visiting = BTreeSet::from([0x1000]);

    let trace = resolve_reference_trace(
        &parent,
        &symbols,
        &BTreeMap::new(),
        &StructuralPointerContext::default(),
        None,
        &map(),
        &mut visiting,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert!(trace.reference_events.is_empty(), "{trace:#?}");
    assert_eq!(trace.return_value, SymbolicValue::Constant(0x12));
}

#[test]
fn callee_read_from_initialized_caller_private_stack_is_composed() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "private_stack_input_parent".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x93, 0x07, 0x20, 0x01, // li a5, 0x12
            0x23, 0x06, 0xf1, 0x00, // sb a5, 12(sp)
            0x13, 0x05, 0xc1, 0x00, // addi a0, sp, 12
            0xef, 0x00, 0x50, 0x7f, // jal ra, 0x2000
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let child = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "private_stack_reader".to_owned(),
        address: 0x2000,
        bytes: vec![
            0x03, 0x45, 0x05, 0x00, // lbu a0, 0(a0)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let symbols = BTreeMap::from([(0x2000, child)]);
    let mut visiting = BTreeSet::from([0x1000]);

    let trace = resolve_reference_trace(
        &parent,
        &symbols,
        &BTreeMap::new(),
        &StructuralPointerContext::default(),
        None,
        &map(),
        &mut visiting,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert!(trace.reference_events.is_empty(), "{trace:#?}");
    assert_eq!(trace.return_value, SymbolicValue::Constant(0x12));
}

#[test]
fn entry_stack_argument_is_a_distinct_rv32_abi_input() {
    let symbol = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "stack_argument_reader".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x03, 0x45, 0x01, 0x00, // lbu a0, 0(sp): ninth ABI argument
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };

    let trace = trace_binary_symbol(
        &symbol,
        &map(),
        &BTreeMap::new(),
        &StructuralPointerContext::default(),
        None,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert_eq!(trace.return_value, SymbolicValue::input(8).and(0xff));
    let generated = generate_reference(&trace, "fixture.elf", "sha256", None, &[]).unwrap();
    assert!(
        generated
            .source
            .contains("pub struct Rv32ReferenceArguments"),
        "{}",
        generated.source
    );
    assert!(
        generated
            .source
            .contains("ReferenceOutcome { exit_a0: Some(args[8] & 0x000000ff_u32) }"),
        "{}",
        generated.source
    );
}

#[test]
fn outgoing_stack_argument_is_substituted_into_a_direct_callee() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "stack_argument_parent".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x93, 0x07, 0x20, 0x01, // li a5, 0x12
            0x23, 0x00, 0xf1, 0x00, // sb a5, 0(sp): ninth outgoing argument
            0xef, 0x00, 0xd0, 0x7f, // jal ra, 0x2004
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let child = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "stack_argument_child".to_owned(),
        address: 0x2004,
        bytes: vec![
            0x03, 0x45, 0x01, 0x00, // lbu a0, 0(sp)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let symbols = BTreeMap::from([(0x2004, child)]);
    let mut visiting = BTreeSet::from([0x1000]);

    let trace = resolve_reference_trace(
        &parent,
        &symbols,
        &BTreeMap::new(),
        &StructuralPointerContext::default(),
        None,
        &map(),
        &mut visiting,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert_eq!(trace.reference_dependencies, ["stack_argument_child"]);
    assert_eq!(trace.return_value, SymbolicValue::Constant(0x12));
}

#[test]
fn incoming_stack_argument_survives_an_unrelated_callee_stack_write() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "stack_argument_after_output_call".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x13, 0x05, 0xc1, 0x00, // addi a0, sp, 12
            0xef, 0x00, 0xd0, 0x7f, // jal ra, 0x2000
            0x03, 0x45, 0x01, 0x00, // lbu a0, 0(sp): ninth incoming argument
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let child = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "unrelated_stack_writer".to_owned(),
        address: 0x2000,
        bytes: vec![
            0x93, 0x07, 0x20, 0x01, // li a5, 0x12
            0x23, 0x00, 0xf5, 0x00, // sb a5, 0(a0)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let symbols = BTreeMap::from([(0x2000, child)]);
    let mut visiting = BTreeSet::from([0x1000]);

    let trace = resolve_reference_trace(
        &parent,
        &symbols,
        &BTreeMap::new(),
        &StructuralPointerContext::default(),
        None,
        &map(),
        &mut visiting,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert_eq!(trace.reference_dependencies, ["unrelated_stack_writer"]);
    assert_eq!(trace.return_value, SymbolicValue::input(8).and(0xff));
}

#[test]
fn pointer_reloaded_after_a_call_recovers_caller_memory_provenance() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "deferred_pointer_parent".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x23, 0x24, 0xb1, 0x00, // sw a1, 8(sp)
            0x13, 0x05, 0xc1, 0x00, // addi a0, sp, 12
            0xef, 0x00, 0x90, 0x7f, // jal ra, 0x2000
            0x03, 0x26, 0x81, 0x00, // lw a2, 8(sp)
            0x13, 0x07, 0x30, 0x12, // li a4, 0x123
            0x23, 0x10, 0xe6, 0x00, // sh a4, 0(a2)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let child = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "unrelated_stack_output".to_owned(),
        address: 0x2000,
        bytes: vec![
            0x93, 0x07, 0x20, 0x01, // li a5, 0x12
            0x23, 0x00, 0xf5, 0x00, // sb a5, 0(a0)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let symbols = BTreeMap::from([(0x2000, child)]);
    let mut visiting = BTreeSet::from([0x1000]);

    let trace = resolve_reference_trace(
        &parent,
        &symbols,
        &BTreeMap::new(),
        &StructuralPointerContext::default(),
        None,
        &map(),
        &mut visiting,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert_eq!(trace.reference_dependencies, ["unrelated_stack_output"]);
    assert_eq!(trace.reference_events.len(), 1, "{trace:#?}");
    let DraftReferenceEvent::Memory {
        access,
        width,
        address,
        value: Some(value),
        ..
    } = &trace.reference_events[0]
    else {
        panic!("expected a resolved caller-memory write: {trace:#?}");
    };
    assert_eq!((*access, *width), (MemoryAccess::Write, 16));
    assert_eq!(*address, SymbolicValue::input(1));
    assert_eq!(*value, SymbolicValue::Constant(0x123));
}

#[test]
fn deferred_pointer_without_caller_memory_provenance_fails_closed() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "invalid_deferred_pointer_parent".to_owned(),
        address: 0x1000,
        bytes: vec![
            0xb7, 0x75, 0x10, 0x20, // lui a1, 0x20107
            0x83, 0xa5, 0x05, 0x03, // lw a1, 0x30(a1): untrusted MMIO value
            0x23, 0x24, 0xb1, 0x00, // sw a1, 8(sp)
            0x13, 0x05, 0xc1, 0x00, // addi a0, sp, 12
            0xef, 0x00, 0x10, 0x7f, // jal ra, 0x2000
            0x03, 0x26, 0x81, 0x00, // lw a2, 8(sp)
            0x13, 0x07, 0x30, 0x12, // li a4, 0x123
            0x23, 0x10, 0xe6, 0x00, // sh a4, 0(a2)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let child = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "unrelated_stack_output".to_owned(),
        address: 0x2000,
        bytes: vec![
            0x93, 0x07, 0x20, 0x01, // li a5, 0x12
            0x23, 0x00, 0xf5, 0x00, // sb a5, 0(a0)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let symbols = BTreeMap::from([(0x2000, child)]);
    let mut visiting = BTreeSet::from([0x1000]);

    let trace = resolve_reference_trace(
        &parent,
        &symbols,
        &BTreeMap::new(),
        &StructuralPointerContext::default(),
        None,
        &map(),
        &mut visiting,
    )
    .unwrap();

    assert!(!trace.is_reference_eligible(), "{trace:#?}");
    assert!(
        trace
            .reference_failure_reasons()
            .iter()
            .any(|reason| reason.contains("did not resolve to affine caller-owned RAM")),
        "{trace:#?}"
    );
}
