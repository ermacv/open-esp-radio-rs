//! Event dispatch, call linking, compaction, and basic rendering.

use super::*;

use open_radio_vendor_analysis_model::{
    ReviewedExternalCallEvidence, ReviewedExternalCallExecutionModel,
};

static LINK_UNIT_DELAY_ARGUMENTS: [crate::ExternalArgumentSpec; 1] =
    [crate::ExternalArgumentSpec {
        name: "micros",
        c_type: "u32",
        direction: crate::ExternalArgumentDirection::Input,
    }];
static LINK_UNIT_DELAY_SEMANTIC: crate::DirectSemanticFunctionSpec =
    crate::DirectSemanticFunctionSpec {
        id: "test-link-unit-delay",
        source: "test-addon",
        c_name: "ets_delay_us",
        argument_count: 1,
        body_policy: crate::SemanticFunctionBodyPolicy::OpaqueBoundary,
        return_model: crate::ExternalReturnModel::Unmodeled,
        semantic: crate::ExternalSemanticSpec {
            operation: "time.blocking-delay",
            arguments: &LINK_UNIT_DELAY_ARGUMENTS,
            return_type: "void",
            replacement: Some("Rust async timer"),
            event_dispatch: None,
        },
        evidence: "authoritative-link-unit-relocation-symbol",
    };

static C_MEMCPY_ARGUMENTS: [crate::ExternalArgumentSpec; 3] = [
    crate::ExternalArgumentSpec {
        name: "destination",
        c_type: "void *",
        direction: crate::ExternalArgumentDirection::Output,
    },
    crate::ExternalArgumentSpec {
        name: "source",
        c_type: "const void *",
        direction: crate::ExternalArgumentDirection::Input,
    },
    crate::ExternalArgumentSpec {
        name: "length",
        c_type: "size_t",
        direction: crate::ExternalArgumentDirection::Input,
    },
];
static C_MEMCPY_SEMANTIC: crate::DirectSemanticFunctionSpec = crate::DirectSemanticFunctionSpec {
    id: "test-c-standard-memcpy",
    source: "test-c-addon",
    c_name: "memcpy",
    argument_count: 3,
    body_policy: crate::SemanticFunctionBodyPolicy::OpaqueBoundary,
    return_model: crate::ExternalReturnModel::Unmodeled,
    semantic: crate::ExternalSemanticSpec {
        operation: "memory.copy",
        arguments: &C_MEMCPY_ARGUMENTS,
        return_type: "void *",
        replacement: None,
        event_dispatch: None,
    },
    evidence: "exact public symbol identity and standardized C function contract",
};

#[test]
fn authoritative_link_unit_symbol_names_and_types_a_direct_external_call() {
    let owner = symbol("vendor_init", 0x1000, vec![0x67, 0x80, 0x00, 0x00]);
    let external = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "ets_delay_us".to_owned(),
        address: 0x2f80_003c,
        bytes: Vec::new(),
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let hooks = Box::leak(Box::new(crate::RiscvSummaryHooks {
        secondary_return_target: |_| false,
        direct_semantic: |_| None,
        direct_external_semantic: |name| {
            (name == "ets_delay_us").then_some(&LINK_UNIT_DELAY_SEMANTIC)
        },
        reference_intrinsic: |_, _, _| None,
        standard_memory_function: |_| None,
        wide_signed_divide: |_, _| None,
    }));
    let mut resolver = empty_resolver();
    resolver.symbols = vec![owner.clone()];
    resolver.symbols_by_address.insert(0x2f80_003c, external);
    resolver.relocated_calls.insert(
        crate::StructuralCallSite::new(&owner, 0x1004),
        ("ets_delay_us".to_owned(), Some(0x2f80_003c)),
    );
    resolver.pointer_context.summary_hooks = Some(hooks);
    let identities = IrIdentityCatalog::new(&resolver, None);
    let mut calls = vec![LinkedCall {
        kind: "internal",
        target: "ets_delay_us".to_owned(),
        site: Some(0x1004),
        tail: false,
        result_modeled: false,
        execution_model: None,
        semantics: None,
        semantic_operation: None,
        semantic_contract: None,
        replacement_hint: None,
        project_symbol: None,
        project_candidates: Vec::new(),
        trampoline: None,
        argument_shapes: 1,
        arguments: vec!["const:0x0000000a".to_owned()],
        argument_bindings: Vec::new(),
        typed_arguments: Vec::new(),
        guard_paths: None,
    }];

    annotate_direct_semantic_calls(&mut calls, &owner, &resolver, &identities);

    assert_eq!(calls[0].kind, "external");
    assert_eq!(calls[0].target, "ets_delay_us");
    assert_eq!(
        calls[0].semantic_operation.as_deref(),
        Some("time.blocking-delay")
    );
    assert_eq!(
        calls[0]
            .semantic_contract
            .as_ref()
            .map(|contract| contract.source),
        Some("authoritative-link-unit-symbol")
    );
    assert_eq!(calls[0].typed_arguments[0].name, "micros");
}

