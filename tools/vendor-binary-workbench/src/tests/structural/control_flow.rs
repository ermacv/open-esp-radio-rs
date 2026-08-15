use super::super::*;

#[test]
fn structurally_accounted_floating_load_does_not_block_later_integer_ir() {
    let symbol = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "float_then_integer".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x07, 0x20, 0x05, 0x00, // flw f0, 0(a0)
            0x13, 0x05, 0x15, 0x00, // addi a0, a0, 1
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

    assert!(trace.blockers.is_empty(), "{trace:#?}");
    assert_ne!(trace.return_value, SymbolicValue::Unknown);
    assert!(trace.is_reference_eligible());
}

#[test]
fn unsupported_floating_arithmetic_remains_a_blocker() {
    let fadd_s = (11_u32 << 20) | (10 << 15) | (12 << 7) | 0x53;
    let symbol = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "floating_arithmetic_then_integer".to_owned(),
        address: 0x1000,
        bytes: [fadd_s, 0x0015_0513, 0x0000_8067]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect(),
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

    assert_eq!(trace.blockers.len(), 1, "{trace:#?}");
    assert!(trace.blockers[0].contains("class=floating-point"));
    assert_ne!(trace.return_value, SymbolicValue::Unknown);
    assert!(!trace.is_reference_eligible());
}

#[test]
fn rx11ax_ampdu_float_slice_preserves_structural_value_flow() {
    let fmv_from_integer =
        |floating: u32, integer: u32| (0x78_u32 << 25) | (integer << 15) | (floating << 7) | 0x53;
    let symbol = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "rx11ax_ampdu_float_slice".to_owned(),
        address: 0x1000,
        bytes: [
            fmv_from_integer(13, 12), // f13 <- raw bits from a2
            fmv_from_integer(8, 13),  // f8 <- raw bits from a3
            fmv_from_integer(14, 14), // f14 <- raw bits from a4
            0xd005_77d3,              // fcvt.s.w f15, a0, dyn
            0x08d7_f7d3,              // fsub.s f15, f15, f13, dyn
            0x18d7_f7d3,              // fdiv.s f15, f15, f13, dyn
            0x40f7_7743,              // fmadd.s f14, f14, f15, f8, dyn
            0xc007_17d3,              // fcvt.w.s a5, f14, rtz
            0x00f5_a023,              // sw a5, 0(a1)
            0x0000_8067,              // ret
        ]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect(),
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

    assert!(trace.blockers.is_empty(), "{trace:#?}");
    assert_eq!(trace.reference_blockers.len(), 5, "{trace:#?}");
    assert!(
        trace
            .reference_blockers
            .iter()
            .all(|blocker| blocker.starts_with("floating-")),
        "{trace:#?}"
    );
    assert!(!trace.is_reference_eligible());
    let value = trace
        .reference_events
        .iter()
        .find_map(|event| match event {
            DraftReferenceEvent::Memory {
                access: MemoryAccess::Write,
                value: Some(value),
                ..
            } => Some(value),
            _ => None,
        })
        .expect("the final store retains its structural value");
    let canonical = value.canonical();
    assert!(canonical.contains("SingleToSignedWord"), "{canonical}");
    assert!(canonical.contains("FusedMultiplyAddSingle"), "{canonical}");
    assert!(canonical.contains("DivideSingle"), "{canonical}");
    assert!(canonical.contains("SubtractSingle"), "{canonical}");
    assert!(canonical.contains("SignedWordToSingle"), "{canonical}");
}

#[test]
fn floating_comparison_with_unknown_inputs_remains_a_blocker() {
    let fmv_f10 = (0x78_u32 << 25) | (10 << 15) | (10 << 7) | 0x53;
    let fmv_f11 = (0x78_u32 << 25) | (11 << 15) | (11 << 7) | 0x53;
    let feq = (0x50_u32 << 25) | (11 << 20) | (10 << 15) | (2 << 12) | (10 << 7) | 0x53;
    let symbol = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "unknown_floating_compare".to_owned(),
        address: 0x1000,
        bytes: [fmv_f10, fmv_f11, feq, 0x0000_8067]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect(),
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

    assert_eq!(trace.blockers.len(), 1, "{trace:#?}");
    assert!(trace.blockers[0].contains("class=floating-point"));
    assert_eq!(trace.return_value, SymbolicValue::Unknown);
    assert!(!trace.is_reference_eligible());
}

