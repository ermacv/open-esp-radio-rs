//! Structured pseudo flow, context projection, and MMIO field discovery.

use super::*;
use crate::LocatedReferenceEvent;

#[test]
fn pseudo_ir_keeps_a_named_call_and_structured_branch() {
    let callee_flow = DraftReferenceFlow {
        events: Vec::new(),
        terminator: DraftReferenceTerminator::Return(SymbolicValue::input(0)),
    };
    let flow = DraftReferenceFlow {
        events: vec![DraftReferenceEvent::ComposedCall {
            token: 0,
            symbol: "vendor_child".to_owned(),
            arguments: vec![SymbolicValue::input(0)].into_boxed_slice(),
            flow: Box::new(callee_flow),
            result_modeled: true,
        }],
        terminator: DraftReferenceTerminator::Branch {
            condition: BranchCondition {
                site: 0x1010,
                operation: BranchOperation::Equal,
                left: SymbolicValue::input(0),
                right: SymbolicValue::Constant(0),
            },
            taken: Box::new(DraftReferenceFlow {
                events: Vec::new(),
                terminator: DraftReferenceTerminator::Return(SymbolicValue::Constant(1)),
            }),
            not_taken: Box::new(DraftReferenceFlow {
                events: Vec::new(),
                terminator: DraftReferenceTerminator::Return(SymbolicValue::Constant(2)),
            }),
        },
    };
    let trace = FunctionAnalysis {
        symbol: "vendor_parent".to_owned(),
        events: Vec::new(),
        located_events: Vec::new(),
        located_reference_events: Vec::new(),
        reference_events: Vec::new(),
        reference_dependencies: vec!["vendor_child".to_owned()],
        blockers: Vec::new(),
        reference_blockers: Vec::new(),
        return_value: SymbolicValue::Unknown,
        reference_flow: Some(flow),
        unresolved_branch: None,
    };

    let pseudo = render_pseudo("vendor_parent", &trace, &[], &[], &[], &[], None);
    assert!(
        pseudo.contains("let call0 = vendor_child(arg0);"),
        "{pseudo}"
    );
    assert!(pseudo.contains("if arg0 == 0x00000000"), "{pseudo}");
    assert!(pseudo.contains("return 0x00000001;"), "{pseudo}");
    assert!(pseudo.contains("return 0x00000002;"), "{pseudo}");
}

#[test]
fn context_map_recovers_argument_offsets_branch_paths_and_rmw_masks() {
    let write = DraftReferenceEvent::Memory {
        access: MemoryAccess::Write,
        width: 32,
        address: SymbolicValue::input(2).add_constant(4),
        region: "caller-owned ABI argument RAM".to_owned(),
        value: Some(SymbolicValue::MemoryImage {
            read_token: 0,
            and_mask: 0xffff_ffdf,
            or_mask: 0x20,
        }),
    };
    let trace = FunctionAnalysis {
        symbol: "update_context".to_owned(),
        events: Vec::new(),
        located_events: Vec::new(),
        located_reference_events: Vec::new(),
        reference_events: Vec::new(),
        reference_dependencies: Vec::new(),
        blockers: Vec::new(),
        reference_blockers: Vec::new(),
        return_value: SymbolicValue::Constant(0),
        reference_flow: Some(DraftReferenceFlow {
            events: Vec::new(),
            terminator: DraftReferenceTerminator::Branch {
                condition: BranchCondition {
                    site: 0x1010,
                    operation: BranchOperation::NotEqual,
                    left: SymbolicValue::input(1),
                    right: SymbolicValue::Constant(0),
                },
                taken: Box::new(DraftReferenceFlow {
                    events: vec![write],
                    terminator: DraftReferenceTerminator::Return(SymbolicValue::Constant(1)),
                }),
                not_taken: Box::new(DraftReferenceFlow {
                    events: Vec::new(),
                    terminator: DraftReferenceTerminator::Return(SymbolicValue::Constant(0)),
                }),
            },
        }),
        unresolved_branch: None,
    };

    let accesses = context_accesses_for_trace(&trace);
    let fields = context_fields_for_accesses(&accesses);
    let pseudo = render_pseudo("update_context", &trace, &[], &[], &[], &[], None);

    assert_eq!(accesses.len(), 1);
    assert_eq!(accesses[0].argument, 2);
    assert_eq!(accesses[0].offset, 4);
    assert_eq!(accesses[0].write_mask, Some(0x20));
    assert_eq!(accesses[0].preserved_mask, Some(0xffff_ffdf));
    assert_eq!(accesses[0].forced_zero_mask, Some(0));
    assert_eq!(accesses[0].forced_one_mask, Some(0x20));
    assert!(accesses[0].path.contains("if arg1 != 0x00000000"));
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].reads, 0);
    assert_eq!(fields[0].writes, 1);
    assert_eq!(fields[0].write_mask, 0x20);
    assert!(
        pseudo.contains("ctx2.write32(+0x4, ((ramread0 & 0xffffffdf) | 0x00000020));"),
        "{pseudo}"
    );
}