#[test]
fn unique_archive_origin_can_name_a_relaxed_internal_definition() {
    let owner = symbol("vendor_init", 0x1000, vec![0x67, 0x80, 0x00, 0x00]);
    let linked = symbol("pp_post", 0x2000, vec![0x67, 0x80, 0x00, 0x00]);
    let hooks = Box::leak(Box::new(crate::RiscvSummaryHooks {
        secondary_return_target: |_| false,
        direct_semantic: |_| None,
        direct_external_semantic: |_| None,
        reference_intrinsic: |_, _, _| None,
        standard_memory_function: |_| None,
        wide_signed_divide: |_, _| None,
    }));
    let mut resolver = empty_resolver();
    resolver.symbols = vec![owner.clone(), linked.clone()];
    resolver.pointer_context.summary_hooks = Some(hooks);
    resolver.register_projected_direct_semantic(&linked, &LINK_UNIT_DELAY_SEMANTIC);
    let identities = IrIdentityCatalog::new(&resolver, None);
    let mut calls = vec![LinkedCall {
        kind: "internal",
        target: identities.symbol(&linked),
        site: Some(0x1004),
        tail: false,
        result_modeled: false,
        execution_model: None,
        semantics: None,
        semantic_operation: None,
        semantic_contract: None,
        replacement_hint: None,
        project_symbol: None,
        project_candidates: Vec::new(),
        trampoline: None,
        argument_shapes: 1,
        arguments: vec!["arg0".to_owned()],
        argument_bindings: Vec::new(),
        typed_arguments: Vec::new(),
        guard_paths: None,
    }];

    annotate_direct_semantic_calls(&mut calls, &owner, &resolver, &identities);

    assert_eq!(
        calls[0].semantic_operation.as_deref(),
        Some("time.blocking-delay")
    );
    assert_eq!(
        calls[0]
            .semantic_contract
            .as_ref()
            .map(|contract| contract.source),
        Some("unique-reviewed-archive-origin")
    );
}

