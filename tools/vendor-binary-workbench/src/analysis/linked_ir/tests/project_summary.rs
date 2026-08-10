//! Project linking and reachable context/effect summaries.

use super::*;

#[test]
fn duplicate_private_names_get_stable_address_qualified_ir_identities() {
    let first = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "private_helper".to_owned(),
        address: 0x1000,
        bytes: vec![0x67, 0x80, 0x00, 0x00],
        addresses_resolved: true,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };
    let second = artifact::ArtifactSymbolDefinition {
        address: 0x2000,
        ..first.clone()
    };
    let resolver = ReferenceResolver {
        symbols: vec![first.clone(), second.clone()],
        symbols_by_address: BTreeMap::from([
            (first.address as u32, first.clone()),
            (second.address as u32, second.clone()),
        ]),
        symbol_ids: BTreeMap::from([
            (
                (None, first.name.clone(), first.address),
                first.address as u32,
            ),
            (
                (None, second.name.clone(), second.address),
                second.address as u32,
            ),
        ]),
        exported_symbol_keys: BTreeSet::new(),
        relocated_calls: BTreeMap::new(),
        pointer_context: direct::StructuralPointerContext::default(),
    };
    let map = MmioMap {
        registers: Vec::new(),
        regions: Vec::new(),
    };

    let report =
        build_linked_ir_for_source(&resolver, "private_", &map, "primary", false, false, 0);

    assert_eq!(report.exported_functions, 0);
    assert_eq!(report.local_functions, 2);
    assert_eq!(
        report
            .functions
            .iter()
            .map(|function| (function.identity.as_str(), function.binding))
            .collect::<Vec<_>>(),
        [
            ("private_helper@0x00001000", "local"),
            ("private_helper@0x00002000", "local"),
        ]
    );

    let project_report = merge_linked_ir(vec![
        build_linked_ir_for_source(&resolver, "private_", &map, "libphy", true, false, 0),
        build_linked_ir_for_source(&resolver, "private_", &map, "rom", true, false, 0),
    ]);
    assert_eq!(project_report.functions.len(), 4);
    assert_eq!(
        project_report
            .functions
            .iter()
            .map(|function| (function.source.as_str(), function.identity.as_str()))
            .collect::<Vec<_>>(),
        [
            ("libphy", "libphy::private_helper@0x00001000"),
            ("libphy", "libphy::private_helper@0x00002000"),
            ("rom", "rom::private_helper@0x00001000"),
            ("rom", "rom::private_helper@0x00002000"),
        ]
    );

    let serial = summarize_linked_ir_with_jobs(
        build_linked_functions_for_roots(
            &resolver,
            resolver.symbols.clone(),
            "",
            &map,
            "primary",
            "primary",
            false,
            false,
        ),
        1,
    );
    let parallel = summarize_linked_ir_with_jobs(
        build_all_linked_functions_parallel(
            &resolver,
            resolver.symbols.clone(),
            &map,
            "primary",
            false,
            2,
        ),
        2,
    );
    assert_eq!(parallel, serial);
    assert_eq!(merge_linked_ir_with_jobs(vec![serial.clone()], 2), serial);
}

#[test]
fn decode_blockers_only_include_cfg_reachable_instructions() {
    let symbol = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "entry".to_owned(),
        address: 0x1000,
        bytes: [
            0x0020_29f3_u32.to_le_bytes().as_slice(),
            [0x67, 0x80, 0x00, 0x00].as_slice(),
            [0x00, 0x00, 0x00, 0x00].as_slice(),
        ]
        .concat(),
        addresses_resolved: true,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };
    let resolver = ReferenceResolver {
        symbols: vec![symbol.clone()],
        symbols_by_address: BTreeMap::from([(symbol.address as u32, symbol.clone())]),
        symbol_ids: BTreeMap::from([(
            (None, symbol.name.clone(), symbol.address),
            symbol.address as u32,
        )]),
        exported_symbol_keys: BTreeSet::new(),
        relocated_calls: BTreeMap::new(),
        pointer_context: direct::StructuralPointerContext::default(),
    };

    let map = MmioMap {
        registers: Vec::new(),
        regions: Vec::new(),
    };
    let report = build_linked_ir_for_source(&resolver, "", &map, "primary", false, false, 1);

    assert_eq!(report.functions.len(), 1);
    assert_eq!(report.functions[0].decode_blockers.len(), 1);
    assert_eq!(report.functions[0].decode_blockers[0].address, 0x1000);
    assert_eq!(
        report.functions[0].decode_blockers[0].class,
        "floating-point-csr"
    );
}

