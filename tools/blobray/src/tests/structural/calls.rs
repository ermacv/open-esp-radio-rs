use super::super::*;

#[test]
fn call_results_are_substituted_into_parent_dataflow() {
    let value = SymbolicValue::CallResult(7).and(0xff).shift_left(8).or(3);
    let call_results = BTreeMap::from([(7, SymbolicValue::Constant(0x1234))]);
    let private_stack_reads = BTreeMap::new();

    let rewritten = value
        .rewrite_call_context(&[], &[], &[], &call_results, &private_stack_reads)
        .unwrap();

    assert_eq!(rewritten, SymbolicValue::Constant(0x3403));
}

#[test]
fn whole_call_result_preserves_a_symbolic_expression_during_substitution() {
    let replacement = SymbolicValue::expression(
        ExpressionOperation::ShiftRightArithmetic,
        SymbolicValue::RegisterImage {
            read_token: 0,
            address: 0x2010_708c,
            and_mask: 0x0000_0fff,
            or_mask: 0xffff_f000,
        },
        SymbolicValue::Constant(2),
    );
    let value = SymbolicValue::CallResult(7).add_constant(2);
    let call_results = BTreeMap::from([(7, replacement.clone())]);

    let rewritten = value
        .rewrite_call_context(&[], &[], &[], &call_results, &BTreeMap::new())
        .unwrap();

    assert_eq!(
        rewritten,
        SymbolicValue::expression(
            ExpressionOperation::Add,
            replacement,
            SymbolicValue::Constant(2),
        )
    );
    assert!(rewritten.is_resolved());
}