#[test]
fn event_dispatch_projection_assigns_reviewed_argument_roles() {
    let actions = vec![
        projected_semantic_action("wifi.internal-signal.post", Vec::new(), None),
        projected_semantic_action(
            "vendor.radio.notify",
            vec![projected_argument(0, "signal", "u32", "const:0x1a")],
            Some(event_dispatch_contract(
                "internal-signal",
                "unspecified",
                Some("test::radio-owner"),
                &[("selector", "signal")],
            )),
        ),
        projected_semantic_action(
            "platform.queue.publish",
            vec![
                projected_argument(0, "queue", "*mut void", "arg0"),
                projected_argument(1, "item", "*const void", "arg1"),
                projected_argument(2, "higher_priority_task_woken", "*mut bool", "arg2"),
            ],
            Some(event_dispatch_contract(
                "rtos-queue",
                "isr",
                None,
                &[
                    ("channel", "queue"),
                    ("payload", "item"),
                    ("wake-output", "higher_priority_task_woken"),
                ],
            )),
        ),
        projected_semantic_action(
            "platform.event.publish",
            vec![
                projected_argument(0, "event_base", "*const char", "arg0"),
                projected_argument(1, "event_id", "i32", "const:0x7"),
                projected_argument(2, "event_data", "*const void", "arg1"),
                projected_argument(3, "event_data_size", "usize", "const:0x4"),
                projected_argument(4, "ticks_to_wait", "u32", "const:0x0"),
            ],
            Some(event_dispatch_contract(
                "rtos-event-loop",
                "unspecified",
                None,
                &[
                    ("channel", "event_base"),
                    ("selector", "event_id"),
                    ("payload", "event_data"),
                    ("payload-size", "event_data_size"),
                    ("wait", "ticks_to_wait"),
                ],
            )),
        ),
    ];

    let dispatches = project_event_dispatches(&actions);

    assert_eq!(dispatches.len(), 3);
    assert_eq!(dispatches[0].semantic_action_index, 1);
    assert_eq!(dispatches[0].mechanism, "internal-signal");
    assert_eq!(dispatches[0].execution_context, "unspecified");
    assert_eq!(dispatches[0].receiver.as_deref(), Some("test::radio-owner"));
    assert!(dispatches[0].interface_complete);
    assert_eq!(dispatches[0].bindings[0].role, "selector");
    assert_eq!(dispatches[0].bindings[0].argument.value, "const:0x1a");

    assert_eq!(dispatches[1].semantic_action_index, 2);
    assert_eq!(dispatches[1].mechanism, "rtos-queue");
    assert_eq!(dispatches[1].execution_context, "isr");
    assert_eq!(
        dispatches[1]
            .bindings
            .iter()
            .map(|binding| binding.role)
            .collect::<Vec<_>>(),
        ["channel", "payload", "wake-output"]
    );

    assert_eq!(dispatches[2].semantic_action_index, 3);
    assert_eq!(dispatches[2].mechanism, "rtos-event-loop");
    assert_eq!(
        dispatches[2]
            .bindings
            .iter()
            .map(|binding| binding.role)
            .collect::<Vec<_>>(),
        ["channel", "selector", "payload", "payload-size", "wait"]
    );
    assert!(
        dispatches
            .iter()
            .all(|dispatch| dispatch.interface_complete && dispatch.blockers.is_empty())
    );
    assert!(
        dispatches[1..]
            .iter()
            .all(|dispatch| dispatch.receiver.is_none())
    );
}

#[test]
fn event_dispatch_projection_exposes_contract_and_schema_mismatches() {
    let actions = vec![projected_semantic_action(
        "wifi.internal-signal.post",
        vec![projected_argument(0, "unexpected", "u32", "arg0")],
        Some(event_dispatch_contract(
            "internal-signal",
            "unspecified",
            None,
            &[("selector", "signal")],
        )),
    )];

    let dispatches = project_event_dispatches(&actions);

    assert_eq!(dispatches.len(), 1);
    assert!(!dispatches[0].interface_complete);
    assert!(dispatches[0].bindings.is_empty());
    assert_eq!(
        dispatches[0].blockers,
        [
            "missing semantic argument signal for role selector",
            "unexpected semantic argument unexpected at position 0",
        ]
    );
}