#[test]
fn vendor_custom_decode_blocker_stops_unknown_control_flow() {
    let symbol = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "custom_then_integer".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x0b, 0x00, 0x00, 0x00, // custom-0 encoding
            0x13, 0x05, 0x15, 0x00, // addi a0, a0, 1 (not proven reachable)
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

    assert_eq!(trace.blockers.len(), 1, "{trace:#?}");
    assert!(trace.blockers[0].contains("class=vendor-custom"));
    assert_eq!(trace.return_value, SymbolicValue::Unknown);
}

#[test]
fn forward_local_jump_skips_dead_instructions() {
    let symbol = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "local_jump".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x6f, 0x00, 0x80, 0x00, // j 0x1008
            0x73, 0x00, 0x10, 0x00, // ebreak (unreachable)
            0x13, 0x05, 0x05, 0x00, // mv a0, a0
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
    assert_eq!(trace.return_value, SymbolicValue::input(0));
}

#[test]
fn local_jump_loop_fails_closed() {
    let symbol = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "local_loop".to_owned(),
        address: 0x1000,
        bytes: vec![0x6f, 0x00, 0x00, 0x00], // j 0x1000
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

    assert!(!trace.is_reference_eligible());
    assert!(trace.blockers[0].contains("control-flow loop"));
}

#[test]
fn constant_counted_loop_is_bounded_and_fully_unrolled() {
    let symbol = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "constant_counted_loop".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x93, 0x05, 0x00, 0x00, // li a1, 0
            0x13, 0x06, 0x30, 0x00, // li a2, 3
            0x93, 0x85, 0x15, 0x00, // addi a1, a1, 1
            0xe3, 0x9e, 0xc5, 0xfe, // bne a1, a2, -4
            0x13, 0x85, 0x05, 0x00, // mv a0, a1
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
    assert!(trace.blockers.is_empty(), "{trace:#?}");
    assert_eq!(trace.return_value, SymbolicValue::Constant(3));
}

#[test]
fn calibration_sized_constant_loop_is_bounded_and_fully_unrolled() {
    let symbol = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "calibration_sized_constant_loop".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x93, 0x05, 0x00, 0x00, // li a1, 0
            0x13, 0x06, 0xc0, 0x12, // li a2, 300
            0x93, 0x85, 0x15, 0x00, // addi a1, a1, 1
            0xe3, 0x9e, 0xc5, 0xfe, // bne a1, a2, -4
            0x13, 0x85, 0x05, 0x00, // mv a0, a1
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
    assert!(trace.blockers.is_empty(), "{trace:#?}");
    assert_eq!(trace.return_value, SymbolicValue::Constant(300));
}