#[test]
fn returning_direct_call_is_flattened_from_binary_symbols() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "parent".to_owned(),
        address: 0x1000,
        bytes: vec![
            0xef, 0x10, 0x00, 0x00, // jal ra, 0x2000
            0x13, 0x75, 0xf5, 0x0f, // andi a0, a0, 255
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let child = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "child".to_owned(),
        address: 0x2000,
        bytes: vec![
            0x13, 0x05, 0x05, 0x00, // mv a0, a0
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
        &direct::StructuralRelocatedCalls::new(),
        &StructuralPointerContext::default(),
        None,
        &map(),
        &mut visiting,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert_eq!(trace.reference_dependencies, ["child"]);
    assert_eq!(trace.return_value, SymbolicValue::input(0).and(0xff));
}

#[test]
fn direct_call_to_symbolic_cfg_callee_is_scoped_and_composed() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "branch_parent".to_owned(),
        address: 0x1000,
        bytes: vec![
            0xef, 0x10, 0x00, 0x00, // jal ra, 0x2000
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let child = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "branch_child".to_owned(),
        address: 0x2000,
        bytes: vec![
            0x63, 0x06, 0x05, 0x00, // beq a0, zero, 0x200c
            0x13, 0x05, 0x10, 0x00, // li a0, 1
            0x67, 0x80, 0x00, 0x00, // ret
            0x13, 0x05, 0x20, 0x00, // li a0, 2
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
        &direct::StructuralRelocatedCalls::new(),
        &StructuralPointerContext::default(),
        None,
        &map(),
        &mut visiting,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert!(matches!(
        trace.reference_events.as_slice(),
        [DraftReferenceEvent::ComposedCall {
            token: 0,
            symbol,
            result_modeled: true,
            ..
        }] if symbol == "branch_child"
    ));
    let generated = generate_reference(&trace, "oracle.elf", "abc123", None, &[]).unwrap();
    assert!(generated.source.contains("let call_result0 = {"));
    assert!(
        generated
            .source
            .contains("// Composed direct call: branch_child.")
    );
    assert!(generated.source.contains("if (call0_arg0"));
    assert!(
        generated
            .source
            .contains("ReferenceOutcome { exit_a0: Some(call_result0) }")
    );
    assert_generated_reference_compiles("scoped-callee", &generated.source);
}

#[test]
fn nested_call_graph_keeps_each_composed_token_scope_local() {
    let grandparent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "grandparent".to_owned(),
        address: 0x0800,
        bytes: vec![
            0x17, 0x03, 0x00, 0x00, // auipc t1, 0
            0xe7, 0x00, 0x03, 0x00, // jalr ra, 0(t1)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let parent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "branch_parent".to_owned(),
        address: 0x1000,
        bytes: vec![
            0xef, 0x10, 0x00, 0x00, // jal ra, 0x2000
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let child = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "branch_child".to_owned(),
        address: 0x2000,
        bytes: vec![
            0x63, 0x06, 0x05, 0x00, // beq a0, zero, 0x200c
            0x13, 0x05, 0x10, 0x00, // li a0, 1
            0x67, 0x80, 0x00, 0x00, // ret
            0x13, 0x05, 0x20, 0x00, // li a0, 2
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let symbols = BTreeMap::from([(0x1000, parent), (0x2000, child)]);
    let relocations = direct::StructuralRelocatedCalls::from([(
        StructuralCallSite::new(&grandparent, 0x0800),
        ("branch_parent".to_owned(), Some(0x1000)),
    )]);
    let mut visiting = BTreeSet::from([0x0800]);

    let trace = resolve_reference_trace(
        &grandparent,
        &symbols,
        &relocations,
        &StructuralPointerContext::default(),
        None,
        &map(),
        &mut visiting,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert_eq!(
        trace.reference_dependencies,
        ["branch_parent", "branch_child"]
    );
    let generated = generate_reference(&trace, "oracle.elf", "abc123", None, &[]).unwrap();
    assert_eq!(generated.source.matches("let call_result0 = {").count(), 2);
    assert_generated_reference_compiles("nested-call-scopes", &generated.source);
}

#[test]
fn caller_cfg_can_branch_on_a_composed_callee_result() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "branch_on_call_result".to_owned(),
        address: 0x1000,
        bytes: vec![
            0xef, 0x10, 0x00, 0x00, // jal ra, 0x2000
            0x63, 0x06, 0x05, 0x00, // beq a0, zero, 0x1010
            0x13, 0x05, 0x30, 0x00, // li a0, 3
            0x67, 0x80, 0x00, 0x00, // ret
            0x13, 0x05, 0x40, 0x00, // li a0, 4
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let child = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "branch_child".to_owned(),
        address: 0x2000,
        bytes: vec![
            0x63, 0x06, 0x05, 0x00, // beq a0, zero, 0x200c
            0x13, 0x05, 0x10, 0x00, // li a0, 1
            0x67, 0x80, 0x00, 0x00, // ret
            0x13, 0x05, 0x20, 0x00, // li a0, 2
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
        &direct::StructuralRelocatedCalls::new(),
        &StructuralPointerContext::default(),
        None,
        &map(),
        &mut visiting,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    let generated = generate_reference(&trace, "oracle.elf", "abc123", None, &[]).unwrap();
    assert!(generated.source.contains("let call_result0 = {"));
    assert!(generated.source.contains("if (call0_arg0"));
    assert!(generated.source.contains("if (call_result0"));
    assert!(
        generated
            .source
            .contains("ReferenceOutcome { exit_a0: Some(0x00000004_u32) }")
    );
    assert!(
        generated
            .source
            .contains("ReferenceOutcome { exit_a0: Some(0x00000003_u32) }")
    );
    assert_generated_reference_compiles("branch-on-call-result", &generated.source);
}

#[test]
fn caller_cfg_rejects_an_unmodeled_callee_result_used_as_a_condition() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "branch_on_void_call".to_owned(),
        address: 0x1000,
        bytes: vec![
            0xef, 0x10, 0x00, 0x00, // jal ra, 0x2000
            0x63, 0x06, 0x05, 0x00, // beq a0, zero, 0x1010
            0x13, 0x05, 0x30, 0x00, // li a0, 3
            0x67, 0x80, 0x00, 0x00, // ret
            0x13, 0x05, 0x40, 0x00, // li a0, 4
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let delay = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "ets_delay_us".to_owned(),
        address: 0x2000,
        bytes: vec![0x73, 0x00, 0x10, 0x00],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let symbols = BTreeMap::from([(0x2000, delay)]);
    let mut visiting = BTreeSet::from([0x1000]);

    let trace = resolve_reference_trace(
        &parent,
        &symbols,
        &direct::StructuralRelocatedCalls::new(),
        &synthetic_delay_pointer_context(),
        None,
        &map(),
        &mut visiting,
    )
    .unwrap();

    assert!(!trace.is_reference_eligible());
    assert!(trace.reference_blockers.iter().any(|blocker| {
        blocker.contains("composed call result is used without a modeled callee `a0`")
    }));
}

#[test]
fn caller_cfg_allows_an_unmodeled_callee_result_when_it_is_discarded() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "branch_with_void_call".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x63, 0x0a, 0x05, 0x00, // beq a0, zero, 0x1014
            0x17, 0x03, 0x00, 0x00, // auipc t1, 0
            0xe7, 0x00, 0x03, 0x00, // jalr ra, 0(t1)
            0x13, 0x05, 0x70, 0x00, // li a0, 7
            0x67, 0x80, 0x00, 0x00, // ret
            0x13, 0x05, 0x80, 0x00, // li a0, 8
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let delay = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "ets_delay_us".to_owned(),
        address: 0x2000,
        bytes: vec![0x73, 0x00, 0x10, 0x00],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let symbols = BTreeMap::from([(0x2000, delay)]);
    let relocations = direct::StructuralRelocatedCalls::from([(
        StructuralCallSite::new(&parent, 0x1004),
        ("ets_delay_us".to_owned(), Some(0x2000)),
    )]);
    let mut visiting = BTreeSet::from([0x1000]);

    let trace = resolve_reference_trace(
        &parent,
        &symbols,
        &relocations,
        &synthetic_delay_pointer_context(),
        None,
        &map(),
        &mut visiting,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    let generated = generate_reference(&trace, "oracle.elf", "abc123", None, &[]).unwrap();
    assert!(generated.source.contains("io.delay_micros("));
    assert!(!generated.source.contains("let call_result0 = {"));
    assert_generated_reference_compiles("discarded-call-result", &generated.source);
}

#[test]
fn relocated_returning_call_is_flattened_without_executing_auipc_jalr() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "relocated_parent".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x17, 0x03, 0x00, 0x00, // auipc t1, 0
            0xe7, 0x00, 0x03, 0x00, // jalr ra, 0(t1)
            0x13, 0x75, 0xf5, 0x0f, // andi a0, a0, 255
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let child = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "companion_child".to_owned(),
        address: 0x2000,
        bytes: vec![0x67, 0x80, 0x00, 0x00], // ret
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let symbols = BTreeMap::from([(0x2000, child)]);
    let relocations = direct::StructuralRelocatedCalls::from([(
        StructuralCallSite::new(&parent, 0x1000),
        ("companion_child".to_owned(), Some(0x2000)),
    )]);
    let mut visiting = BTreeSet::from([0x1000]);

    let trace = resolve_reference_trace(
        &parent,
        &symbols,
        &relocations,
        &StructuralPointerContext::default(),
        None,
        &map(),
        &mut visiting,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert_eq!(trace.reference_dependencies, ["companion_child"]);
    assert_eq!(trace.return_value, SymbolicValue::input(0).and(0xff));
}

#[test]
fn constant_size_memcpy_relocation_becomes_ordered_memory_effects() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: Some("memory.o".to_owned()),
        name: "copy_four".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x13, 0x06, 0x40, 0x00, // li a2, 4
            0x17, 0x03, 0x00, 0x00, // auipc t1, 0
            0xe7, 0x00, 0x03, 0x00, // jalr ra, 0(t1)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: false,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let relocations = direct::StructuralRelocatedCalls::from([(
        StructuralCallSite::new(&parent, 0x1004),
        ("memcpy".to_owned(), None),
    )]);
    let mut visiting = BTreeSet::from([0x1000]);

    let pointer_context = synthetic_delay_pointer_context();
    let trace = resolve_reference_trace(
        &parent,
        &BTreeMap::new(),
        &relocations,
        &pointer_context,
        None,
        &map(),
        &mut visiting,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert_eq!(trace.reference_events.len(), 8);
    assert!(trace.reference_events[..4].iter().all(|event| matches!(
        event,
        DraftReferenceEvent::Memory {
            access: MemoryAccess::Read,
            width: 8,
            ..
        }
    )));
    assert!(trace.reference_events[4..].iter().all(|event| matches!(
        event,
        DraftReferenceEvent::Memory {
            access: MemoryAccess::Write,
            width: 8,
            ..
        }
    )));
    assert_eq!(trace.return_value, SymbolicValue::input(0));
    let generated = generate_reference(&trace, "memory.o", "abc123", None, &[]).unwrap();
    assert_eq!(generated.source.matches("memory.read(8,").count(), 4);
    assert_eq!(generated.source.matches("memory.write(8,").count(), 4);
    assert_generated_reference_compiles("constant-memcpy", &generated.source);
}

#[test]
fn resolved_memset_relocation_preserves_standard_effects_and_return_pointer() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: Some("memory.o".to_owned()),
        name: "fill_three".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x13, 0x06, 0x30, 0x00, // li a2, 3
            0x17, 0x03, 0x00, 0x00, // auipc t1, 0
            0xe7, 0x00, 0x03, 0x00, // jalr ra, 0(t1)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: false,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let relocations = direct::StructuralRelocatedCalls::from([(
        StructuralCallSite::new(&parent, 0x1004),
        ("memset".to_owned(), Some(0x2000)),
    )]);
    let mut visiting = BTreeSet::from([0x1000]);

    let pointer_context = synthetic_delay_pointer_context();
    let trace = resolve_reference_trace(
        &parent,
        &BTreeMap::new(),
        &relocations,
        &pointer_context,
        None,
        &map(),
        &mut visiting,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert_eq!(trace.reference_events.len(), 3);
    for event in &trace.reference_events {
        let DraftReferenceEvent::Memory {
            access: MemoryAccess::Write,
            width: 8,
            value: Some(value),
            ..
        } = event
        else {
            panic!("expected one memset byte write");
        };
        assert_eq!(
            value.canonical(),
            SymbolicValue::input(1).and(0xff).canonical()
        );
    }
    assert_eq!(trace.return_value, SymbolicValue::input(0));
}

#[test]
fn reviewed_allocator_result_accepts_memset_and_controller_field_write() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: Some("controller.o".to_owned()),
        name: "allocate_controller_state".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x13, 0x05, 0x00, 0x01, // li a0, 16
            0x17, 0x03, 0x00, 0x00, // auipc t1, 0
            0xe7, 0x00, 0x03, 0x00, // jalr ra, 0(t1)
            0x13, 0x04, 0x05, 0x00, // mv s0, a0
            0x93, 0x05, 0x00, 0x00, // li a1, 0
            0x13, 0x06, 0x00, 0x01, // li a2, 16
            0x13, 0x05, 0x04, 0x00, // mv a0, s0
            0x17, 0x03, 0x00, 0x00, // auipc t1, 0
            0xe7, 0x00, 0x03, 0x00, // jalr ra, 0(t1)
            0x23, 0x26, 0x04, 0x00, // sw zero, 12(s0)
            0x13, 0x05, 0x04, 0x00, // mv a0, s0
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: false,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let relocations = direct::StructuralRelocatedCalls::from([
        (
            StructuralCallSite::new(&parent, 0x1004),
            ("test_malloc".to_owned(), None),
        ),
        (
            StructuralCallSite::new(&parent, 0x101c),
            ("memset".to_owned(), None),
        ),
    ]);
    let mut visiting = BTreeSet::from([parent.address as u32]);

    let trace = resolve_reference_trace(
        &parent,
        &BTreeMap::new(),
        &relocations,
        &synthetic_delay_pointer_context(),
        None,
        &map(),
        &mut visiting,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert!(matches!(
        trace.reference_events.first(),
        Some(DraftReferenceEvent::ModeledDirectCall { function, .. })
            if function.return_model == ExternalReturnModel::Allocated { size_argument: 0 }
    ));
    let writes = trace
        .reference_events
        .iter()
        .filter_map(|event| match event {
            DraftReferenceEvent::Memory {
                access: MemoryAccess::Write,
                address,
                ..
            } => Some(address),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(writes.len(), 17, "{trace:#?}");
    assert!(writes.iter().all(|address| matches!(
        address
            .memory_object_location_with_reads(&BTreeMap::new())
            .map(|location| location.root),
        Some(MemoryObjectRoot::Allocation { call_token: 0 })
    )));
    assert!(
        trace
            .reference_blockers
            .iter()
            .all(|blocker| { !blocker.contains("no writable byte-memory provenance") })
    );
}

#[test]
fn reviewed_allocator_result_accepts_memcpy_destination() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: Some("controller.o".to_owned()),
        name: "copy_controller_state".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x93, 0x84, 0x05, 0x00, // mv s1, a1
            0x13, 0x05, 0x40, 0x00, // li a0, 4
            0x17, 0x03, 0x00, 0x00, // auipc t1, 0
            0xe7, 0x00, 0x03, 0x00, // jalr ra, 0(t1)
            0x13, 0x04, 0x05, 0x00, // mv s0, a0
            0x13, 0x05, 0x04, 0x00, // mv a0, s0
            0x93, 0x85, 0x04, 0x00, // mv a1, s1
            0x13, 0x06, 0x40, 0x00, // li a2, 4
            0x17, 0x03, 0x00, 0x00, // auipc t1, 0
            0xe7, 0x00, 0x03, 0x00, // jalr ra, 0(t1)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: false,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let relocations = direct::StructuralRelocatedCalls::from([
        (
            StructuralCallSite::new(&parent, 0x1008),
            ("test_malloc".to_owned(), None),
        ),
        (
            StructuralCallSite::new(&parent, 0x1020),
            ("memcpy".to_owned(), None),
        ),
    ]);
    let mut visiting = BTreeSet::from([parent.address as u32]);

    let trace = resolve_reference_trace(
        &parent,
        &BTreeMap::new(),
        &relocations,
        &synthetic_delay_pointer_context(),
        None,
        &map(),
        &mut visiting,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert_eq!(
        trace
            .reference_events
            .iter()
            .filter(|event| matches!(
                event,
                DraftReferenceEvent::Memory {
                    access: MemoryAccess::Read,
                    width: 8,
                    ..
                }
            ))
            .count(),
        4
    );
    assert_eq!(
        trace
            .reference_events
            .iter()
            .filter(|event| matches!(
                event,
                DraftReferenceEvent::Memory {
                    access: MemoryAccess::Write,
                    width: 8,
                    ..
                }
            ))
            .count(),
        4
    );
}

#[test]
fn allocation_pointer_round_trips_through_exact_global_cell() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: Some("controller.o".to_owned()),
        name: "initialize_controller_state".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x93, 0x89, 0x05, 0x00, // mv s3, a1
            0x13, 0x05, 0x00, 0x01, // li a0, 16
            0x17, 0x03, 0x00, 0x00, // auipc t1, 0
            0xe7, 0x00, 0x03, 0x00, // jalr ra, 0(t1)
            0x93, 0x04, 0x05, 0x00, // mv s1, a0
            0x37, 0x04, 0x00, 0x00, // lui s0, %hi(controller_state)
            0x23, 0x20, 0x94, 0x00, // sw s1, %lo(controller_state)(s0)
            0x13, 0x85, 0x04, 0x00, // mv a0, s1
            0x93, 0x05, 0x00, 0x00, // li a1, 0
            0x13, 0x06, 0x00, 0x01, // li a2, 16
            0x17, 0x03, 0x00, 0x00, // auipc t1, 0
            0xe7, 0x00, 0x03, 0x00, // jalr ra, 0(t1)
            0x03, 0x29, 0x04, 0x00, // lw s2, %lo(controller_state)(s0)
            0x13, 0x05, 0x89, 0x00, // addi a0, s2, 8
            0x93, 0x85, 0x09, 0x00, // mv a1, s3
            0x13, 0x06, 0x40, 0x00, // li a2, 4
            0x17, 0x03, 0x00, 0x00, // auipc t1, 0
            0xe7, 0x00, 0x03, 0x00, // jalr ra, 0(t1)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: false,
        memory_regions: Default::default(),
        relocations: vec![
            artifact::SymbolRelocation {
                address: 0x1014,
                kind: artifact::RelocationKind::Hi20,
                symbol: "controller_state".to_owned(),
                addend: 0,
            },
            artifact::SymbolRelocation {
                address: 0x1018,
                kind: artifact::RelocationKind::Lo12S,
                symbol: "controller_state".to_owned(),
                addend: 0,
            },
            artifact::SymbolRelocation {
                address: 0x1030,
                kind: artifact::RelocationKind::Lo12I,
                symbol: "controller_state".to_owned(),
                addend: 0,
            },
        ],
    };
    let relocations = direct::StructuralRelocatedCalls::from([
        (
            StructuralCallSite::new(&parent, 0x1008),
            ("test_malloc".to_owned(), None),
        ),
        (
            StructuralCallSite::new(&parent, 0x1028),
            ("memset".to_owned(), None),
        ),
        (
            StructuralCallSite::new(&parent, 0x1040),
            ("memcpy".to_owned(), None),
        ),
    ]);
    let mut visiting = BTreeSet::from([parent.address as u32]);

    let trace = resolve_reference_trace(
        &parent,
        &BTreeMap::new(),
        &relocations,
        &synthetic_delay_pointer_context(),
        None,
        &map(),
        &mut visiting,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    let copied_offsets = trace
        .located_reference_events
        .iter()
        .filter_map(|located| match &located.event {
            DraftReferenceEvent::Memory {
                access: MemoryAccess::Write,
                address,
                ..
            } if located.site == 0x1040 => address
                .memory_object_location_with_reads(&BTreeMap::new())
                .and_then(|location| match location.root {
                    MemoryObjectRoot::Allocation { call_token: 0 } => Some(location.offset),
                    _ => None,
                }),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(copied_offsets, [8, 9, 10, 11], "{trace:#?}");
    assert!(trace.reference_blockers.iter().all(|blocker| {
        !blocker.contains("standard-memory-intrinsic at 0x1040")
            || !blocker.contains("no writable byte-memory provenance")
    }));
}

#[test]
fn overlapping_global_store_invalidates_allocation_pointer_cell() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: Some("controller.o".to_owned()),
        name: "overwrite_controller_pointer".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x13, 0x05, 0x00, 0x01, // li a0, 16
            0x17, 0x03, 0x00, 0x00, // auipc t1, 0
            0xe7, 0x00, 0x03, 0x00, // jalr ra, 0(t1)
            0x93, 0x04, 0x05, 0x00, // mv s1, a0
            0x37, 0x04, 0x06, 0x10, // lui s0, 0x10060
            0x23, 0x20, 0x94, 0x00, // sw s1, 0(s0)
            0x23, 0x11, 0x04, 0x00, // sh zero, 2(s0)
            0x03, 0x25, 0x04, 0x00, // lw a0, 0(s0)
            0x93, 0x05, 0x00, 0x00, // li a1, 0
            0x13, 0x06, 0x40, 0x00, // li a2, 4
            0x17, 0x03, 0x00, 0x00, // auipc t1, 0
            0xe7, 0x00, 0x03, 0x00, // jalr ra, 0(t1)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: vec![artifact::MemoryRegion {
            start: 0x1006_0000,
            length: 4,
            writable: true,
            name: "controller globals".to_owned(),
        }]
        .into(),
        relocations: Vec::new(),
    };
    let relocations = direct::StructuralRelocatedCalls::from([
        (
            StructuralCallSite::new(&parent, 0x1004),
            ("test_malloc".to_owned(), None),
        ),
        (
            StructuralCallSite::new(&parent, 0x1028),
            ("memset".to_owned(), None),
        ),
    ]);
    let mut visiting = BTreeSet::from([parent.address as u32]);
    let mut pointer_context = synthetic_delay_pointer_context();
    pointer_context.data_pointer_cells.insert(
        0x1006_0000,
        SymbolicValue::ExternalResult(UNINITIALIZED_ALLOCATION_EXTERNAL_RESULT_TOKEN_FLAG | 99),
    );

    let trace = resolve_reference_trace(
        &parent,
        &BTreeMap::new(),
        &relocations,
        &pointer_context,
        None,
        &map(),
        &mut visiting,
    )
    .unwrap();

    assert!(!trace.is_reference_eligible());
    assert!(trace.reference_blockers.iter().any(|blocker| {
        blocker.contains("standard-memory-intrinsic at 0x1028")
            && blocker.contains("no writable byte-memory provenance")
    }));
}