#[test]
fn direct_call_graph_survives_reference_summary_inlining() {
    let parent = symbol(
        "vendor_parent",
        0x1000,
        vec![
            0x97, 0x00, 0x00, 0x00, // auipc ra, 0
            0xe7, 0x80, 0x00, 0x00, // jalr ra, 0(ra)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
    );
    let child = symbol(
        "vendor_child",
        0x2000,
        vec![0x67, 0x80, 0x00, 0x00], // ret
    );
    let child_id = 0x8000_0000;
    let resolver = ReferenceResolver {
        symbols: vec![parent.clone(), child.clone()],
        symbols_by_address: BTreeMap::from([(child_id, child)]),
        symbol_ids: BTreeMap::from([
            (
                (parent.member.clone(), parent.name.clone(), parent.address),
                0x8000_0001,
            ),
            (
                (
                    Some("member.o".to_owned()),
                    "vendor_child".to_owned(),
                    0x2000,
                ),
                child_id,
            ),
        ]),
        exported_symbol_keys: BTreeSet::new(),
        relocated_calls: BTreeMap::from([(
            direct::StructuralCallSite::new(&parent, 0x1000),
            ("vendor_child".to_owned(), Some(child_id)),
        )]),
        pointer_context: direct::StructuralPointerContext::default(),
        data_symbols: Vec::new(),
        data_objects: Vec::new(),
        projected_direct_semantics: BTreeMap::new(),
        projected_origins: BTreeMap::new(),
    };
    let map = MmioMap {
        registers: Vec::new(),
        regions: Vec::new(),
    };

    let identities = IrIdentityCatalog::new(&resolver, None);
    let graph = explore_direct_calls(&parent, &resolver, &identities, &map);
    let calls = graph.calls.into_iter().collect::<Vec<_>>();

    assert!(graph.blockers.is_empty(), "{:#?}", graph.blockers);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].kind, "internal");
    assert_eq!(calls[0].target, "member.o:vendor_child");
    assert_eq!(calls[0].site, Some(0x1000));
    assert_eq!(calls[0].argument_bindings.len(), 16);
    assert_eq!(
        calls[0].argument_bindings[0],
        LinkedArgumentBinding {
            position: 0,
            caller_argument: 0,
            offset: 0,
            expression: "arg0".to_owned(),
        }
    );

    let roots_only = build_linked_ir_for_source(
        &resolver,
        &map,
        LinkedIrSourceOptions {
            symbol_prefix: "vendor_parent",
            source: "primary",
            namespace_identities: false,
            include_reachable: false,
            jobs: 1,
            compact_projected_actions: false,
        },
    );
    assert_eq!(roots_only.functions.len(), 1);
    assert_eq!(roots_only.functions[0].symbol, "vendor_parent");

    let report = build_linked_ir_for_source(
        &resolver,
        &map,
        LinkedIrSourceOptions {
            symbol_prefix: "vendor_parent",
            source: "primary",
            namespace_identities: false,
            include_reachable: true,
            jobs: 1,
            compact_projected_actions: false,
        },
    );
    assert_eq!(
        report
            .functions
            .iter()
            .map(|function| (function.symbol.as_str(), function.selection))
            .collect::<Vec<_>>(),
        [
            ("vendor_child", "reachable-internal"),
            ("vendor_parent", "symbol-prefix-root"),
        ]
    );
}

#[test]
fn lossless_relocation_call_survives_an_earlier_semantic_stop() {
    let parent = symbol("vendor_parent", 0x1000, vec![0x73, 0, 0, 0]);
    let child = symbol("vendor_child", 0x2000, vec![0x67, 0x80, 0, 0]);
    let child_id = 0x8000_0000;
    let mut resolver = empty_resolver();
    resolver.symbols = vec![parent.clone(), child.clone()];
    resolver.symbols_by_address.insert(child_id, child);
    resolver.relocated_calls.insert(
        direct::StructuralCallSite::new(&parent, 0x1008),
        ("vendor_child".to_owned(), Some(child_id)),
    );
    let identities = IrIdentityCatalog::new(&resolver, None);
    let mut calls = BTreeSet::new();

    add_lossless_relocation_calls(&mut calls, &parent, &resolver, &identities);

    assert_eq!(calls.len(), 1);
    let call = calls.first().unwrap();
    assert_eq!(call.kind, "structural-relocation");
    assert_eq!(call.target, "member.o:vendor_child");
    assert_eq!(call.site, Some(0x1008));
    assert_eq!(call.argument_shapes, 0);
    assert!(call.guard_paths.is_none());
}

#[test]
fn archive_call_projects_through_relaxed_instruction_correspondence() {
    let runtime = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "vendor_parent".to_owned(),
        address: 0x1000,
        bytes: vec![0xef, 0x00, 0x00, 0x00], // jal ra, 0
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let origin = artifact::ArtifactSymbolDefinition {
        member: Some("parent.o".to_owned()),
        name: "vendor_parent".to_owned(),
        address: 0,
        bytes: vec![
            0x97, 0x00, 0x00, 0x00, // auipc ra, 0
            0xe7, 0x80, 0x00, 0x00, // jalr ra, 0(ra)
        ],
        addresses_resolved: false,
        memory_regions: Default::default(),
        relocations: vec![artifact::SymbolRelocation {
            address: 0,
            kind: artifact::RelocationKind::Call,
            symbol: "vendor_child".to_owned(),
            addend: 0,
        }],
    };
    let child = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "vendor_child".to_owned(),
        address: 0x2000,
        bytes: vec![0x67, 0x80, 0x00, 0x00],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let mut resolver = empty_resolver();
    resolver.symbols = vec![runtime.clone(), child.clone()];
    resolver.symbols_by_address.insert(0x1000, runtime.clone());
    resolver.symbols_by_address.insert(0x2000, child);
    resolver.register_projected_origin(&runtime, origin);
    let identities = IrIdentityCatalog::new(&resolver, None);
    let mut calls = BTreeSet::new();

    add_projected_origin_calls(&mut calls, &runtime, &resolver, &identities).unwrap();

    let call = calls.first().unwrap();
    assert_eq!(call.kind, "structural-relocation");
    assert_eq!(call.target, "vendor_child");
    assert_eq!(call.site, Some(0x1000));
    assert_eq!(call.argument_shapes, 0);
}