#[test]
fn memory_object_map_keeps_relocated_global_symbol_identity() {
    let trace = FunctionAnalysis {
        symbol: "update_global".to_owned(),
        events: Vec::new(),
        located_events: Vec::new(),
        located_reference_events: Vec::new(),
        reference_events: vec![DraftReferenceEvent::Memory {
            access: MemoryAccess::Write,
            width: 16,
            address: SymbolicValue::SymbolAddress {
                member: Some("state.o".to_owned()),
                symbol: "phy_state".to_owned(),
                hi_addend: 4,
                lo_addend: Some(4),
                post_offset: 8,
            },
            region: "symbol:state.o::phy_state".to_owned(),
            value: Some(SymbolicValue::Constant(7)),
        }],
        reference_dependencies: Vec::new(),
        blockers: Vec::new(),
        reference_blockers: Vec::new(),
        return_value: SymbolicValue::Constant(0),
        reference_flow: None,
        unresolved_branch: None,
    };

    let accesses = memory_object_accesses_for_trace(&trace);
    let fields = memory_object_fields_for_accesses(&accesses);
    assert_eq!(accesses.len(), 1);
    assert_eq!(accesses[0].offset, 12);
    assert!(matches!(
        &accesses[0].object,
        LinkedMemoryObject::Global { member, symbol }
            if member.as_deref() == Some("state.o") && symbol == "phy_state"
    ));
    assert_eq!(fields[0].writes, 1);
    assert_eq!(fields[0].write_mask, 0xffff);
}

#[test]
fn memory_object_map_distinguishes_global_pointer_from_its_runtime_pointee() {
    let global_address = SymbolicValue::SymbolAddress {
        member: Some("state.o".to_owned()),
        symbol: "g_state".to_owned(),
        hi_addend: 0,
        lo_addend: Some(0),
        post_offset: 0,
    };
    let trace = FunctionAnalysis {
        symbol: "update_indirect_state".to_owned(),
        events: Vec::new(),
        located_events: Vec::new(),
        located_reference_events: Vec::new(),
        reference_events: vec![
            DraftReferenceEvent::Memory {
                access: MemoryAccess::Read,
                width: 32,
                address: global_address,
                region: "symbol:state.o::g_state".to_owned(),
                value: None,
            },
            DraftReferenceEvent::Memory {
                access: MemoryAccess::Write,
                width: 16,
                address: SymbolicValue::memory_read(0, 32, false).add_constant(0x1c),
                region: "dereferenced known pointer RAM".to_owned(),
                value: Some(SymbolicValue::Constant(9)),
            },
        ],
        reference_dependencies: Vec::new(),
        blockers: Vec::new(),
        reference_blockers: Vec::new(),
        return_value: SymbolicValue::Constant(0),
        reference_flow: None,
        unresolved_branch: None,
    };

    let accesses = memory_object_accesses_for_trace(&trace);
    assert_eq!(accesses.len(), 2);
    assert!(
        accesses
            .iter()
            .any(|access| matches!(access.object, LinkedMemoryObject::Global { .. }))
    );
    let pointee = accesses
        .iter()
        .find(|access| matches!(access.object, LinkedMemoryObject::Dereferenced { .. }))
        .expect("dereferenced global access");
    assert!(matches!(
        &pointee.object,
        LinkedMemoryObject::Dereferenced {
            pointer,
            pointer_offset: 0,
        } if matches!(pointer.as_ref(), LinkedMemoryObject::Global { member, symbol }
            if member.as_deref() == Some("state.o") && symbol == "g_state")
    ));
    assert_eq!(pointee.offset, 0x1c);
}