#[test]
fn unknown_alias_store_invalidates_allocation_pointer_cell() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: Some("controller.o".to_owned()),
        name: "ambiguous_controller_pointer".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x13, 0x09, 0x06, 0x00, // mv s2, a2
            0x13, 0x05, 0x00, 0x01, // li a0, 16
            0x17, 0x03, 0x00, 0x00, // auipc t1, 0
            0xe7, 0x00, 0x03, 0x00, // jalr ra, 0(t1)
            0x93, 0x04, 0x05, 0x00, // mv s1, a0
            0x37, 0x04, 0x06, 0x10, // lui s0, 0x10060
            0x23, 0x20, 0x94, 0x00, // sw s1, 0(s0)
            0x23, 0x20, 0x09, 0x00, // sw zero, 0(s2)
            0x03, 0x25, 0x04, 0x00, // lw a0, 0(s0)
            0x93, 0x05, 0x00, 0x00, // li a1, 0
            0x13, 0x06, 0x40, 0x00, // li a2, 4
            0x17, 0x03, 0x00, 0x00, // auipc t1, 0
            0xe7, 0x00, 0x03, 0x00, // jalr ra, 0(t1)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: vec![artifact::MemoryRegion {
            start: 0x1006_0000,
            length: 4,
            writable: true,
            name: "controller globals".to_owned(),
        }]
        .into(),
        relocations: Vec::new(),
    };
    let relocations = direct::StructuralRelocatedCalls::from([
        (
            StructuralCallSite::new(&parent, 0x1008),
            ("test_malloc".to_owned(), None),
        ),
        (
            StructuralCallSite::new(&parent, 0x102c),
            ("memset".to_owned(), None),
        ),
    ]);
    let mut visiting = BTreeSet::from([parent.address as u32]);

    let trace = resolve_reference_trace(
        &parent,
        &BTreeMap::new(),
        &relocations,
        &synthetic_delay_pointer_context(),
        None,
        &map(),
        &mut visiting,
    )
    .unwrap();

    assert!(!trace.is_reference_eligible());
    assert!(trace.reference_blockers.iter().any(|blocker| {
        blocker.contains("standard-memory-intrinsic at 0x102c")
            && blocker.contains("no writable byte-memory provenance")
    }));
}