#[test]
fn external_call_keeps_reviewed_table_slot_semantics() {
    let event = DraftReferenceEvent::ReviewedExternalCall {
        token: 0,
        site: 0x40,
        candidates: vec![ReviewedExternalCall {
            id: "pack::wifi-osi@+0x20".to_owned(),
            contract: "pack::wifi-osi".to_owned(),
            name: "ets_delay_us".to_owned(),
            argument_types: vec!["u32".to_owned()],
            return_type: "void".to_owned(),
            variadic: false,
            semantic_operation: Some("time.delay-micros".to_owned()),
            replacement_hint: Some("Rust async timer".to_owned()),
            execution_model: Some(ReviewedExternalCallExecutionModel {
                id: "wifi-osi-models.delay-us".to_owned(),
                return_model: ExternalReturnModel::Constant(0),
                outputs: Vec::new(),
            }),
            tail: false,
            evidence: ReviewedExternalCallEvidence::ObservedCallSite,
            slot_load_site: Some(0x3c),
        }],
        arguments: vec![SymbolicValue::input(0)].into_boxed_slice(),
    };
    let mut calls = BTreeSet::new();
    let resolver = empty_resolver();
    let identities = IrIdentityCatalog::new(&resolver, None);
    let mut pseudo = String::new();

    collect_call_event(&event, &resolver, &identities, &mut calls);
    render_event(&event, &mut pseudo, 1, &mut RenderState::default());
    let call = calls.iter().next().unwrap();

    assert_eq!(call.kind, "reviewed-external");
    assert_eq!(call.site, Some(0x40));
    assert_eq!(call.target, "pack::wifi-osi::ets_delay_us");
    assert_eq!(call.arguments, [SymbolicValue::input(0).canonical()]);
    assert_eq!(
        call.semantic_operation.as_deref(),
        Some("time.delay-micros")
    );
    assert_eq!(
        call.semantic_contract.as_ref(),
        Some(&LinkedSemanticContract {
            source: "reviewed-interface-pack",
            id: "pack::wifi-osi@+0x20".to_owned(),
            evidence: "reviewed-layout-and-observed-call-site".to_owned(),
            body_policy: "opaque-boundary",
            event_dispatch: None,
        })
    );
    assert_eq!(call.replacement_hint.as_deref(), Some("Rust async timer"));
    assert_eq!(call.typed_arguments.len(), 1);
    assert_eq!(call.typed_arguments[0].name, "arg0");
    assert_eq!(call.typed_arguments[0].c_type, "u32");
    assert_eq!(call.typed_arguments[0].direction, "unknown");
    assert!(call.trampoline.is_none());
    assert!(call.result_modeled);
    assert_eq!(
        call.execution_model,
        Some(LinkedExternalExecutionModel {
            id: "wifi-osi-models.delay-us".to_owned(),
            return_model: "constant:0x00000000".to_owned(),
            outputs: Vec::new(),
        })
    );
    assert!(
        pseudo.contains(
            "reviewed_abi.ets_delay_us(arg0); // site 0x00000040; model=wifi-osi-models.delay-us"
        ),
        "{pseudo}"
    );
    assert!(
        call.semantics.as_deref().is_some_and(
            |semantics| semantics.contains("executable-model=wifi-osi-models.delay-us")
        ),
        "{:?}",
        call.semantics
    );
}