#[test]
fn backward_edge_to_an_unvisited_return_block_is_not_a_loop() {
    let symbol = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "backward_acyclic_edge".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x63, 0x08, 0x05, 0x00, // beq a0, zero, 0x1010
            0x13, 0x05, 0x10, 0x00, // li a0, 1
            0x67, 0x80, 0x00, 0x00, // ret
            0x13, 0x00, 0x00, 0x00, // nop (unreachable padding)
            0xe3, 0x0c, 0x05, 0xfe, // beq a0, zero, 0x1008
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let symbols = BTreeMap::from([(0x1000, symbol.clone())]);

    let trace = resolve_reference_trace(
        &symbol,
        &symbols,
        &BTreeMap::new(),
        &StructuralPointerContext::default(),
        None,
        &map(),
        &mut BTreeSet::new(),
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert!(trace.reference_flow.is_some(), "{trace:#?}");
}

#[test]
fn partial_cfg_keeps_indexed_memory_evidence_across_an_opaque_call() {
    let symbol = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "partial_indexed_snapshot".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x63, 0x04, 0x05, 0x00, // beq a0, zero, 0x1008
            0x67, 0x80, 0x00, 0x00, // ret
            0x13, 0x04, 0x05, 0x00, // mv s0, a0
            0xe7, 0x80, 0x07, 0x00, // jalr a5 (opaque callback)
            0x93, 0x07, 0xc0, 0x02, // li a5, 44
            0x33, 0x04, 0xf4, 0x02, // mul s0, s0, a5
            0xb7, 0xf7, 0x02, 0x10, // lui a5, 0x1002f
            0x93, 0x87, 0x07, 0x56, // addi a5, a5, 0x560
            0xb3, 0x87, 0x87, 0x00, // add a5, a5, s0
            0x23, 0xa0, 0xb7, 0x00, // sw a1, 0(a5)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let symbols = BTreeMap::from([(0x1000, symbol.clone())]);

    let trace = resolve_reference_trace(
        &symbol,
        &symbols,
        &BTreeMap::new(),
        &StructuralPointerContext::default(),
        None,
        &map(),
        &mut BTreeSet::new(),
    )
    .unwrap();

    assert!(!trace.is_reference_eligible(), "{trace:#?}");
    assert!(
        trace
            .reference_blockers
            .iter()
            .any(|blocker| blocker.starts_with("unresolved-indirect-call at ")),
        "{trace:#?}"
    );
    let flow = trace
        .reference_flow
        .as_ref()
        .expect("partial evidence retains structured control flow");
    let DraftReferenceTerminator::Branch {
        taken, not_taken, ..
    } = &flow.terminator
    else {
        panic!("expected the input branch, got {flow:#?}");
    };
    let memory_event = taken
        .events
        .iter()
        .chain(&not_taken.events)
        .find(|event| matches!(event, DraftReferenceEvent::Memory { .. }))
        .expect("one branch retains the indexed write");
    let DraftReferenceEvent::Memory { address, value, .. } = memory_event else {
        unreachable!();
    };
    assert_eq!(value, &Some(SymbolicValue::Unknown));
    assert_eq!(
        address.memory_object_location_with_reads(&BTreeMap::new()),
        Some(MemoryObjectLocation {
            root: MemoryObjectRoot::Indexed {
                root: std::sync::Arc::new(MemoryObjectRoot::Absolute {
                    address: 0x1002_f560,
                }),
                argument: 0,
                stride: 0x2c,
            },
            offset: 0,
        })
    );
}

#[test]
fn partial_cfg_keeps_branch_evidence_across_unsupported_floating_arithmetic() {
    let fadd_s = (11_u32 << 20) | (10 << 15) | (12 << 7) | 0x53;
    let lui_a2 = (0x20107_u32 << 12) | (12 << 7) | 0x37;
    let store_a1 = (1_u32 << 25) | (11 << 20) | (12 << 15) | (2 << 12) | (16 << 7) | 0x23;
    let symbol = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "partial_floating_branch".to_owned(),
        address: 0x1000,
        bytes: [
            0x0005_0863, // beq a0, zero, 0x1010
            fadd_s,
            lui_a2,
            store_a1,
            0x0000_8067, // ret
        ]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect(),
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let symbols = BTreeMap::from([(0x1000, symbol.clone())]);

    let trace = resolve_reference_trace(
        &symbol,
        &symbols,
        &BTreeMap::new(),
        &StructuralPointerContext::default(),
        None,
        &map(),
        &mut BTreeSet::new(),
    )
    .unwrap();

    assert!(!trace.is_reference_eligible(), "{trace:#?}");
    assert!(
        trace
            .reference_blockers
            .iter()
            .any(|blocker| blocker.contains("class=floating-point")),
        "{trace:#?}"
    );
    let flow = trace
        .reference_flow
        .as_ref()
        .expect("partial FP path retains structured control flow");
    let DraftReferenceTerminator::Branch {
        taken, not_taken, ..
    } = &flow.terminator
    else {
        panic!("expected input branch: {flow:#?}");
    };
    assert!(
        taken
            .events
            .iter()
            .chain(&not_taken.events)
            .any(|event| matches!(event, DraftReferenceEvent::Observable(_))),
        "{flow:#?}"
    );
}

#[test]
fn structured_cfg_resolves_common_stack_spills_on_each_complete_path() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "stack_spill_branch".to_owned(),
        address: 0x1000,
        bytes: [
            0xff01_0113, // addi sp, sp, -16
            0x0081_2623, // sw s0, 12(sp)
            0x0005_0463, // beq a0, zero, 0x1010
            0x00c1_2583, // lw a1, 12(sp), only on one path
            0x0000_8067, // ret
        ]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect(),
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let symbols = BTreeMap::from([(0x1000, parent.clone())]);

    let trace = resolve_reference_trace(
        &parent,
        &symbols,
        &BTreeMap::new(),
        &StructuralPointerContext::default(),
        None,
        &map(),
        &mut BTreeSet::new(),
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    let flow = trace.reference_flow.as_ref().expect("structured branch");
    assert!(flow.events.is_empty(), "{flow:#?}");
    assert!(matches!(
        flow.terminator,
        DraftReferenceTerminator::Branch { .. }
    ));
}

#[test]
fn structured_cfg_composes_private_stack_memset_on_only_one_path() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: Some("branch.o".to_owned()),
        name: "branch_with_stack_fill".to_owned(),
        address: 0x1000,
        bytes: [
            0xff01_0113, // addi sp, sp, -16
            0x0205_0263, // beq a0, zero, 0x1028
            0x0001_0513, // mv a0, sp
            0x05a0_0593, // li a1, 0x5a
            0x0040_0613, // li a2, 4
            0x0000_0317, // auipc t1, 0
            0x0003_00e7, // jalr ra, 0(t1), relocated memset
            0x0001_2683, // lw a3, 0(sp), initialized only on this path
            0x0070_0513, // li a0, 7
            0x0000_8067, // ret
            0x0080_0513, // li a0, 8
            0x0000_8067, // ret
        ]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect(),
        addresses_resolved: false,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let relocations = BTreeMap::from([(
        StructuralCallSite::new(&parent, 0x1014),
        ("memset".to_owned(), Some(0x2000)),
    )]);
    let memset = artifact::ArtifactSymbolDefinition {
        member: Some("runtime.o".to_owned()),
        name: "memset".to_owned(),
        address: 0x2000,
        // The standard ABI contract, not this deliberately unsupported body,
        // owns the call behavior.
        bytes: vec![0x73, 0x00, 0x10, 0x00],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let symbols = BTreeMap::from([(0x2000, memset)]);

    let pointer_context = synthetic_delay_pointer_context();
    let trace = resolve_reference_trace(
        &parent,
        &symbols,
        &relocations,
        &pointer_context,
        None,
        &map(),
        &mut BTreeSet::from([0x1000]),
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    let DraftReferenceTerminator::Branch {
        taken, not_taken, ..
    } = &trace
        .reference_flow
        .as_ref()
        .expect("structured branch")
        .terminator
    else {
        panic!("expected branch: {trace:#?}");
    };
    assert!(taken.events.is_empty(), "{taken:#?}");
    assert!(not_taken.events.is_empty(), "{not_taken:#?}");
    assert!(matches!(
        taken.terminator,
        DraftReferenceTerminator::Return(SymbolicValue::Constant(8))
    ));
    assert!(matches!(
        not_taken.terminator,
        DraftReferenceTerminator::Return(SymbolicValue::Constant(7))
    ));
}

#[test]
fn delay_intrinsic_is_composed_without_decoding_its_rom_body() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "delay_wrapper".to_owned(),
        address: 0x1000,
        bytes: vec![0x6f, 0x10, 0x00, 0x00], // j 0x2000
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let delay = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "ets_delay_us".to_owned(),
        address: 0x2000,
        bytes: vec![0x73, 0x00, 0x10, 0x00], // body is deliberately unsupported
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let symbols = BTreeMap::from([(0x2000, delay)]);
    let mut visiting = BTreeSet::from([0x1000]);

    let trace = resolve_reference_trace(
        &parent,
        &symbols,
        &BTreeMap::new(),
        &synthetic_delay_pointer_context(),
        None,
        &map(),
        &mut visiting,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert_eq!(trace.reference_dependencies, ["ets_delay_us"]);
    assert_eq!(
        trace.reference_events,
        [DraftReferenceEvent::DelayMicros {
            micros: SymbolicValue::input(0)
        }]
    );
}

#[test]
fn constant_conditional_branch_follows_only_the_feasible_edge() {
    let symbol = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "constant_branch".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x63, 0x04, 0x00, 0x00, // beq zero, zero, 0x1008
            0x73, 0x00, 0x10, 0x00, // ebreak (infeasible)
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
}

#[test]
fn symbolic_conditional_branch_fails_closed() {
    let symbol = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "symbolic_branch".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x63, 0x04, 0x05, 0x00, // beq a0, zero, 0x1008
            0x67, 0x80, 0x00, 0x00, // ret
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

    assert!(!trace.is_reference_eligible());
    assert!(trace.blockers[0].contains("input-dependent control-flow"));
}

#[test]
fn bounded_symbolic_cfg_becomes_structured_reference_flow() {
    let symbol = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "symbolic_branch_reference".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x63, 0x06, 0x05, 0x00, // beq a0, zero, 0x100c
            0x13, 0x05, 0x10, 0x00, // li a0, 1
            0x67, 0x80, 0x00, 0x00, // ret
            0x13, 0x05, 0x20, 0x00, // li a0, 2
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let mut visiting = BTreeSet::from([0x1000]);

    let trace = resolve_reference_trace(
        &symbol,
        &BTreeMap::new(),
        &BTreeMap::new(),
        &StructuralPointerContext::default(),
        None,
        &map(),
        &mut visiting,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert!(!trace.is_exact());
    let DraftReferenceTerminator::Branch {
        condition,
        taken,
        not_taken,
    } = &trace.reference_flow.as_ref().unwrap().terminator
    else {
        panic!("expected a structured branch");
    };
    assert_eq!(condition.site, 0x1000);
    assert!(matches!(
        taken.terminator,
        DraftReferenceTerminator::Return(SymbolicValue::Constant(2))
    ));
    assert!(matches!(
        not_taken.terminator,
        DraftReferenceTerminator::Return(SymbolicValue::Constant(1))
    ));

    let generated = generate_reference(&trace, "oracle.elf", "abc123", None, &[]).unwrap();
    assert!(generated.exit_a0_modeled);
    assert!(
        generated
            .source
            .contains("// Symbolic branch from 0x00001000.")
    );
    assert!(generated.source.contains("if (args[0]"));
    assert!(
        generated
            .source
            .contains("ReferenceOutcome { exit_a0: Some(0x00000002_u32) }")
    );
    assert!(
        generated
            .source
            .contains("ReferenceOutcome { exit_a0: Some(0x00000001_u32) }")
    );
}

#[test]
fn loop_invariant_symbolic_branch_is_one_structured_decision() {
    let symbol = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "loop_invariant_symbolic_branch".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x93, 0x05, 0x00, 0x00, // li a1, 0
            0x13, 0x06, 0x30, 0x00, // li a2, 3
            0x63, 0x04, 0x05, 0x00, // beq a0, zero, 0x1010
            0x93, 0x86, 0x16, 0x00, // addi a3, a3, 1
            0x93, 0x85, 0x15, 0x00, // addi a1, a1, 1
            0xe3, 0x9a, 0xc5, 0xfe, // bne a1, a2, 0x1008
            0x13, 0x85, 0x05, 0x00, // mv a0, a1
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let mut visiting = BTreeSet::from([0x1000]);

    let trace = resolve_reference_trace(
        &symbol,
        &BTreeMap::new(),
        &BTreeMap::new(),
        &StructuralPointerContext::default(),
        None,
        &map(),
        &mut visiting,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    let DraftReferenceTerminator::Branch {
        condition,
        taken,
        not_taken,
    } = &trace.reference_flow.as_ref().unwrap().terminator
    else {
        panic!("expected one loop-invariant structured branch");
    };
    assert_eq!(condition.site, 0x1008);
    assert!(matches!(
        taken.terminator,
        DraftReferenceTerminator::Return(SymbolicValue::Constant(3))
    ));
    assert!(matches!(
        not_taken.terminator,
        DraftReferenceTerminator::Return(SymbolicValue::Constant(3))
    ));
}

#[test]
fn constant_call_argument_specializes_a_child_branch() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "constant_wrapper".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x13, 0x05, 0x00, 0x00, // li a0, 0
            0x6f, 0x00, 0x40, 0x00, // j 0x1008
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let child = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "conditional_child".to_owned(),
        address: 0x1008,
        bytes: vec![
            0x63, 0x04, 0x05, 0x00, // beq a0, zero, 0x1010
            0x73, 0x00, 0x10, 0x00, // ebreak (infeasible for this call)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let symbols = BTreeMap::from([(0x1008, child)]);
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
    assert_eq!(trace.reference_dependencies, ["conditional_child"]);
    assert_eq!(trace.return_value, SymbolicValue::Constant(0));
}

#[test]
fn local_basic_block_labels_do_not_truncate_the_function() {
    let disassembly = r#"
20100000 <conditional>:
20100000: beqz a0, 0x20100008 <.Ldone>
20100004: nop
20100008 <.Ldone>:
20100008: j 0x20100010 <child>
20100010 <next_function>:
20100010: ret
"#;
    let trace = trace_disassembly("conditional", disassembly, &map());
    assert!(!trace.is_exact());
    assert_eq!(trace.blockers.len(), 2);
    assert!(trace.blockers[0].contains("beqz"));
    assert!(trace.blockers[1].contains("j"));
}

#[test]
fn input_dependent_rmw_is_canonical_across_instruction_selection() {
    let vendor = r#"
20100000 <vendor>:
20100000: lui a4, 0x20107
20100004: lw a5, 0x30(a4)
20100008: slli a0, a0, 0x5
2010000c: andi a0, a0, 0x20
20100010: andi a5, a5, -0x21
20100014: or a0, a0, a5
20100018: sw a0, 0x30(a4)
2010001c: ret
"#;
    let rust = r#"
20100100 <rust>:
20100100: lui a4, 0x20107
20100104: lw a5, 0x30(a4)
20100108: andi a0, a0, 0x1
2010010c: slli a0, a0, 0x5
20100110: andi a5, a5, -0x21
20100114: or a5, a5, a0
20100118: sw a5, 0x30(a4)
2010011c: ret
"#;
    let vendor = trace_disassembly("vendor", vendor, &map());
    let rust = trace_disassembly("rust", rust, &map());
    assert!(vendor.is_exact());
    assert!(rust.is_exact());
    assert!(traces_equal(&vendor, &rust));
    assert!(
        vendor.events[1]
            .memory_value()
            .is_some_and(|value| value.contains("5=arg0.0"))
    );
}

#[test]
fn return_comparison_detects_a_wrong_field_from_the_same_read() {
    let vendor = r#"
20100000 <vendor>:
20100000: lui a4, 0x20107
20100004: lw a0, 0x30(a4)
20100008: srli a0, a0, 0xa
2010000c: andi a0, a0, 0x1
20100010: ret
"#;
    let rust = r#"
20100100 <rust>:
20100100: lui a4, 0x20107
20100104: lw a0, 0x30(a4)
20100108: srli a0, a0, 0x9
2010010c: andi a0, a0, 0x1
20100110: ret
"#;
    let vendor = trace_disassembly("vendor", vendor, &map());
    let rust = trace_disassembly("rust", rust, &map());
    assert!(traces_equal(&vendor, &rust));
    assert!(!returns_equal(&vendor, &rust));
}

#[test]
fn tail_jump_and_unresolved_write_both_fail_closed() {
    let tail = r#"
20100000 <tailing>:
20100000: j 0x20100020
"#;
    let trace = trace_disassembly("tailing", tail, &map());
    assert!(!trace.is_exact());
    assert_eq!(trace.blockers.len(), 1);

    let unresolved = r#"
20100000 <dynamic>:
20100000: lui a4, 0x20107
20100002: mul a0, a0, a1
20100004: sw a0, 0x30(a4)
20100008: ret
"#;
    let trace = trace_disassembly("dynamic", unresolved, &map());
    assert!(!trace.is_exact());
    assert_eq!(trace.blockers.len(), 1);
}

#[test]
fn fence_presence_and_position_are_compared() {
    let vendor = r#"
20100000 <vendor>:
20100000: fence r, w
20100004: fence w, r
20100008: ret
"#;
    let without_fence = r#"
20100100 <rust>:
20100100: ret
"#;
    let reversed = r#"
20100200 <rust>:
20100200: fence w, r
20100204: fence r, w
20100208: ret
"#;
    let vendor = trace_disassembly("vendor", vendor, &map());
    assert!(vendor.is_exact());
    assert!(!traces_equal(
        &vendor,
        &trace_disassembly("rust", without_fence, &map())
    ));
    assert!(!traces_equal(
        &vendor,
        &trace_disassembly("rust", reversed, &map())
    ));
}