#[test]
fn project_call_linking_requires_one_exported_definition() {
    let unresolved = || LinkedCall {
        kind: "unresolved",
        target: "vendor_child".to_owned(),
        site: Some(0),
        tail: true,
        result_modeled: false,
        semantics: None,
        semantic_operation: None,
        semantic_contract: None,
        replacement_hint: None,
        project_symbol: Some("vendor_child".to_owned()),
        project_candidates: Vec::new(),
        trampoline: None,
        argument_shapes: 1,
        arguments: Vec::new(),
        argument_bindings: Vec::new(),
        typed_arguments: Vec::new(),
        guard_paths: None,
    };

    let mut unique = vec![
        summarize_linked_ir(vec![linked_test_function(
            "parent",
            "vendor_parent",
            "global-or-weak",
            vec![unresolved()],
        )]),
        summarize_linked_ir(vec![linked_test_function(
            "child",
            "vendor_child",
            "global-or-weak",
            Vec::new(),
        )]),
    ];
    link_project_calls(&mut unique);
    let parent = &unique[0].functions[0];
    assert_eq!(parent.calls[0].kind, "project-linked");
    assert_eq!(parent.calls[0].target, "child::vendor_child");
    assert_eq!(parent.dependencies, ["child::vendor_child"]);
    assert!(!parent.complete);

    let mut ambiguous = vec![
        summarize_linked_ir(vec![linked_test_function(
            "parent",
            "vendor_parent",
            "global-or-weak",
            vec![unresolved()],
        )]),
        summarize_linked_ir(vec![linked_test_function(
            "child-a",
            "vendor_child",
            "global-or-weak",
            Vec::new(),
        )]),
        summarize_linked_ir(vec![linked_test_function(
            "child-b",
            "vendor_child",
            "global-or-weak",
            Vec::new(),
        )]),
    ];
    link_project_calls(&mut ambiguous);
    let call = &ambiguous[0].functions[0].calls[0];
    assert_eq!(call.kind, "unresolved");
    assert_eq!(
        call.project_candidates,
        ["child-a::vendor_child", "child-b::vendor_child"]
    );
}

#[test]
fn reachable_effect_summary_keeps_cross_artifact_provenance() {
    let unresolved = LinkedCall {
        kind: "unresolved",
        target: "vendor_child".to_owned(),
        site: Some(0),
        tail: false,
        result_modeled: false,
        semantics: None,
        semantic_operation: None,
        semantic_contract: None,
        replacement_hint: None,
        project_symbol: Some("vendor_child".to_owned()),
        project_candidates: Vec::new(),
        trampoline: None,
        argument_shapes: 1,
        arguments: Vec::new(),
        argument_bindings: Vec::new(),
        typed_arguments: Vec::new(),
        guard_paths: None,
    };
    let mut child = linked_test_function(
        "child",
        "vendor_child",
        "global-or-weak",
        vec![LinkedCall {
            kind: "external",
            target: "wifi_osi::ets_delay_us".to_owned(),
            site: None,
            tail: false,
            result_modeled: true,
            semantics: Some("reviewed delay boundary".to_owned()),
            semantic_operation: Some("time.delay-micros".to_owned()),
            semantic_contract: None,
            replacement_hint: Some("Rust async timer".to_owned()),
            project_symbol: None,
            project_candidates: Vec::new(),
            trampoline: None,
            argument_shapes: 1,
            arguments: vec!["const:0x00000014".to_owned()],
            argument_bindings: Vec::new(),
            typed_arguments: Vec::new(),
            guard_paths: None,
        }],
    );
    child.complete = true;
    child.exact = true;
    child.mmio_accesses.push(LinkedMmioAccess {
        ordinal: 0,
        address: 0x6000_1000,
        width: 32,
        register: "UNKNOWN_60001000".to_owned(),
        access: "write",
        mode: "static",
        path: "entry".to_owned(),
        address_expression: None,
        guard: None,
        predicate_mask: None,
        predicate_expected: None,
        value: Some("const:0x00000001".to_owned()),
        modified_mask: Some(u32::MAX),
        preserved_mask: None,
        inverted_mask: None,
        forced_zero_mask: Some(u32::MAX - 1),
        forced_one_mask: Some(1),
        read_derived_mask: None,
        dynamic_mask: None,
    });
    child.delays.push(LinkedDelay {
        ordinal: 0,
        path: "entry".to_owned(),
        micros: "const:0x00000014".to_owned(),
        constant_micros: Some(20),
    });
    child.context_accesses.push(ContextAccess {
        argument: 0,
        offset: 4,
        access: "write",
        width: 32,
        path: "entry".to_owned(),
        value: Some("const:0x00000001".to_owned()),
        value_pseudo: Some("0x00000001".to_owned()),
        write_mask: Some(u32::MAX),
        preserved_mask: None,
        forced_zero_mask: Some(u32::MAX - 1),
        forced_one_mask: Some(1),
    });

    let mut reports = vec![
        summarize_linked_ir(vec![linked_test_function(
            "parent",
            "vendor_parent",
            "global-or-weak",
            vec![unresolved],
        )]),
        summarize_linked_ir(vec![child]),
    ];
    link_project_calls(&mut reports);
    let report = merge_linked_ir(reports);
    let parent = report
        .functions
        .iter()
        .find(|function| function.identity == "parent::vendor_parent")
        .unwrap();

    assert_eq!(
        parent.effect_summary.reachable_functions,
        ["child::vendor_child"]
    );
    assert_eq!(parent.effect_summary.max_depth, 1);
    assert!(!parent.effect_summary.call_graph_closed);
    assert_eq!(parent.effect_summary.mmio_registers.len(), 1);
    assert_eq!(
        parent.effect_summary.mmio_registers[0].origins,
        ["child::vendor_child"]
    );
    assert_eq!(parent.effect_summary.delays[0].constant_micros, Some(20));
    assert_eq!(
        parent.effect_summary.semantic_operations[0].operation,
        "time.delay-micros"
    );
    assert_eq!(
        parent.effect_summary.semantic_operations[0].origins,
        ["child::vendor_child"]
    );
    assert!(!parent.effect_summary.context_projection_complete);
    assert!(parent.effect_summary.context_fields.is_empty());
    assert!(
        parent
            .effect_summary
            .context_projection_blockers
            .iter()
            .any(|blocker| blocker.contains("no affine binding for child::vendor_child arg0"))
    );
}