#[test]
fn standard_memory_call_is_a_semantic_boundary_independent_of_its_body() {
    let event = DraftReferenceEvent::Call {
        token: 0,
        site: 0x40,
        target: 0x2000,
        arguments: vec![
            SymbolicValue::input(0),
            SymbolicValue::input(1),
            SymbolicValue::Constant(16),
        ]
        .into_boxed_slice(),
    };
    let owner = symbol("caller", 0x1000, vec![0x67, 0x80, 0x00, 0x00]);
    let runtime = symbol("memcpy", 0x2000, vec![0x73, 0x00, 0x10, 0x00]);
    let hooks = Box::leak(Box::new(crate::RiscvSummaryHooks {
        secondary_return_target: |_| false,
        direct_semantic: |symbol| (symbol.name == "memcpy").then_some(&C_MEMCPY_SEMANTIC),
        direct_external_semantic: |name| (name == "memcpy").then_some(&C_MEMCPY_SEMANTIC),
        reference_intrinsic: |_, _, _| None,
        standard_memory_function: |_| None,
        wide_signed_divide: |_, _| None,
    }));
    let mut resolver = empty_resolver();
    resolver.symbols.push(runtime.clone());
    resolver.symbols_by_address.insert(0x2000, runtime);
    resolver.pointer_context.summary_hooks = Some(hooks);
    let identities = IrIdentityCatalog::new(&resolver, None);
    let mut calls = BTreeSet::new();

    collect_call_event(&event, &resolver, &identities, &mut calls);
    let mut calls = calls.into_iter().collect::<Vec<_>>();
    annotate_direct_semantic_calls(&mut calls, &owner, &resolver, &identities);

    let call = calls.first().unwrap();
    assert_eq!(call.kind, "semantic-boundary");
    assert_eq!(call.semantic_operation.as_deref(), Some("memory.copy"));
    assert_eq!(
        call.semantic_contract.as_ref(),
        Some(&LinkedSemanticContract {
            source: "test-c-addon",
            id: "test-c-standard-memcpy".to_owned(),
            evidence: "exact public symbol identity and standardized C function contract"
                .to_owned(),
            body_policy: "opaque-boundary",
            event_dispatch: None,
        })
    );
    assert!(
        call.semantics
            .as_deref()
            .unwrap()
            .contains("reviewed direct semantic function=memcpy")
    );
    let mut pseudo = String::new();
    render_event(
        &event,
        &mut pseudo,
        1,
        &mut RenderState::with_context(None, std::slice::from_ref(call)),
    );
    assert!(
        pseudo.contains("memory_copy(arg0, arg1, 0x00000010)"),
        "{pseudo}"
    );
}

#[test]
fn call_compaction_keeps_only_bindings_shared_by_every_argument_shape() {
    let variant = |second_argument: &str, second_caller: u8| LinkedCall {
        kind: "internal",
        target: "member.o:callee".to_owned(),
        site: Some(0x24),
        tail: false,
        result_modeled: false,
        execution_model: None,
        semantics: None,
        semantic_operation: None,
        semantic_contract: None,
        replacement_hint: None,
        project_symbol: None,
        project_candidates: Vec::new(),
        trampoline: None,
        argument_shapes: 1,
        arguments: vec!["arg0".to_owned(), second_argument.to_owned()],
        argument_bindings: vec![
            LinkedArgumentBinding {
                position: 0,
                caller_argument: 0,
                offset: 0,
                expression: "arg0".to_owned(),
            },
            LinkedArgumentBinding {
                position: 1,
                caller_argument: second_caller,
                offset: 4,
                expression: format!("arg{second_caller} + 0x4"),
            },
        ],
        typed_arguments: vec![LinkedCallArgument {
            position: 1,
            name: "context".to_owned(),
            c_type: "*mut context".to_owned(),
            direction: "input-output",
            value: second_argument.to_owned(),
        }],
        guard_paths: None,
    };

    let calls = compact_calls([variant("arg1+0x4", 1), variant("arg3+0x4", 3)]);

    assert_eq!(calls.len(), 1);
    let call = &calls[0];
    assert_eq!(call.argument_shapes, 2);
    assert_eq!(call.arguments, ["arg0", "varies-across-2-shapes"]);
    assert_eq!(call.typed_arguments[0].value, "varies-across-2-shapes");
    assert_eq!(
        call.argument_bindings,
        [LinkedArgumentBinding {
            position: 0,
            caller_argument: 0,
            offset: 0,
            expression: "arg0".to_owned(),
        }]
    );

    let guarded = |taken| {
        let mut call = variant("arg1+0x4", 1);
        call.guard_paths = Some(vec![LinkedCallGuardPath {
            guards: vec![LinkedCallGuard {
                site: 0x10,
                condition: "arg0 != 0".to_owned(),
                operation: "not-equal",
                taken,
                result_sources: Vec::new(),
                direct_mmio_sources: Vec::new(),
            }],
        }]);
        call
    };
    let guarded = compact_calls([guarded(true), guarded(false)]);
    assert_eq!(guarded[0].argument_shapes, 1);
    assert_eq!(
        guarded[0].guard_paths,
        Some(vec![LinkedCallGuardPath { guards: Vec::new() }])
    );
}

