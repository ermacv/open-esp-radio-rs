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
        &BTreeMap::new(),
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
        &BTreeMap::new(),
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
            .contains("ReferenceOutcome { exit_a0: Some(call_result0 & 0xffffffff_u32) }")
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
    let relocations = BTreeMap::from([(
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
        &BTreeMap::new(),
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
        &BTreeMap::new(),
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
    let relocations = BTreeMap::from([(
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
    assert!(
        generated
            .source
            .contains("// Composed direct call: ets_delay_us.")
    );
    assert!(generated.source.contains("let call0_arg0 ="));
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
    let relocations = BTreeMap::from([(
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
    let relocations = BTreeMap::from([(
        StructuralCallSite::new(&parent, 0x1004),
        ("memcpy".to_owned(), None),
    )]);
    let mut visiting = BTreeSet::from([0x1000]);

    let trace = resolve_reference_trace(
        &parent,
        &BTreeMap::new(),
        &relocations,
        &StructuralPointerContext::default(),
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
fn constant_size_memset_relocation_preserves_byte_and_return_pointer() {
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
    let relocations = BTreeMap::from([(
        StructuralCallSite::new(&parent, 0x1004),
        ("memset".to_owned(), None),
    )]);
    let mut visiting = BTreeSet::from([0x1000]);

    let trace = resolve_reference_trace(
        &parent,
        &BTreeMap::new(),
        &relocations,
        &StructuralPointerContext::default(),
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
    let relocations = BTreeMap::from([(
        StructuralCallSite::new(&parent, 0x1000),
        ("memcpy".to_owned(), None),
    )]);
    let mut visiting = BTreeSet::from([0x1000]);

    let trace = resolve_reference_trace(
        &parent,
        &BTreeMap::new(),
        &relocations,
        &StructuralPointerContext::default(),
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
    let relocations = BTreeMap::from([(
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
    let relocations = BTreeMap::from([(
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
        &BTreeMap::new(),
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
    let relocations = BTreeMap::from([(
        StructuralCallSite::new(&parent, 0x1000),
        ("wifi_log".to_owned(), Some(0x2f80_0040)),
    )]);
    let mut context = StructuralPointerContext::default();
    context.diagnostic_calls.insert("wifi_log".to_owned(), 2);

    let trace = trace_binary_symbol(&parent, &map(), &relocations, &context, None).unwrap();

    assert!(matches!(
        trace.reference_events.as_slice(),
        [DraftReferenceEvent::DiagnosticCall {
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
    let relocations = BTreeMap::from([(
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

    let trace = trace_binary_symbol(&parent, &map(), &BTreeMap::new(), &context, None).unwrap();

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

    let trace = trace_binary_symbol(&parent, &map(), &BTreeMap::new(), &context, None).unwrap();

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
                outputs: vec![ExternalOutputModel::PrivateStackU8 {
                    pointer_argument: 2,
                }],
            }),
            tail: false,
            evidence: ReviewedExternalCallEvidence::ObservedCallSite,
            slot_load_site: None,
        }],
    );

    let trace = trace_binary_symbol(&parent, &map(), &BTreeMap::new(), &context, None).unwrap();

    assert!(trace.blockers.is_empty(), "{trace:#?}");
    assert!(trace.reference_blockers.is_empty(), "{trace:#?}");
    assert!(matches!(
        &trace.return_value,
        SymbolicValue::Expression {
            operation: ExpressionOperation::Add,
            left,
            right,
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
        &BTreeMap::new(),
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
            .contains("let external_output0_0 = external_outcome0.outputs[0] & 0xff_u32;")
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

    let trace = trace_binary_symbol(&parent, &map(), &BTreeMap::new(), &context, None).unwrap();

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