#[test]
fn opaque_pointer_stored_in_global_cell_is_not_fresh_allocation() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: Some("controller.o".to_owned()),
        name: "opaque_controller_pointer".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x13, 0x05, 0x40, 0x00, // li a0, 4
            0x17, 0x03, 0x00, 0x00, // auipc t1, 0
            0xe7, 0x00, 0x03, 0x00, // jalr ra, 0(t1)
            0x93, 0x04, 0x05, 0x00, // mv s1, a0
            0x37, 0x04, 0x06, 0x10, // lui s0, 0x10060
            0x23, 0x20, 0x94, 0x00, // sw s1, 0(s0)
            0x03, 0x25, 0x04, 0x00, // lw a0, 0(s0)
            0x93, 0x05, 0x00, 0x00, // li a1, 0
            0x13, 0x06, 0x40, 0x00, // li a2, 4
            0x17, 0x03, 0x00, 0x00, // auipc t1, 0
            0xe7, 0x00, 0x03, 0x00, // jalr ra, 0(t1)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: vec![artifact::MemoryRegion {
            start: 0x1006_0000,
            length: 4,
            writable: true,
            name: "controller globals".to_owned(),
        }]
        .into(),
        relocations: Vec::new(),
    };
    let relocations = direct::StructuralRelocatedCalls::from([
        (
            StructuralCallSite::new(&parent, 0x1004),
            ("test_opaque_pointer".to_owned(), None),
        ),
        (
            StructuralCallSite::new(&parent, 0x1024),
            ("memset".to_owned(), None),
        ),
    ]);
    let mut visiting = BTreeSet::from([parent.address as u32]);

    let trace = resolve_reference_trace(
        &parent,
        &BTreeMap::new(),
        &relocations,
        &synthetic_delay_pointer_context(),
        None,
        &map(),
        &mut visiting,
    )
    .unwrap();

    assert!(!trace.is_reference_eligible());
    assert!(trace.reference_blockers.iter().any(|blocker| {
        blocker.contains("standard-memory-intrinsic at 0x1024")
            && blocker.contains("no writable byte-memory provenance")
    }));
}