#[test]
fn cfg_guard_paths_minimize_complementary_branches_without_weakening_other_clauses() {
    let guard = |site, condition: &str, taken| LinkedCallGuard {
        site,
        condition: condition.to_owned(),
        operation: "not-equal",
        taken,
        result_sources: Vec::new(),
        direct_mmio_sources: Vec::new(),
    };
    let paths = normalize_guard_paths([
        LinkedCallGuardPath {
            guards: vec![
                guard(0x10, "arg0 != 0", true),
                guard(0x20, "arg1 == 3", true),
            ],
        },
        LinkedCallGuardPath {
            guards: vec![
                guard(0x10, "arg0 != 0", true),
                guard(0x20, "arg1 == 3", false),
            ],
        },
        LinkedCallGuardPath {
            guards: vec![
                guard(0x10, "arg0 != 0", true),
                guard(0x20, "arg1 == 3", true),
                guard(0x30, "arg2 < 4", true),
            ],
        },
    ]);

    assert_eq!(
        paths,
        [LinkedCallGuardPath {
            guards: vec![guard(0x10, "arg0 != 0", true)],
        }]
    );
}

#[test]
fn guard_path_rendering_keeps_the_residual_condition_for_one_literal() {
    let guard = |site, condition: &str, taken| LinkedCallGuard {
        site,
        condition: condition.to_owned(),
        operation: "not-equal",
        taken,
        result_sources: Vec::new(),
        direct_mmio_sources: Vec::new(),
    };
    let path = LinkedCallGuardPath {
        guards: vec![
            guard(0x10, "status & 0x30 != 0", false),
            guard(0x20, "queue != 0", true),
        ],
    };
    assert_eq!(
        format_guard_path(&path),
        "!(status & 0x30 != 0) && (queue != 0)"
    );
    assert_eq!(format_guard_path_without(&path, 0), "(queue != 0)");
}

#[test]
fn cfg_guard_names_call_results_without_token_prefix_collisions() {
    let call_results = BTreeMap::from([
        (1, "vendor::one".to_owned()),
        (10, "vendor::ten".to_owned()),
    ]);

    assert_eq!(
        name_call_results(
            "call1 | call10 | external10 | external-result:10 | call-result:1 | callback",
            &call_results
        ),
        "result_of_vendor__one | result_of_vendor__ten | result_of_vendor__ten | result_of_vendor__ten | result_of_vendor__one | callback"
    );
}

#[test]
fn diagnostic_compaction_counts_exact_fragments_and_keeps_first_ordinals() {
    let diagnostic = compact_diagnostic(
        "symbolic-cfg: unsupported effects; repeated call; unique jump; repeated call; repeated call",
    );

    assert_eq!(diagnostic.original_fragments, 5);
    assert_eq!(diagnostic.fragments.len(), 3);
    assert_eq!(diagnostic.fragments[0].first_ordinal, 0);
    assert_eq!(diagnostic.fragments[0].occurrences, 1);
    assert_eq!(diagnostic.fragments[1].first_ordinal, 1);
    assert_eq!(diagnostic.fragments[1].occurrences, 3);
    assert_eq!(diagnostic.fragments[1].message, "repeated call");
    assert_eq!(diagnostic.fragments[2].first_ordinal, 2);
    assert_eq!(
        diagnostic.rendered,
        "symbolic-cfg: unsupported effects; repeated call [repeated 3 times]; unique jump"
    );
}