#[test]
fn memory_object_map_keeps_absolute_address_space() {
    let trace = FunctionAnalysis {
        symbol: "update_absolute".to_owned(),
        events: Vec::new(),
        located_events: Vec::new(),
        located_reference_events: Vec::new(),
        reference_events: vec![DraftReferenceEvent::Memory {
            access: MemoryAccess::Write,
            width: 32,
            address: SymbolicValue::Constant(0x3fc8_1000),
            region: "dram".to_owned(),
            value: Some(SymbolicValue::Constant(1)),
        }],
        reference_dependencies: Vec::new(),
        blockers: Vec::new(),
        reference_blockers: Vec::new(),
        return_value: SymbolicValue::Constant(0),
        reference_flow: None,
        unresolved_branch: None,
    };

    let accesses = memory_object_accesses_for_trace(&trace);
    assert!(matches!(
        &accesses[0].object,
        LinkedMemoryObject::Absolute {
            address_space,
            address: 0x3fc8_1000,
        } if address_space == "dram"
    ));
}

#[test]
fn linked_absolute_table_base_is_not_reported_as_a_giant_context_offset() {
    let mut accesses = vec![MemoryObjectAccess {
        object: LinkedMemoryObject::Argument { index: 0 },
        offset: 0x1000_299c,
        access: "read",
        width: 8,
        path: "entry".to_owned(),
        value: None,
        value_pseudo: None,
        write_mask: None,
        preserved_mask: None,
        forced_zero_mask: None,
        forced_one_mask: None,
    }];
    let resolver = ReferenceResolver {
        symbols: Vec::new(),
        symbols_by_address: BTreeMap::new(),
        symbol_ids: BTreeMap::new(),
        exported_symbol_keys: BTreeSet::new(),
        relocated_calls: BTreeMap::new(),
        pointer_context: direct::StructuralPointerContext::default(),
        data_symbols: vec![artifact::ArtifactDataSymbolDefinition {
            member: None,
            name: "coex_pti_tab".to_owned(),
            address: 0x1000_299c,
            size: 48,
            exported: true,
        }],
        projected_direct_semantics: BTreeMap::new(),
    };

    attribute_data_symbols(&mut accesses, &resolver);

    assert_eq!(accesses[0].offset, 0);
    assert!(matches!(
        &accesses[0].object,
        LinkedMemoryObject::Indexed {
            object,
            argument: 0,
            stride: 1,
        } if matches!(object.as_ref(), LinkedMemoryObject::Global { member: None, symbol }
            if symbol == "coex_pti_tab")
    ));
    assert!(context_accesses_for_memory_objects(&accesses).is_empty());

    let trace = FunctionAnalysis {
        symbol: "coex_core_pti_get".to_owned(),
        events: Vec::new(),
        located_events: Vec::new(),
        located_reference_events: Vec::new(),
        reference_events: vec![DraftReferenceEvent::Memory {
            access: MemoryAccess::Read,
            width: 8,
            address: SymbolicValue::input(0).add_constant(0x1000_299c),
            region: "dram".to_owned(),
            value: None,
        }],
        reference_dependencies: Vec::new(),
        blockers: Vec::new(),
        reference_blockers: Vec::new(),
        return_value: SymbolicValue::Constant(0),
        reference_flow: None,
        unresolved_branch: None,
    };
    let pseudo = render_pseudo(
        "coex_core_pti_get",
        &trace,
        &[],
        &[],
        &[],
        &[],
        Some(&resolver),
    );
    assert!(pseudo.contains("coex_pti_tab[arg0 + 0x0].read8()"));
}