#[test]
fn non_allocator_symbolic_return_does_not_gain_writable_provenance() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: Some("controller.o".to_owned()),
        name: "opaque_result_is_not_storage".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x13, 0x05, 0x80, 0x00, // li a0, 8
            0x17, 0x03, 0x00, 0x00, // auipc t1, 0
            0xe7, 0x00, 0x03, 0x00, // jalr ra, 0(t1)
            0x13, 0x04, 0x05, 0x00, // mv s0, a0
            0x93, 0x05, 0x00, 0x00, // li a1, 0
            0x13, 0x06, 0x80, 0x00, // li a2, 8
            0x13, 0x05, 0x04, 0x00, // mv a0, s0
            0x17, 0x03, 0x00, 0x00, // auipc t1, 0
            0xe7, 0x00, 0x03, 0x00, // jalr ra, 0(t1)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: false,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let relocations = direct::StructuralRelocatedCalls::from([
        (
            StructuralCallSite::new(&parent, 0x1004),
            ("test_opaque_result".to_owned(), None),
        ),
        (
            StructuralCallSite::new(&parent, 0x101c),
            ("memset".to_owned(), None),
        ),
    ]);
    let mut visiting = BTreeSet::from([parent.address as u32]);

    let trace = resolve_reference_trace(
        &parent,
        &BTreeMap::new(),
        &relocations,
        &synthetic_delay_pointer_context(),
        None,
        &map(),
        &mut visiting,
    )
    .unwrap();

    assert!(!trace.is_reference_eligible());
    assert!(trace.reference_blockers.iter().any(|blocker| {
        blocker.contains("standard-memory-intrinsic at 0x101c")
            && blocker.contains("external-result:0")
            && blocker.contains("no writable byte-memory provenance")
    }));
}

#[test]
fn dynamic_size_memcpy_relocation_remains_fail_closed() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: Some("memory.o".to_owned()),
        name: "copy_dynamic".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x17, 0x03, 0x00, 0x00, // auipc t1, 0
            0xe7, 0x00, 0x03, 0x00, // jalr ra, 0(t1)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: false,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let relocations = direct::StructuralRelocatedCalls::from([(
        StructuralCallSite::new(&parent, 0x1000),
        ("memcpy".to_owned(), None),
    )]);
    let mut visiting = BTreeSet::from([0x1000]);

    let pointer_context = synthetic_delay_pointer_context();
    let trace = resolve_reference_trace(
        &parent,
        &BTreeMap::new(),
        &relocations,
        &pointer_context,
        None,
        &map(),
        &mut visiting,
    )
    .unwrap();

    assert!(!trace.is_reference_eligible());
    assert!(trace.reference_blockers.iter().any(|blocker| {
        blocker.contains("standard-memory-intrinsic")
            && blocker.contains("memcpy length is not constant")
    }));
}

#[test]
fn unresolved_call_relocation_fails_closed() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "unresolved_parent".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x17, 0x03, 0x00, 0x00, // auipc t1, 0
            0x67, 0x00, 0x03, 0x00, // jalr zero, 0(t1)
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let relocations = direct::StructuralRelocatedCalls::from([(
        StructuralCallSite::new(&parent, 0x1000),
        ("missing_child".to_owned(), None),
    )]);

    let trace = trace_binary_symbol(
        &parent,
        &map(),
        &relocations,
        &StructuralPointerContext::default(),
        None,
    )
    .unwrap();

    assert!(!trace.is_reference_eligible());
    assert!(
        trace
            .reference_blockers
            .iter()
            .any(|blocker| blocker.contains("unresolved-call-relocation"))
    );
}

#[test]
fn unresolved_returning_relocation_continues_with_abi_clobbers() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "unresolved_returning_parent".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x13, 0x04, 0x05, 0x00, // mv s0, a0
            0x17, 0x03, 0x00, 0x00, // auipc t1, 0
            0xe7, 0x00, 0x03, 0x00, // jalr ra, 0(t1)
            0x23, 0x22, 0xb4, 0x00, // sw a1, 4(s0)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let relocations = direct::StructuralRelocatedCalls::from([(
        StructuralCallSite::new(&parent, 0x1004),
        ("missing_child".to_owned(), None),
    )]);

    let trace = trace_binary_symbol(
        &parent,
        &map(),
        &relocations,
        &StructuralPointerContext::default(),
        None,
    )
    .unwrap();

    assert!(!trace.is_reference_eligible());
    assert!(
        trace
            .reference_blockers
            .iter()
            .any(|blocker| blocker.contains("unresolved-call-relocation"))
    );
    let DraftReferenceEvent::Memory {
        access: MemoryAccess::Write,
        address,
        value,
        ..
    } = &trace.reference_events[0]
    else {
        panic!("expected the post-call context write");
    };
    assert!(address.canonical().contains("arg0"));
    assert_eq!(value, &Some(SymbolicValue::Unknown));
}

#[test]
fn unresolved_indirect_returning_call_continues_with_abi_clobbers() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "unresolved_indirect_parent".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x13, 0x04, 0x05, 0x00, // mv s0, a0
            0xe7, 0x00, 0x03, 0x00, // jalr ra, 0(t1)
            0x23, 0x22, 0xb4, 0x00, // sw a1, 4(s0)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };

    let trace = trace_binary_symbol(
        &parent,
        &map(),
        &direct::StructuralRelocatedCalls::new(),
        &StructuralPointerContext::default(),
        None,
    )
    .unwrap();

    assert!(!trace.is_reference_eligible());
    assert!(
        trace
            .reference_blockers
            .iter()
            .any(|blocker| blocker.contains("unresolved-indirect-call"))
    );
    let DraftReferenceEvent::Memory {
        access: MemoryAccess::Write,
        address,
        value,
        ..
    } = &trace.reference_events[0]
    else {
        panic!("expected the post-call context write");
    };
    assert!(address.canonical().contains("arg0"));
    assert_eq!(value, &Some(SymbolicValue::Unknown));
}