#[test]
fn affine_call_bindings_project_transitive_context_fields() {
    let internal = |target: &str, caller_argument: u8, offset: i32| -> LinkedCall {
        LinkedCall {
            kind: "internal",
            target: target.to_owned(),
            site: Some(0x10),
            tail: false,
            result_modeled: false,
            semantics: None,
            semantic_operation: None,
            semantic_contract: None,
            replacement_hint: None,
            project_symbol: None,
            project_candidates: Vec::new(),
            trampoline: None,
            argument_shapes: 1,
            arguments: vec![format!("arg{caller_argument}{offset:+#x}")],
            argument_bindings: vec![LinkedArgumentBinding {
                position: 0,
                caller_argument,
                offset,
                expression: format!("arg{caller_argument}{offset:+#x}"),
            }],
            typed_arguments: Vec::new(),
            guard_paths: Some(vec![LinkedCallGuardPath {
                guards: vec![LinkedCallGuard {
                    site: 0x08,
                    condition: "arg1 != 0".to_owned(),
                    operation: "not-equal",
                    taken: true,
                    result_sources: Vec::new(),
                    direct_mmio_sources: Vec::new(),
                }],
            }]),
        }
    };
    let mut root = linked_test_function(
        "rom",
        "root",
        "global-or-weak",
        vec![internal("rom::middle", 2, 0x20)],
    );
    root.complete = true;
    let mut middle =
        linked_test_function("rom", "middle", "local", vec![internal("rom::leaf", 0, -8)]);
    middle.complete = true;
    let mut leaf = linked_test_function("rom", "leaf", "local", Vec::new());
    leaf.complete = true;
    leaf.calls.push(LinkedCall {
        kind: "external",
        target: "platform::timer_arm".to_owned(),
        site: None,
        tail: false,
        result_modeled: false,
        semantics: Some("typed trampoline".to_owned()),
        semantic_operation: Some("timer.arm-micros".to_owned()),
        semantic_contract: Some(LinkedSemanticContract {
            source: "registered-external-table-slot",
            id: "platform::timer-arm".to_owned(),
            evidence: "exact-pointer-cell-and-slot".to_owned(),
            event_dispatch: None,
        }),
        replacement_hint: Some("Rust async timer registration".to_owned()),
        project_symbol: None,
        project_candidates: Vec::new(),
        trampoline: Some(LinkedTrampoline {
            table: "platform".to_owned(),
            pointer_symbol: "platform_table_ptr".to_owned(),
            backing_symbol: "platform_table".to_owned(),
            version: 1,
            magic: 0x1234_5678,
            table_size: 0x100,
            magic_offset: 0xfc,
            function_id: "timer-arm".to_owned(),
            slot: 0x20,
            c_name: "timer_arm".to_owned(),
            argument_count: 1,
            return_model: "unmodeled".to_owned(),
            operation: "timer.arm-micros".to_owned(),
            return_type: "void".to_owned(),
            replacement_hint: Some("Rust async timer registration".to_owned()),
        }),
        argument_shapes: 1,
        arguments: vec!["arg0 + 0x4".to_owned()],
        argument_bindings: vec![LinkedArgumentBinding {
            position: 0,
            caller_argument: 0,
            offset: 4,
            expression: "arg0 + 0x4".to_owned(),
        }],
        typed_arguments: vec![LinkedCallArgument {
            position: 0,
            name: "timer".to_owned(),
            c_type: "*mut timer".to_owned(),
            direction: "input-output",
            value: "arg0 + 0x4".to_owned(),
        }],
        guard_paths: Some(vec![LinkedCallGuardPath {
            guards: vec![LinkedCallGuard {
                site: 0x0c,
                condition: "arg2 == 1".to_owned(),
                operation: "equal",
                taken: false,
                result_sources: Vec::new(),
                direct_mmio_sources: Vec::new(),
            }],
        }]),
    });
    leaf.context_accesses.push(ContextAccess {
        argument: 0,
        offset: 0x10,
        access: "write",
        width: 32,
        path: "entry / if arg1 != 0".to_owned(),
        value: Some("const:0x00000001".to_owned()),
        value_pseudo: Some("0x00000001".to_owned()),
        write_mask: Some(u32::MAX),
        preserved_mask: None,
        forced_zero_mask: Some(u32::MAX - 1),
        forced_one_mask: Some(1),
    });
    leaf.context_fields = context_fields_for_accesses(&leaf.context_accesses);

    let report = summarize_linked_ir(vec![root, middle, leaf]);
    let root = report
        .functions
        .iter()
        .find(|function| function.identity == "rom::root")
        .unwrap();
    assert!(root.effect_summary.context_projection_complete);
    assert!(root.effect_summary.context_projection_blockers.is_empty());
    assert_eq!(root.effect_summary.context_fields.len(), 1);
    let field = &root.effect_summary.context_fields[0];
    assert_eq!((field.argument, field.offset, field.width), (2, 0x28, 32));
    assert_eq!((field.reads, field.writes), (0, 1));
    assert_eq!(field.write_mask, u32::MAX);
    assert_eq!(field.origins, ["rom::leaf"]);
    assert_eq!(field.write_values, ["0x00000001"]);
    assert!(field.paths[0].contains("rom::root --call@0x00000010--> rom::middle"));
    assert!(field.paths[0].contains("rom::leaf / entry / if arg1 != 0"));
    assert_eq!(root.effect_summary.trampoline_calls.len(), 1);
    let trampoline = &root.effect_summary.trampoline_calls[0];
    assert_eq!(trampoline.origin, "rom::leaf");
    assert_eq!(trampoline.trampoline.slot, 0x20);
    assert_eq!(trampoline.trampoline.operation, "timer.arm-micros");
    assert_eq!(trampoline.arguments[0].binding, "affine-root-context");
    assert_eq!(trampoline.arguments[0].root_argument, Some(2));
    assert_eq!(trampoline.arguments[0].root_offset, Some(0x1c));
    assert_eq!(root.effect_summary.semantic_actions.len(), 1);
    let action = &root.effect_summary.semantic_actions[0];
    assert_eq!(action.operation, "timer.arm-micros");
    assert_eq!(action.origin, "rom::leaf");
    assert_eq!(action.site_path, [Some(0x10), Some(0x10), None]);
    assert!(
        action
            .path
            .contains("rom::root --call@0x00000010--> rom::middle")
    );
    assert!(
        action
            .path
            .ends_with("--semantic@composed--> platform::timer_arm")
    );
    assert_eq!(
        action
            .contract
            .as_ref()
            .map(|contract| contract.id.as_str()),
        Some("platform::timer-arm")
    );
    assert_eq!(action.arguments[0].binding, "affine-root-context");
    assert_eq!(action.arguments[0].root_argument, Some(2));
    assert_eq!(action.arguments[0].root_offset, Some(0x1c));
    let guard_scopes = action.guard_scopes.as_ref().unwrap();
    assert_eq!(guard_scopes.len(), 3);
    assert_eq!(guard_scopes[0].function, "rom::root");
    assert_eq!(guard_scopes[1].function, "rom::middle");
    assert_eq!(guard_scopes[2].function, "rom::leaf");
    assert!(!guard_scopes[2].paths[0].guards[0].taken);
    assert_eq!(report.trampoline_slots.len(), 1);
    assert_eq!(report.trampoline_slots[0].call_shapes, 1);
}
