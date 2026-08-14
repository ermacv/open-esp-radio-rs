use super::*;

#[test]
fn schema_v9_parses_reviewed_event_delivery_and_case_handler() {
    let directory = std::env::temp_dir().join(format!(
        "vendor-workbench-function-event-route-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let pack_path = directory.join("functions.toml");
    std::fs::write(
        &pack_path,
        r#"schema = 9
id = "fixture"

[[event-routes]]
id = "rx-ready"
profile = "linked"
source = "vendor"
dispatcher = "vendor::post_rx"
mechanism = "internal-signal"
selector-role = "selector"
selector-value = 25
receiver = "fixture::worker"
execution-context = "task"
consumer-profile = "linked"
consumer-source = "vendor"
consumer-entry = "vendor::worker"
delivery-operation = "rtos.queue.receive"
delivery-output-role = "item-out"
delivery-selector-offset = 0
delivery-selector-width = 32
delivery-encoding = "little-endian"
case-handler-profile = "linked"
case-handler-source = "vendor"
case-handler-function = "vendor::handle_rx"
terminal-profile = "linked"
terminal-source = "vendor"
terminal-function = "vendor::recycle_rx"
rationale = "Reviewed scheduler table maps signal 25 to the worker entry."
"#,
    )
    .unwrap();
    let pack = FunctionPack::load_reviewed(&pack_path).unwrap();
    assert_eq!(pack.event_routes.len(), 1);
    assert_eq!(pack.event_routes[0].selector_value, 25);
    assert_eq!(pack.event_routes[0].consumer_entry, "vendor::worker");
    assert_eq!(
        pack.event_routes[0].case_handler.as_ref().unwrap().function,
        "vendor::handle_rx"
    );
    assert_eq!(
        pack.event_routes[0].terminal.as_ref().unwrap().function,
        "vendor::recycle_rx"
    );
    assert!(pack.event_routes[0].replay.is_none());
    std::fs::remove_dir_all(directory).unwrap();
}

fn write_ir(path: &std::path::Path) {
    let digest = "a".repeat(64);
    let call = |kind: &'static str,
                target: &str,
                site: u32,
                semantic_operation: Option<String>,
                arguments: Vec<String>,
                guards: Vec<crate::LinkedCallGuard>| crate::LinkedCall {
        kind,
        target: target.to_owned(),
        site: Some(site),
        tail: false,
        result_modeled: false,
        execution_model: None,
        semantics: None,
        semantic_operation,
        semantic_contract: None,
        replacement_hint: None,
        project_symbol: None,
        project_candidates: Vec::new(),
        trampoline: None,
        argument_shapes: 1,
        arguments,
        argument_bindings: Vec::new(),
        typed_arguments: Vec::new(),
        guard_paths: Some(vec![crate::LinkedCallGuardPath { guards }]),
    };
    let provenance = || crate::LinkedReturnProvenance {
        exact: false,
        known_zero_bits: 0,
        known_one_bits: 0,
        unknown_bits: u32::MAX,
        sources: Vec::new(),
    };
    let semantic_summary = || crate::LinkedSummarySemantic {
        operation: "rtos.queue.send".to_owned(),
        call_shapes: 1,
        targets: vec!["wifi_osi::queue_send_from_isr".to_owned()],
        replacement_hints: Vec::new(),
        origins: vec!["rom::vendor_helper".to_owned()],
    };
    let mut irq_effects = crate::LinkedEffectSummary {
        transitive_effects_materialized: true,
        call_graph_closed: true,
        reachable_function_count: 1,
        context_projection_materialized: true,
        context_projection_complete: true,
        ..crate::LinkedEffectSummary::default()
    };
    irq_effects
        .context_fields
        .push(crate::LinkedSummaryContextField {
            argument: 0,
            offset: 4,
            width: 32,
            reads: 1,
            writes: 1,
            write_mask: u32::MAX,
            origins: vec!["rom::vendor_irq".to_owned()],
            paths: vec!["entry".to_owned()],
            write_values: vec!["value".to_owned()],
        });
    irq_effects
        .memory_fields
        .push(crate::LinkedSummaryMemoryField {
            object: crate::LinkedMemoryObject::Argument { index: 0 },
            offset: 4,
            width: 32,
            reads: 1,
            writes: 1,
            write_mask: u32::MAX,
            origins: vec!["rom::vendor_irq".to_owned()],
            paths: vec!["entry".to_owned()],
            write_values: vec!["value".to_owned()],
        });
    irq_effects.semantic_operations.push(semantic_summary());

    let mut helper_effects = crate::LinkedEffectSummary {
        transitive_effects_materialized: true,
        call_graph_closed: true,
        context_projection_materialized: true,
        context_projection_complete: true,
        ..crate::LinkedEffectSummary::default()
    };
    helper_effects
        .memory_fields
        .push(crate::LinkedSummaryMemoryField {
            object: crate::LinkedMemoryObject::Global {
                member: Some("state.o".to_owned()),
                symbol: "phy_state".to_owned(),
            },
            offset: 12,
            width: 16,
            reads: 0,
            writes: 1,
            write_mask: 65535,
            origins: vec!["rom::vendor_helper".to_owned()],
            paths: vec!["entry".to_owned()],
            write_values: vec!["7".to_owned()],
        });
    // A narrow semantic field may overlap a machine-word access performed by
    // a structure copy. Both are valid observations and remain distinct
    // review items identified by `(offset, width)`.
    helper_effects
        .memory_fields
        .push(crate::LinkedSummaryMemoryField {
            object: crate::LinkedMemoryObject::Global {
                member: Some("state.o".to_owned()),
                symbol: "phy_state".to_owned(),
            },
            offset: 12,
            width: 32,
            reads: 1,
            writes: 0,
            write_mask: 0,
            origins: vec!["rom::vendor_helper".to_owned()],
            paths: vec!["entry / structure copy".to_owned()],
            write_values: Vec::new(),
        });
    helper_effects.semantic_operations.push(semantic_summary());

    let function = |identity: &str,
                    symbol: &str,
                    selection: &'static str,
                    object_offset: u32,
                    calls: Vec<crate::LinkedCall>,
                    effect_summary: crate::LinkedEffectSummary,
                    pseudo: &str| crate::LinkedIrFunction {
        source: "rom".to_owned(),
        identity: identity.to_owned(),
        selection,
        member: None,
        symbol: symbol.to_owned(),
        binding: "global",
        address: Some(object_offset),
        object_offset,
        size: 4,
        flow_kind: "linear",
        complete: true,
        exact: true,
        return_value: "?".to_owned(),
        return_provenance: provenance(),
        dependencies: Vec::new(),
        projected_relocations: Vec::new(),
        local_value_flow: Vec::new(),
        calls,
        direct_mmio_predicates: Vec::new(),
        mmio_accesses: Vec::new(),
        instruction_effects: Vec::new(),
        delays: Vec::new(),
        context_accesses: Vec::new(),
        context_fields: Vec::new(),
        memory_accesses: Vec::new(),
        memory_fields: Vec::new(),
        scenario_suggestions: Vec::new(),
        effect_summary,
        call_graph_diagnostics: Vec::new(),
        direct_diagnostics: Vec::new(),
        reference_diagnostics: Vec::new(),
        decode_blockers: Vec::new(),
        pseudo: pseudo.to_owned(),
    };
    let mut functions = vec![
        function(
            "rom::vendor_irq",
            "vendor_irq",
            "symbol-prefix-root",
            256,
            vec![call(
                "internal",
                "rom::vendor_helper",
                128,
                None,
                Vec::new(),
                Vec::new(),
            )],
            irq_effects,
            "fn vendor_irq(ctx0: *mut u8) { ctx0.write32(+0x4, value); }",
        ),
        function(
            "rom::vendor_helper",
            "vendor_helper",
            "reachable-internal",
            512,
            vec![call(
                "external",
                "wifi_osi::queue_send_from_isr",
                288,
                Some("rtos.queue.send-from-isr".to_owned()),
                vec!["?".to_owned(), "0x0000002a".to_owned()],
                vec![crate::LinkedCallGuard {
                    site: 288,
                    condition: "(arg0 & 0x00000001) != 0".to_owned(),
                    operation: "not-equal",
                    taken: true,
                    result_sources: Vec::new(),
                    direct_mmio_sources: Vec::new(),
                }],
            )],
            helper_effects,
            "fn vendor_helper() { semantic.rtos_queue_send_from_isr(); }",
        ),
    ];
    functions[0]
        .decode_blockers
        .push(crate::LinkedDecodeBlocker {
            address: 0x118,
            width: 2,
            raw: 0,
            class: "zero-fill-or-illegal-trap",
            linear_control_flow: false,
        });
    let mut document: serde_json::Value = serde_json::from_str(
        &crate::artifacts::render_linked_ir_fixture(functions, Vec::new()),
    )
    .unwrap();
    document["artifacts"] = serde_json::json!([{
        "source": "rom",
        "artifact": {"path": "rom.elf", "sha256": digest},
        "reviewed_code_boundaries": []
    }]);
    crate::artifacts::write_fixture_bundle(path, &serde_json::to_string_pretty(&document).unwrap())
        .unwrap();
}

#[test]
fn generated_template_is_valid_unreviewed_workspace() {
    let directory = std::env::temp_dir().join(format!(
        "vendor-workbench-function-pack-template-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let report = directory.join("profile.ir");
    let pack = directory.join("functions.toml");
    write_ir(&report);
    let reports = vec![("rom-phy".to_owned(), report)];
    let facts = FunctionFacts::load(&reports).unwrap();
    let irq = facts
        .functions
        .iter()
        .find(|function| function.identity == "rom::vendor_irq")
        .unwrap();
    assert_eq!(irq.decode_blockers.len(), 1);
    assert_eq!(irq.decode_blockers[0].class, "zero-fill-or-illegal-trap");
    assert_eq!(irq.decode_blockers[0].address, 0x118);
    let helper = facts
        .functions
        .iter()
        .find(|function| function.identity == "rom::vendor_helper")
        .unwrap();
    assert!(
        helper
            .calls
            .iter()
            .any(|call| { call.semantic_operation.as_deref() == Some("rtos.queue.send-from-isr") })
    );
    write_function_pack_template(&pack, &facts, "fixture").unwrap();
    let workspace = FunctionWorkspace::load(&reports, &pack).unwrap();
    let summary = workspace.summary();
    assert_eq!(summary.inputs, 1);
    assert_eq!(summary.observed_functions, 1);
    assert_eq!(summary.unreviewed_functions, 1);
    assert_eq!(summary.unreviewed_contexts, 1);
    assert_eq!(summary.unreviewed_fields, 1);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn reviewed_names_require_matching_digest_and_complete_explicit_claims() {
    let directory = std::env::temp_dir().join(format!(
        "vendor-workbench-function-pack-reviewed-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let report = directory.join("profile.ir");
    let pack = directory.join("functions.toml");
    write_ir(&report);
    let reports = vec![("rom-phy".to_owned(), report)];
    let reviewed = r#"schema = 9
id = "fixture"

[[inputs]]
profile = "rom-phy"
source = "rom"
artifact-sha256 = "__ARTIFACT_DIGEST__"

[[functions]]
profile = "rom-phy"
source = "rom"
identity = "rom::vendor_irq"
status = "reviewed"
name = "vendor_interrupt_handler"
role = "interrupt.handler"
summary = "Posts the recovered event after updating caller-owned state."

[functions.signature]
return-abi = "void"

[[functions.signature.arguments]]
index = 0
name = "state"
abi = "mut-ptr"
role = "state"

[[functions.contexts]]
argument = 0
status = "reviewed"
name = "state"
type-name = "VendorState"

[[functions.contexts.fields]]
offset = 4
width = 32
status = "reviewed"
name = "pending_events"
display-type = "u32"
description = "Observed read/write event word."

[[types]]
id = "vendor-state"
name = "VendorState"
description = "Logical state shared by reviewed function projections."

[[types.bindings]]
profile = "rom-phy"
source = "rom"
name = "irq_state"
kind = "argument"
function = "rom::vendor_irq"
argument = 0

[[types.fields]]
offset = 4
width = 32
status = "reviewed"
name = "pending_events"
display-type = "u32"
description = "Observed event word unified across bound objects."

[[types]]
id = "phy-global-state"
name = "PhyGlobalState"

[[types.bindings]]
profile = "rom-phy"
source = "rom"
name = "phy_state_global"
kind = "global"
member = "state.o"
symbol = "phy_state"

[[types.fields]]
offset = 12
width = 16
status = "reviewed"
name = "calibration_state"
display-type = "u16"
"#
    .replace("__ARTIFACT_DIGEST__", &"a".repeat(64));
    std::fs::write(&pack, &reviewed).unwrap();
    let workspace = FunctionWorkspace::load(&reports, &pack).unwrap();
    let summary = workspace.summary();
    assert_eq!(summary.reviewed_functions, 1);
    assert_eq!(summary.reviewed_contexts, 1);
    assert_eq!(summary.reviewed_fields, 1);
    assert_eq!(summary.logical_types, 2);
    assert_eq!(summary.type_bindings, 2);
    assert_eq!(summary.type_fields, 3);
    assert_eq!(summary.unreviewed_type_fields, 1);
    let signature = workspace.pack.functions[0].signature.as_ref().unwrap();
    assert_eq!(signature.arguments[0].name, "state");
    assert_eq!(signature.return_abi.as_deref(), Some("void"));
    let binding = crate::interfaces::ResolvedInterfaceSlot {
        id: "fixture::wifi-osi@+0x38".to_owned(),
        contract: "fixture::wifi-osi".to_owned(),
        anchor: "wifi-osi".to_owned(),
        source: "rom".to_owned(),
        layout_version: "vendor-v9".to_owned(),
        offset: 0x38,
        width: 32,
        name: "queue_send_from_isr".to_owned(),
        arguments: vec!["opaque-handle".to_owned(), "const-ptr".to_owned()],
        return_type: "bool".to_owned(),
        variadic: false,
        semantic: Some("rtos.queue.send-from-isr".to_owned()),
        semantic_annotation: None,
        execution_model_set: Some("fixture.services-v1".to_owned()),
        execution_model: Some(crate::interfaces::ResolvedExternalCallExecutionModel {
            id: "fixture.services-v1.queue-send-from-isr".to_owned(),
            set: "fixture.services-v1".to_owned(),
            model: "queue-send-from-isr".to_owned(),
            return_model: crate::ExternalReturnModel::Constant(1),
            outputs: Vec::new(),
        }),
        functions: ["vendor_helper".to_owned()].into(),
        calls: vec![crate::interfaces::ResolvedInterfaceCall {
            artifact: 0,
            member: None,
            function: "vendor_helper".to_owned(),
            function_address: 0x100,
            site: 0x120,
            slot_load_site: Some(0x118),
            kind: "call".to_owned(),
            jalr_offset: 0,
            slot_selector: None,
            slot_index: None,
            slot_index_domain: None,
            arguments: vec![
                crate::interfaces::ResolvedInterfaceArgument {
                    index: 0,
                    kind: "unknown".to_owned(),
                    expression: "?".to_owned(),
                },
                crate::interfaces::ResolvedInterfaceArgument {
                    index: 1,
                    kind: "constant".to_owned(),
                    expression: "0x0000002a".to_owned(),
                },
            ],
        }],
    };
    let mut mismatch = binding.clone();
    mismatch.semantic = Some("rtos.queue.receive".to_owned());
    let error = link_reviewed_interfaces(&workspace, &[mismatch]).unwrap_err();
    assert!(error.to_string().contains("interface semantic mismatch"));

    let links = link_reviewed_interfaces(&workspace, &[binding]).unwrap();
    assert_eq!(links.len(), 2);
    assert!(links.iter().all(|link| {
        link.calls.len() == 1
            && link.calls[0].linked_ir_matches == 1
            && link.calls[0].linked_ir.is_some()
            && link.contract == "fixture::wifi-osi"
            && link.slot == "fixture::wifi-osi@+0x38"
            && link.execution_model.as_deref() == Some("fixture.services-v1.queue-send-from-isr")
    }));
    let report_text = render_function_review(&workspace, Some(&links)).unwrap();
    assert!(report_text.contains("`vendor_interrupt_handler` — reviewed"));
    assert!(report_text.contains("`pending_events`"));
    assert!(report_text.contains("`state`: `VendorState`"));
    assert!(report_text.contains("`rtos.queue.send`"));
    assert!(report_text.contains("Decode blockers: 1 total"));
    assert!(report_text.contains("`zero-fill-or-illegal-trap` at `0x118`"));
    assert!(report_text.contains("Validated interface call sites"));
    assert!(report_text.contains("a1=0x0000002a"));
    assert!(report_text.contains("(arg0 & 0x00000001) != 0"));
    assert!(report_text.contains("`rtos.queue.send-from-isr`"));
    assert!(report_text.contains("`fixture.services-v1.queue-send-from-isr`"));
    assert!(report_text.contains("Unreviewed reachable function inventory"));
    assert!(report_text.contains("`vendor_helper`"));
    assert!(report_text.contains("fn vendor_irq(ctx0: *mut u8)"));
    assert!(!report_text.contains("fn vendor_helper"));
    assert!(report_text.contains("Reviewed logical types"));
    assert!(report_text.contains("`PhyGlobalState`"));
    assert!(report_text.contains("`state.o::phy_state`"));
    assert!(report_text.contains("Vendor functions"));
    assert!(
        report_text.contains("| R/W | `rom::vendor_irq` |"),
        "{report_text}"
    );

    let conflicting = reviewed.replace("offset = 12\nwidth = 16", "offset = 12\nwidth = 24");
    std::fs::write(&pack, conflicting).unwrap();
    let error = FunctionWorkspace::load(&reports, &pack).unwrap_err();
    assert!(error.to_string().contains("duplicate or unobserved field"));
    std::fs::write(&pack, &reviewed).unwrap();

    let gapped_signature =
        reviewed.replace("index = 0\nname = \"state\"", "index = 1\nname = \"state\"");
    std::fs::write(&pack, gapped_signature).unwrap();
    let error = FunctionWorkspace::load(&reports, &pack).unwrap_err();
    assert!(error.to_string().contains("indices must be contiguous"));
    std::fs::write(&pack, &reviewed).unwrap();

    let stale = std::fs::read_to_string(&pack)
        .unwrap()
        .replace(&"a".repeat(64), &"b".repeat(64));
    let expected_span = stale.find(&format!("\"{}\"", "b".repeat(64))).unwrap();
    std::fs::write(&pack, &stale).unwrap();
    let error = FunctionWorkspace::load(&reports, &pack).unwrap_err();
    assert!(error.to_string().contains("stale function input digest"));
    assert!(error.to_string().contains(&pack.display().to_string()));
    let actual_span = match error {
        crate::error::WorkbenchError::ManifestSource { span, .. } => span.offset(),
        error => panic!("expected source diagnostic, got {error:?}"),
    };
    assert_eq!(actual_span, expected_span);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ignored_context_covers_its_observed_fields_without_claiming_names() {
    let directory = std::env::temp_dir().join(format!(
        "vendor-workbench-function-pack-ignored-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let report = directory.join("profile.ir");
    let pack = directory.join("functions.toml");
    write_ir(&report);
    let reports = vec![("rom-phy".to_owned(), report)];
    let ignored = r#"schema = 9
id = "fixture"

[[inputs]]
profile = "rom-phy"
source = "rom"
artifact-sha256 = "__ARTIFACT_DIGEST__"

[[functions]]
profile = "rom-phy"
source = "rom"
identity = "rom::vendor_irq"
status = "reviewed"
name = "vendor_interrupt_handler"
role = "interrupt.handler"
summary = "Posts an event."

[[functions.contexts]]
argument = 0
status = "ignored"
"#
    .replace("__ARTIFACT_DIGEST__", &"a".repeat(64));
    std::fs::write(&pack, ignored).unwrap();
    let summary = FunctionWorkspace::load(&reports, &pack).unwrap().summary();
    assert_eq!(summary.ignored_contexts, 1);
    assert_eq!(summary.ignored_fields, 1);
    assert_eq!(summary.unreviewed_contexts, 0);
    assert_eq!(summary.unreviewed_fields, 0);
    std::fs::remove_dir_all(directory).unwrap();
}