#[test]
fn linked_diagnostic_symbol_remains_a_modeled_boundary() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "linked_diagnostic_parent".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x97, 0x00, 0x00, 0x00, // auipc ra, 0
            0xe7, 0x80, 0x00, 0x00, // jalr ra, 0(ra)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let relocations = direct::StructuralRelocatedCalls::from([(
        StructuralCallSite::new(&parent, 0x1000),
        ("wifi_log".to_owned(), Some(0x2f80_0040)),
    )]);
    let mut context = StructuralPointerContext::default();
    context.diagnostic_calls.insert("wifi_log".to_owned(), 2);

    let trace = trace_binary_symbol(&parent, &map(), &relocations, &context, None).unwrap();

    assert!(matches!(
        trace.reference_events.as_slice(),
        [DraftReferenceEvent::DiagnosticCall {
            site: 0x1000,
            function,
            argument_count: 2,
            arguments,
        }] if function == "wifi_log" && arguments.len() == 2
    ));
}

#[test]
fn linked_tail_diagnostic_symbol_terminates_without_a_link_register_blocker() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "linked_tail_diagnostic_parent".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x6f, 0x00, 0x00, 0x00, // j wifi_log
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let relocations = direct::StructuralRelocatedCalls::from([(
        StructuralCallSite::new(&parent, 0x1000),
        ("wifi_log".to_owned(), Some(0x2f80_0040)),
    )]);
    let mut context = StructuralPointerContext::default();
    context.diagnostic_calls.insert("wifi_log".to_owned(), 2);

    let trace = trace_binary_symbol(&parent, &map(), &relocations, &context, None).unwrap();

    assert!(
        trace.reference_blockers.is_empty(),
        "{:#?}",
        trace.reference_blockers
    );
    assert!(matches!(
        trace.reference_events.as_slice(),
        [DraftReferenceEvent::DiagnosticCall {
            site: 0x1000,
            function,
            argument_count: 2,
            arguments,
        }] if function == "wifi_log" && arguments.len() == 2
    ));
}

#[test]
fn modeled_direct_platform_call_propagates_constant_result() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "fixed_xtal_parent".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x97, 0x00, 0x00, 0x00, // auipc ra, 0
            0xe7, 0x80, 0x00, 0x00, // jalr ra, 0(ra)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let relocations = direct::StructuralRelocatedCalls::from([(
        StructuralCallSite::new(&parent, 0x1000),
        ("rtc_clk_xtal_freq_get".to_owned(), None),
    )]);
    let context = synthetic_delay_pointer_context();

    let trace = trace_binary_symbol(&parent, &map(), &relocations, &context, None).unwrap();

    assert!(trace.blockers.is_empty(), "{:#?}", trace.blockers);
    assert!(
        trace.reference_blockers.is_empty(),
        "{:#?}",
        trace.reference_blockers
    );
    assert_eq!(trace.return_value, SymbolicValue::Constant(40));
    assert!(matches!(
        trace.reference_events.as_slice(),
        [DraftReferenceEvent::ModeledDirectCall { function, .. }]
            if function.name == "rtc_clk_xtal_freq_get"
                && function.return_model == ExternalReturnModel::Constant(40)
    ));
}

#[test]
fn modeled_direct_wide_runtime_call_propagates_both_return_words() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "wide_runtime_parent".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x97, 0x00, 0x00, 0x00, // auipc ra, 0
            0xe7, 0x80, 0x00, 0x00, // jalr ra, 0(ra)
            0x13, 0x85, 0x05, 0x00, // mv a0, a1
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let relocations = direct::StructuralRelocatedCalls::from([(
        StructuralCallSite::new(&parent, 0x1000),
        ("__umoddi3".to_owned(), None),
    )]);
    let context = synthetic_delay_pointer_context();

    let trace = trace_binary_symbol(&parent, &map(), &relocations, &context, None).unwrap();

    assert!(trace.blockers.is_empty(), "{:#?}", trace.blockers);
    assert!(
        trace.reference_blockers.is_empty(),
        "{:#?}",
        trace.reference_blockers
    );
    assert_eq!(trace.return_value, SymbolicValue::ExternalResultHigh(0));
    assert!(matches!(
        trace.reference_events.as_slice(),
        [DraftReferenceEvent::ModeledDirectCall { function, arguments, .. }]
            if function.name == "__umoddi3"
                && function.return_model == ExternalReturnModel::SymbolicU64
                && arguments.len() == 4
    ));
}

#[test]
fn reviewed_indirect_call_keeps_abi_identity_without_claiming_execution_semantics() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "reviewed_indirect_parent".to_owned(),
        address: 0x1000,
        bytes: vec![
            0xe7, 0x00, 0x03, 0x00, // jalr ra, 0(t1)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let mut context = StructuralPointerContext::default();
    context.reviewed_external_calls.insert(
        StructuralCallSite::new(&parent, 0x1000),
        vec![ReviewedExternalCall {
            id: "pack::wifi-osi@+0x40".to_owned(),
            contract: "pack::wifi-osi".to_owned(),
            name: "semphr_give".to_owned(),
            argument_types: vec!["opaque-handle".to_owned()],
            return_type: "i32".to_owned(),
            variadic: false,
            semantic_operation: None,
            replacement_hint: None,
            execution_model: None,
            tail: false,
            evidence: ReviewedExternalCallEvidence::ObservedCallSite,
            slot_load_site: None,
        }],
    );

    let trace = trace_binary_symbol(
        &parent,
        &map(),
        &direct::StructuralRelocatedCalls::new(),
        &context,
        None,
    )
    .unwrap();

    assert!(trace.blockers.is_empty(), "{trace:#?}");
    assert!(
        trace
            .reference_blockers
            .iter()
            .any(|blocker| blocker.contains("unmodeled-reviewed-external-call"))
    );
    assert!(!trace.reference_blockers.iter().any(|blocker| {
        blocker.contains("unresolved-indirect-call")
            || blocker.contains("unregistered-external-abi-slot")
    }));
    assert!(matches!(
        trace.reference_events.as_slice(),
        [DraftReferenceEvent::ReviewedExternalCall {
            site: 0x1000,
            candidates,
            arguments,
            ..
        }] if candidates[0].name == "semphr_give" && arguments.len() == 1
    ));
}