#[test]
fn instruction_effects_keep_exact_mmio_and_memory_sites() {
    let memory = DraftReferenceEvent::Memory {
        access: MemoryAccess::Write,
        width: 32,
        address: SymbolicValue::input(0).add_constant(4),
        region: "caller-owned ABI argument RAM".to_owned(),
        value: Some(SymbolicValue::Constant(7)),
    };
    let mmio = DraftReferenceEvent::Observable(ObservableEvent::Memory {
        access: MemoryAccess::Write,
        width: 32,
        address: 0x2010_42b4,
        register: "STA_BEACON_FILTER.CONTROL".to_owned(),
        value: Some(SymbolicValue::Constant(0)),
    });
    let trace = FunctionAnalysis {
        symbol: "instruction_sites".to_owned(),
        events: Vec::new(),
        located_events: Vec::new(),
        located_reference_events: vec![
            LocatedReferenceEvent {
                site: 0x1004,
                event: memory.clone(),
            },
            LocatedReferenceEvent {
                site: 0x1008,
                event: mmio.clone(),
            },
        ],
        reference_events: vec![memory, mmio],
        reference_dependencies: Vec::new(),
        blockers: Vec::new(),
        reference_blockers: Vec::new(),
        return_value: SymbolicValue::Constant(0),
        reference_flow: None,
        unresolved_branch: None,
    };
    let resolver = ReferenceResolver {
        symbols: Vec::new(),
        symbols_by_address: BTreeMap::new(),
        symbol_ids: BTreeMap::new(),
        exported_symbol_keys: BTreeSet::new(),
        relocated_calls: BTreeMap::new(),
        pointer_context: direct::StructuralPointerContext::default(),
        data_symbols: Vec::new(),
        projected_direct_semantics: BTreeMap::new(),
    };
    let memory_accesses = memory_object_accesses_for_trace(&trace);
    let mmio_accesses = mmio_accesses_for_trace(&trace);
    let effects =
        instruction_effects_for_trace(&trace, &resolver, &mmio_accesses, &memory_accesses);

    assert!(effects.iter().any(|effect| matches!(
        effect,
        LinkedInstructionEffect::Memory {
            site: 0x1004,
            object: LinkedMemoryObject::Argument { index: 0 },
            offset: 4,
            ..
        }
    )));
    assert!(effects.iter().any(|effect| matches!(
        effect,
        LinkedInstructionEffect::Mmio {
            site: 0x1008,
            address: 0x2010_42b4,
            ..
        }
    )));
}