#[test]
fn diagnostic_compaction_leaves_a_single_fragment_unchanged() {
    let diagnostic = compact_diagnostic("decoder stopped at unsupported instruction");

    assert_eq!(diagnostic.original_fragments, 1);
    assert_eq!(diagnostic.fragments.len(), 1);
    assert_eq!(
        diagnostic.rendered,
        "decoder stopped at unsupported instruction"
    );
}

#[test]
fn diagnostics_expose_stable_root_cause_metadata() {
    let first = compact_diagnostic(
        "unmodeled-memory-load at 0x10002ea6: lw a4, 0(a2); base a2 = expr:Add(arg1,arg0)",
    );
    let second = compact_diagnostic(
        "unmodeled-memory-load at 0x10002ea6: lw a4, 0(a2); base a2 = expr:Add(arg1,const:0x4)",
    );

    assert_eq!(first.kind, "memory-load");
    assert_eq!(first.site, Some(0x1000_2ea6));
    assert_eq!(first.root_id, second.root_id);
    assert!(first.root_id.starts_with("blocker-"));
}

#[test]
fn diagnostic_root_ids_distinguish_sites() {
    let first = compact_diagnostic("input-dependent control-flow at 0x1000: beq a0, zero, 4");
    let second = compact_diagnostic("input-dependent control-flow at 0x1004: beq a0, zero, 4");

    assert_eq!(first.kind, "control-flow");
    assert_ne!(first.root_id, second.root_id);
}

#[test]
fn diagnostic_compaction_bounds_large_human_evidence() {
    let messages = (0..80)
        .map(|index| format!("diagnostic {index}: {}", "x".repeat(1_024)))
        .collect::<Vec<_>>();
    let diagnostics = compact_diagnostics(&messages);

    assert_eq!(diagnostics.len(), 65);
    assert!(
        diagnostics
            .last()
            .unwrap()
            .rendered
            .contains("16 additional")
    );
    assert!(diagnostics[0].rendered.contains("fragment truncated"));
    assert!(diagnostics[0].rendered.len() < 600);
}

#[test]
fn pseudo_value_renders_register_images_as_read_modify_write_expressions() {
    let value = SymbolicValue::RegisterImage {
        read_token: 3,
        address: 0x2010_7030,
        and_mask: 0xdfff_ffff,
        or_mask: 0x2000_0000,
    };

    assert_eq!(pseudo_value(&value), "((read3 & 0xdfffffff) | 0x20000000)");
}

#[test]
fn pseudo_value_hides_allocator_identity_flag() {
    let value = SymbolicValue::ExternalResult(
        open_radio_vendor_analysis_model::ALLOCATED_EXTERNAL_RESULT_TOKEN_FLAG | 7,
    );
    assert_eq!(pseudo_value(&value), "external7");

    let bits = SymbolicValue::from_bits(value.bits());
    assert!(pseudo_value(&bits).contains("external7"));
    assert!(!pseudo_value(&bits).contains("107374"));
}

#[test]
fn pseudo_value_compacts_aligned_symbolic_bit_slices_into_masks() {
    let mut bits = [BitSource::Constant(false); 32];
    for (bit, source) in bits.iter_mut().enumerate().take(8).skip(4) {
        *source = BitSource::CallResult {
            call_token: 10,
            bit: bit as u8,
            inverted: false,
        };
    }

    assert_eq!(
        pseudo_value(&SymbolicValue::Bits(Box::new(bits))),
        "(call10 & 0x000000f0)"
    );
}

#[test]
fn pseudo_arguments_compact_exact_and_unknown_abi_slot_runs() {
    let mut arguments = (0..8).map(SymbolicValue::input).collect::<Vec<_>>();
    arguments.extend((0..8).map(|_| SymbolicValue::Unknown));

    assert_eq!(
        pseudo_arguments(&arguments),
        "abi_inputs[0..8], unknown_abi_inputs[8..16]"
    );
}