#[test]
fn alternative_reviewed_calls_with_the_same_abi_and_behavior_share_one_model() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "alternative_allocators".to_owned(),
        address: 0x1000,
        bytes: vec![
            0xe7, 0x00, 0x03, 0x00, // jalr ra, 0(t1)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let candidate = |id: &str, name: &str, model: &str| ReviewedExternalCall {
        id: id.to_owned(),
        contract: "pack::wifi-osi".to_owned(),
        name: name.to_owned(),
        argument_types: vec!["usize".to_owned()],
        return_type: "mut-ptr".to_owned(),
        variadic: false,
        semantic_operation: None,
        replacement_hint: None,
        execution_model: Some(ReviewedExternalCallExecutionModel {
            id: model.to_owned(),
            return_model: ExternalReturnModel::OpaquePointer,
            outputs: Vec::new(),
        }),
        tail: false,
        evidence: ReviewedExternalCallEvidence::ObservedCallSite,
        slot_load_site: None,
    };
    let mut context = StructuralPointerContext::default();
    context.reviewed_external_calls.insert(
        StructuralCallSite::new(&parent, 0x1000),
        vec![
            candidate(
                "pack::wifi-osi@+0x158",
                "malloc_internal",
                "malloc-internal",
            ),
            candidate("pack::wifi-osi@+0x168", "wifi_malloc", "wifi-malloc"),
        ],
    );

    let trace = trace_binary_symbol(
        &parent,
        &map(),
        &direct::StructuralRelocatedCalls::new(),
        &context,
        None,
    )
    .unwrap();

    assert!(trace.blockers.is_empty(), "{trace:#?}");
    assert!(trace.reference_blockers.is_empty(), "{trace:#?}");
    assert!(matches!(
        trace.return_value,
        SymbolicValue::ExternalResult(token)
            if token & OPAQUE_POINTER_EXTERNAL_RESULT_TOKEN_FLAG != 0
    ));
    assert!(matches!(
        trace.reference_events.as_slice(),
        [DraftReferenceEvent::ReviewedExternalCall { candidates, .. }]
            if candidates.len() == 2
    ));
}

#[test]
fn projected_relaxed_pointer_load_recovers_reviewed_table_call_and_arguments() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "linked_parent".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x83, 0x27, 0x00, 0x00, // lw a5, 0(zero), relaxed g_osi_funcs_p
            0x03, 0xa3, 0x07, 0x09, // lw t1, 0x90(a5)
            0x13, 0x05, 0x70, 0x00, // li a0, 7
            0xe7, 0x00, 0x03, 0x00, // jalr ra, 0(t1)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let contract = "pack::wifi-osi".to_owned();
    let mut context = StructuralPointerContext::default();
    context.relocated_pointer_symbols.insert(
        "g_osi_funcs_p".to_owned(),
        SymbolicValue::ReviewedExternalTable(contract.clone()),
    );
    context.projected_relocations.insert(
        StructuralCallSite::new(&parent, 0x1000),
        vec![StructuralProjectedRelocation {
            origin_member: Some("pp.o".to_owned()),
            origin_symbol: "linked_parent".to_owned(),
            origin_offsets: vec![0, 4],
            kind: artifact::RelocationKind::Lo12I,
            symbol: "g_osi_funcs_p".to_owned(),
            addend: 0,
            correspondence: "linker-relaxation",
        }],
    );
    context.reviewed_external_slots.insert(
        (contract.clone(), 0x90),
        vec![ReviewedExternalCall {
            id: "pack::wifi-osi@+0x90".to_owned(),
            contract,
            name: "task_create_pinned_to_core".to_owned(),
            argument_types: vec!["u32".to_owned()],
            return_type: "i32".to_owned(),
            variadic: false,
            semantic_operation: Some("rtos.task.create-pinned".to_owned()),
            replacement_hint: None,
            execution_model: None,
            tail: false,
            evidence: ReviewedExternalCallEvidence::ArchiveOriginProjection,
            slot_load_site: Some(0x1004),
        }],
    );

    let trace = trace_binary_symbol(
        &parent,
        &map(),
        &direct::StructuralRelocatedCalls::new(),
        &context,
        None,
    )
    .unwrap();

    assert!(!trace.reference_blockers.iter().any(|blocker| {
        blocker.contains("unresolved-indirect-call")
            || blocker.contains("unmodeled-memory-load")
            || blocker.contains("unregistered-external-abi-slot")
    }));
    assert!(matches!(
        trace.reference_events.as_slice(),
        [DraftReferenceEvent::ReviewedExternalCall {
            site: 0x100c,
            candidates,
            arguments,
            ..
        }] if candidates[0].name == "task_create_pinned_to_core"
            && arguments == &Box::from([SymbolicValue::Constant(7)])
    ));
}

#[test]
fn observed_slot_assignment_promotes_reviewed_indirect_call_to_internal_code() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "linked_parent".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x83, 0x27, 0x00, 0x00, // lw a5, 0(zero), relaxed pointer cell
            0x03, 0xa3, 0x47, 0x08, // lw t1, 0x84(a5)
            0x13, 0x05, 0x70, 0x00, // li a0, 7
            0x67, 0x00, 0x03, 0x00, // jalr zero, 0(t1)
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let contract = "pack::net80211".to_owned();
    let mut context = StructuralPointerContext::default();
    context.relocated_pointer_symbols.insert(
        "net80211_funcs".to_owned(),
        SymbolicValue::ReviewedExternalTable(contract.clone()),
    );
    context.projected_relocations.insert(
        StructuralCallSite::new(&parent, 0x1000),
        vec![StructuralProjectedRelocation {
            origin_member: Some("consumer.o".to_owned()),
            origin_symbol: "linked_parent".to_owned(),
            origin_offsets: vec![0, 4],
            kind: artifact::RelocationKind::Lo12I,
            symbol: "net80211_funcs".to_owned(),
            addend: 0,
            correspondence: "linker-relaxation",
        }],
    );
    context.reviewed_external_slots.insert(
        (contract.clone(), 0x84),
        vec![ReviewedExternalCall {
            id: "pack::net80211@+0x84".to_owned(),
            contract: contract.clone(),
            name: "hostap_input".to_owned(),
            argument_types: vec!["u32".to_owned()],
            return_type: "void".to_owned(),
            variadic: false,
            semantic_operation: None,
            replacement_hint: None,
            execution_model: None,
            tail: true,
            evidence: ReviewedExternalCallEvidence::ArchiveOriginProjection,
            slot_load_site: Some(0x1004),
        }],
    );
    context
        .reviewed_internal_slots
        .insert((contract, 0x84), 0x2000);

    let trace = trace_binary_symbol(
        &parent,
        &map(),
        &direct::StructuralRelocatedCalls::new(),
        &context,
        None,
    )
    .unwrap();

    assert!(
        trace.reference_blockers.is_empty(),
        "{:#?}",
        trace.reference_blockers
    );
    assert!(matches!(
        trace.reference_events.as_slice(),
        [DraftReferenceEvent::TailCall {
            site: 0x100c,
            target: 0x2000,
            arguments,
            ..
        }] if arguments[0] == SymbolicValue::Constant(7)
    ));
}