#[test]
fn mmio_index_keeps_static_indexed_poll_and_write_bit_evidence() {
    assert_eq!(
        candidate_bit_ranges(0x3000_00f3, 32),
        [
            (0, 1, 0x0000_0003),
            (4, 7, 0x0000_00f0),
            (28, 29, 0x3000_0000),
        ]
    );
    let address = 0x2010_7030;
    let write_value = SymbolicValue::register_read(0, address, 32, false)
        .and(0xffff_fff0)
        .or(0x5);
    let trace = FunctionAnalysis {
        symbol: "touch_registers".to_owned(),
        events: Vec::new(),
        located_events: Vec::new(),
        located_reference_events: Vec::new(),
        reference_events: Vec::new(),
        reference_dependencies: Vec::new(),
        blockers: Vec::new(),
        reference_blockers: Vec::new(),
        return_value: SymbolicValue::Constant(0),
        reference_flow: Some(DraftReferenceFlow {
            events: vec![
                DraftReferenceEvent::Observable(ObservableEvent::Memory {
                    access: MemoryAccess::Write,
                    width: 32,
                    address,
                    register: "AGC.CONTROL".to_owned(),
                    value: Some(write_value),
                }),
                DraftReferenceEvent::IndexedMmio {
                    access: MemoryAccess::Read,
                    width: 32,
                    address: SymbolicValue::input(0).shift_left(2).add_constant(address),
                    registers: vec![
                        crate::IndexedMmioRegister {
                            address,
                            name: "AGC.CONTROL".to_owned(),
                        },
                        crate::IndexedMmioRegister {
                            address: address + 4,
                            name: "AGC.STATUS".to_owned(),
                        },
                    ],
                    guard: Some(crate::IndexedMmioGuard {
                        selector: SymbolicValue::input(0),
                        maximum: 2,
                    }),
                    value: None,
                },
                DraftReferenceEvent::PollMmio {
                    width: 32,
                    address: SymbolicValue::Constant(address + 4),
                    registers: vec![crate::IndexedMmioRegister {
                        address: address + 4,
                        name: "AGC.STATUS".to_owned(),
                    }],
                    guard: None,
                    mask: 1,
                    expected: 1,
                },
            ],
            terminator: DraftReferenceTerminator::Return(SymbolicValue::Constant(0)),
        }),
        unresolved_branch: None,
    };

    let accesses = mmio_accesses_for_trace(&trace);

    assert_eq!(accesses.len(), 4);
    assert_eq!(
        accesses
            .iter()
            .map(|access| access.ordinal)
            .collect::<Vec<_>>(),
        [0, 1, 2, 3]
    );
    let write = accesses
        .iter()
        .find(|access| access.access == "write")
        .unwrap();
    assert_eq!(write.mode, "static");
    assert_eq!(write.modified_mask, Some(0xf));
    assert_eq!(write.preserved_mask, Some(0xffff_fff0));
    assert_eq!(write.forced_zero_mask, Some(0xa));
    assert_eq!(write.forced_one_mask, Some(0x5));
    assert_eq!(
        accesses
            .iter()
            .filter(|access| access.mode == "indexed-candidate")
            .count(),
        2
    );
    assert!(
        accesses
            .iter()
            .filter(|access| access.mode == "indexed-candidate")
            .all(|access| access.guard.as_deref() == Some("arg0 <= 2"))
    );
    let pseudo = render_pseudo("touch_registers", &trace, &[], &[], &[], &[], None);
    assert!(pseudo.contains("assert!(arg0 <= 2);"), "{pseudo}");
    assert!(!pseudo.contains("assert!(arg0 < 2);"), "{pseudo}");
    let poll = accesses
        .iter()
        .find(|access| access.access == "poll")
        .unwrap();
    assert_eq!(poll.mode, "static");
    assert_eq!(poll.predicate_mask, Some(1));
    assert_eq!(poll.predicate_expected, Some(1));
    assert_eq!(
        poll.guard.as_deref(),
        Some("value & 0x00000001 == 0x00000001")
    );
}

#[test]
fn field_candidates_separate_evidence_and_exclude_whole_register_masks() {
    let mut register = MmioRegisterAccumulator::default();
    record_access_field_mask(&mut register, 0x30, 32, "writer", "write", None);
    record_access_field_mask(&mut register, 0x30, 32, "poller", "poll", None);
    let false_branch = LinkedMmioFieldPredicateEvidence {
        kind: "producer-return",
        function: "dispatcher".to_owned(),
        producer: Some("wrapper".to_owned()),
        producer_path: vec!["wrapper".to_owned(), "reader".to_owned()],
        site: Some(0x10),
        path: None,
        condition: "result & 0x30 != 0".to_owned(),
        operation: "not-equal",
        taken: Some(false),
        effective_operation: Some("equal"),
        operand: Some("left"),
        comparison_value: Some(0),
        register_comparison_value: Some(0),
        inverted: false,
    };
    let mut true_branch = false_branch.clone();
    true_branch.taken = Some(true);
    true_branch.effective_operation = Some("not-equal");
    record_predicate_field_mask(
        &mut register,
        0x30,
        32,
        "dispatcher",
        &[false_branch, true_branch],
    );
    record_semantic_field_link(
        &mut register,
        SemanticFieldEvidence {
            kind: "producer-return",
            mask: 0x30,
            width: 32,
            operation: "rtos.event.post",
            root: "irq_handler",
            action_target: "pp_post",
            action_origin: "event_dispatch",
            action_site: Some(0x30),
            action_site_path: &[Some(0x20), Some(0x30)],
            action_path: "irq_handler -> event_dispatch -> pp_post",
            predicate_function: "dispatcher",
            producer: Some("wrapper"),
            producer_path: &["wrapper".to_owned(), "reader".to_owned()],
            scope_index: 0,
            scope_alternatives: 1,
            path_index: 0,
            path_expression: "!(result & 0x30 != 0) && (queue != 0)",
            path_guards: 2,
            guard_index: 0,
            residual_path_expression: "(queue != 0)",
            site: 0x10,
            condition: "result & 0x30 != 0",
            taken: false,
            guard_operation: "not-equal",
        },
    );
    record_access_field_mask(&mut register, u32::MAX, 32, "whole_writer", "write", None);
    record_predicate_field_mask(
        &mut register,
        u32::MAX,
        32,
        "whole_dispatcher",
        &[LinkedMmioFieldPredicateEvidence {
            kind: "direct-mmio",
            function: "whole_dispatcher".to_owned(),
            producer: None,
            producer_path: vec!["whole_dispatcher".to_owned()],
            site: Some(0x20),
            path: None,
            condition: "read != 0".to_owned(),
            operation: "not-equal",
            taken: None,
            effective_operation: None,
            operand: Some("left"),
            comparison_value: Some(0),
            register_comparison_value: Some(0),
            inverted: false,
        }],
    );

    assert_eq!(register.field_candidates.len(), 1);
    let candidate = register
        .field_candidates
        .get(&(4, 5, 0x30))
        .expect("partial mask creates one contiguous candidate");
    assert_eq!(candidate.write_shapes, 1);
    assert_eq!(candidate.poll_shapes, 1);
    assert_eq!(candidate.predicate_shapes, 1);
    assert_eq!(
        candidate.functions,
        BTreeSet::from([
            "dispatcher".to_owned(),
            "poller".to_owned(),
            "reader".to_owned(),
            "wrapper".to_owned(),
            "writer".to_owned(),
        ])
    );
    assert_eq!(
        candidate.access_functions,
        BTreeSet::from([
            "poller".to_owned(),
            "reader".to_owned(),
            "writer".to_owned()
        ])
    );
    assert_eq!(
        candidate.predicate_functions,
        BTreeSet::from(["dispatcher".to_owned()])
    );
    assert_eq!(candidate.predicate_evidence.len(), 2);
    assert_eq!(
        candidate.semantic_operations,
        BTreeSet::from(["rtos.event.post".to_owned()])
    );
    assert_eq!(
        candidate.semantic_roots,
        BTreeSet::from(["irq_handler".to_owned()])
    );
    assert_eq!(candidate.semantic_evidence.len(), 1);
    let semantic = candidate.semantic_evidence.first().unwrap();
    assert_eq!(semantic.effective_operation, "equal");
    assert!(!semantic.taken);
    assert_eq!(semantic.action_site_path, [Some(0x20), Some(0x30)]);
    assert_eq!(semantic.path_index, 0);
    assert_eq!(semantic.guard_index, 0);
    assert_eq!(semantic.residual_path_expression, "(queue != 0)");
    assert_eq!(semantic.producer.as_deref(), Some("wrapper"));
    assert_eq!(semantic.producer_path, ["wrapper", "reader"]);
}