#[test]
fn reviewed_void_external_call_is_an_executable_boundary_without_a_fake_result() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "reviewed_void_parent".to_owned(),
        address: 0x1000,
        bytes: vec![
            0xe7, 0x00, 0x03, 0x00, // jalr ra, 0(t1)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let mut context = StructuralPointerContext::default();
    context.reviewed_external_calls.insert(
        StructuralCallSite::new(&parent, 0x1000),
        vec![ReviewedExternalCall {
            id: "pack::services@+0x10".to_owned(),
            contract: "pack::services".to_owned(),
            name: "critical_exit".to_owned(),
            argument_types: vec!["u32".to_owned()],
            return_type: "void".to_owned(),
            variadic: false,
            semantic_operation: Some("critical-section.exit".to_owned()),
            replacement_hint: None,
            execution_model: Some(ReviewedExternalCallExecutionModel {
                id: "critical-exit".to_owned(),
                return_model: ExternalReturnModel::Void,
                outputs: Vec::new(),
            }),
            tail: false,
            evidence: ReviewedExternalCallEvidence::ObservedCallSite,
            slot_load_site: None,
        }],
    );

    let trace = trace_binary_symbol(
        &parent,
        &map(),
        &direct::StructuralRelocatedCalls::new(),
        &context,
        None,
    )
    .unwrap();

    assert!(trace.blockers.is_empty(), "{trace:#?}");
    assert!(trace.reference_blockers.is_empty(), "{trace:#?}");
    assert_eq!(trace.return_value, SymbolicValue::Unknown);
    assert!(matches!(
        trace.reference_events.as_slice(),
        [DraftReferenceEvent::ReviewedExternalCall { candidates, .. }]
            if candidates[0].execution_model.as_ref().is_some_and(|model|
                model.return_model == ExternalReturnModel::Void)
    ));
}

#[test]
fn reviewed_external_call_keeps_return_and_private_stack_output_independent() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "reviewed_return_and_output".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x13, 0x01, 0x01, 0xff, // addi sp, sp, -16
            0x13, 0x06, 0x01, 0x00, // mv a2, sp
            0xe7, 0x00, 0x03, 0x00, // jalr ra, 0(t1)
            0x83, 0x45, 0x01, 0x00, // lbu a1, 0(sp)
            0x33, 0x05, 0xb5, 0x00, // add a0, a0, a1
            0x13, 0x01, 0x01, 0x01, // addi sp, sp, 16
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let mut context = StructuralPointerContext::default();
    context.reviewed_external_calls.insert(
        StructuralCallSite::new(&parent, 0x1008),
        vec![ReviewedExternalCall {
            id: "pack::services@+0x68".to_owned(),
            contract: "pack::services".to_owned(),
            name: "queue_send_from_isr".to_owned(),
            argument_types: vec![
                "opaque-handle".to_owned(),
                "mut-ptr".to_owned(),
                "out-ptr".to_owned(),
            ],
            return_type: "i32".to_owned(),
            variadic: false,
            semantic_operation: Some("rtos.queue.send-from-isr".to_owned()),
            replacement_hint: None,
            execution_model: Some(ReviewedExternalCallExecutionModel {
                id: "queue-send-from-isr".to_owned(),
                return_model: ExternalReturnModel::SymbolicU32,
                outputs: vec![ExternalOutputModel::PrivateStack {
                    pointer_argument: 2,
                    width: 8,
                }],
            }),
            tail: false,
            evidence: ReviewedExternalCallEvidence::ObservedCallSite,
            slot_load_site: None,
        }],
    );

    let trace = trace_binary_symbol(
        &parent,
        &map(),
        &direct::StructuralRelocatedCalls::new(),
        &context,
        None,
    )
    .unwrap();

    assert!(trace.blockers.is_empty(), "{trace:#?}");
    assert!(trace.reference_blockers.is_empty(), "{trace:#?}");
    assert!(matches!(
        &trace.return_value,
        SymbolicValue::Expression {
            operation: ExpressionOperation::Add,
            left,
            right,
            ..
        } if **left == SymbolicValue::ExternalResult(0)
            && matches!(right.bits()[0], BitSource::PrivateStack { read_token: 0, bit: 0, .. })
    ));
    assert!(matches!(
        trace.reference_events.as_slice(),
        [
            DraftReferenceEvent::ReviewedExternalCall { token: 0, .. },
            DraftReferenceEvent::PrivateStackStore { width: 8, .. },
            DraftReferenceEvent::PrivateStackLoad { width: 8, .. },
        ]
    ));
    let DraftReferenceEvent::PrivateStackStore { value, .. } = &trace.reference_events[1] else {
        unreachable!("event shape was asserted above")
    };
    assert!(matches!(
        value.bits()[0],
        BitSource::ExternalOutput {
            call_token: 0,
            output_index: 0,
            bit: 0,
            inverted: false,
        }
    ));
    let mut visiting = BTreeSet::from([parent.address as u32]);
    let flattened = resolve_reference_trace(
        &parent,
        &BTreeMap::new(),
        &direct::StructuralRelocatedCalls::new(),
        &context,
        None,
        &map(),
        &mut visiting,
    )
    .unwrap();
    let generated = generate_reference(&flattened, "fixture.elf", "digest", None, &[]).unwrap();
    assert!(
        generated
            .source
            .contains("let external_result0 = external_outcome0.return_words[0];")
    );
    assert!(
        generated
            .source
            .contains("let external_output0_0 = external_outcome0.outputs[0] & 0x000000ff_u32;")
    );
}

#[test]
fn reviewed_external_u64_result_keeps_a0_and_a1_independent() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "reviewed_u64_result".to_owned(),
        address: 0x1000,
        bytes: vec![
            0xe7, 0x00, 0x03, 0x00, // jalr ra, 0(t1)
            0x33, 0x45, 0xb5, 0x00, // xor a0, a0, a1
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let mut context = StructuralPointerContext::default();
    context.reviewed_external_calls.insert(
        StructuralCallSite::new(&parent, 0x1000),
        vec![ReviewedExternalCall {
            id: "pack::services@+0x108".to_owned(),
            contract: "pack::services".to_owned(),
            name: "esp_timer_get_time".to_owned(),
            argument_types: Vec::new(),
            return_type: "i64".to_owned(),
            variadic: false,
            semantic_operation: Some("time.monotonic-micros.get".to_owned()),
            replacement_hint: None,
            execution_model: Some(ReviewedExternalCallExecutionModel {
                id: "esp-timer-get-time".to_owned(),
                return_model: ExternalReturnModel::SymbolicU64,
                outputs: Vec::new(),
            }),
            tail: false,
            evidence: ReviewedExternalCallEvidence::ObservedCallSite,
            slot_load_site: None,
        }],
    );

    let trace = trace_binary_symbol(
        &parent,
        &map(),
        &direct::StructuralRelocatedCalls::new(),
        &context,
        None,
    )
    .unwrap();

    assert!(trace.blockers.is_empty(), "{trace:#?}");
    assert!(trace.reference_blockers.is_empty(), "{trace:#?}");
    assert_eq!(
        trace.return_value,
        SymbolicValue::expression(
            ExpressionOperation::BitXor,
            SymbolicValue::ExternalResult(0),
            SymbolicValue::ExternalResultHigh(0),
        )
    );
    let generated = generate_reference(&trace, "fixture.elf", "digest", None, &[]).unwrap();
    assert!(
        generated
            .source
            .contains("let external_result0 = external_outcome0.return_words[0];")
    );
    assert!(
        generated
            .source
            .contains("let external_result0_high = external_outcome0.return_words[1];")
    );
}
