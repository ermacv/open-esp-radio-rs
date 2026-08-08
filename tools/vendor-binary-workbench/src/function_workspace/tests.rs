use super::*;

fn write_ir(path: &std::path::Path) {
    let digest = "a".repeat(64);
    std::fs::write(
        path,
        r#"{
  "schema_version": 35,
  "command": "ir export",
  "completeness_claim": false,
  "mmio_field_semantics_claim": false,
  "artifacts": [
    {
      "source": "rom",
      "artifact": {
        "path": "rom.elf",
        "sha256": "__ARTIFACT_DIGEST__"
      }
    }
  ],
  "mmio_registers": [],
  "functions": [
    {
      "source": "rom",
      "identity": "rom::vendor_irq",
      "member": null,
      "symbol": "vendor_irq",
      "selection": "symbol-prefix-root",
      "complete": true,
      "mmio_accesses": [],
      "calls": [
        {
          "kind": "internal",
          "target": "rom::vendor_helper",
          "semantic_operation": null,
          "site": 128,
          "arguments": [],
          "guard_paths": [{"guards": []}]
        }
      ],
      "scenario_suggestions": [],
      "pseudo": "fn vendor_irq(ctx0: *mut u8) { ctx0.write32(+0x4, value); }",
      "effect_summary": {
        "call_graph_closed": true,
        "context_projection_complete": true,
        "context_projection_blockers": [],
        "reachable_functions": ["rom::vendor_helper"],
        "context_fields": [
          {
            "argument": 0,
            "offset": 4,
            "width": 32,
            "reads": 1,
            "writes": 1,
            "write_mask": 4294967295
          }
        ],
        "memory_fields": [
          {
            "object": {"kind": "argument", "index": 0},
            "offset": 4,
            "width": 32,
            "reads": 1,
            "writes": 1,
            "write_mask": 4294967295,
            "origins": ["rom::vendor_irq"],
            "paths": ["entry"],
            "write_values": ["value"]
          }
        ],
        "semantic_operations": [
          {"operation": "rtos.queue.send"}
        ],
        "trampoline_calls": [],
        "event_dispatches": []
      }
    },
    {
      "source": "rom",
      "identity": "rom::vendor_helper",
      "member": null,
      "symbol": "vendor_helper",
      "selection": "reachable-internal",
      "complete": true,
      "mmio_accesses": [],
      "calls": [
        {
          "kind": "external",
          "target": "wifi_osi::queue_send_from_isr",
          "semantic_operation": "rtos.queue.send-from-isr",
          "site": 288,
          "arguments": ["?", "0x0000002a"],
          "guard_paths": [{
            "guards": [{
              "site": 288,
              "condition": "(arg0 & 0x00000001) != 0",
              "operation": "not-equal",
              "taken": true,
              "result_sources": [],
              "direct_mmio_sources": []
            }]
          }]
        }
      ],
      "scenario_suggestions": [],
      "pseudo": "fn vendor_helper() { semantic.rtos_queue_send_from_isr(); }",
      "effect_summary": {
        "call_graph_closed": true,
        "context_projection_complete": true,
        "context_projection_blockers": [],
        "reachable_functions": [],
        "context_fields": [],
        "memory_fields": [
          {
            "object": {"kind": "global", "member": "state.o", "symbol": "phy_state"},
            "offset": 12,
            "width": 16,
            "reads": 0,
            "writes": 1,
            "write_mask": 65535,
            "origins": ["rom::vendor_helper"],
            "paths": ["entry"],
            "write_values": ["7"]
          }
        ],
        "semantic_operations": [
          {"operation": "rtos.queue.send"}
        ],
        "trampoline_calls": [],
        "event_dispatches": []
      }
    }
  ]
}"#
        .replace("__ARTIFACT_DIGEST__", &digest),
    )
    .unwrap();
}

#[test]
fn generated_template_is_valid_unreviewed_workspace() {
    let directory = std::env::temp_dir().join(format!(
        "vendor-workbench-function-pack-template-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let report = directory.join("profile.json");
    let pack = directory.join("functions.toml");
    write_ir(&report);
    let reports = vec![("rom-phy".to_owned(), report)];
    let facts = FunctionFacts::load(&reports).unwrap();
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
    let report = directory.join("profile.json");
    let pack = directory.join("functions.toml");
    write_ir(&report);
    let reports = vec![("rom-phy".to_owned(), report)];
    let reviewed = r#"schema = 2
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
    assert_eq!(summary.type_fields, 2);
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
        execution_model: Some(crate::interfaces::ResolvedExternalCallExecutionModel {
            id: "fixture.services-v1.queue-send-from-isr".to_owned(),
            table: "fixture.services-v1".to_owned(),
            function: "queue-send-from-isr".to_owned(),
            c_name: "queue_send_from_isr".to_owned(),
            return_model: crate::ExternalReturnModel::Constant(1),
        }),
        functions: ["vendor_helper".to_owned()].into(),
        calls: vec![crate::interfaces::ResolvedInterfaceCall {
            artifact: 0,
            member: None,
            function: "vendor_helper".to_owned(),
            function_address: 0x100,
            site: 0x120,
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
    assert!(report_text.contains("Validated interface call sites"));
    assert!(report_text.contains("a1=0x0000002a"));
    assert!(report_text.contains("(arg0 & 0x00000001) != 0"));
    assert!(report_text.contains("`rtos.queue.send-from-isr`"));
    assert!(report_text.contains("`fixture.services-v1.queue-send-from-isr`"));
    assert!(report_text.contains("Reachable internal function reading views"));
    assert!(report_text.contains("`vendor_helper` — unreviewed"));
    assert!(report_text.contains("fn vendor_irq(ctx0: *mut u8)"));
    assert!(report_text.contains("Reviewed logical types"));
    assert!(report_text.contains("`PhyGlobalState`"));
    assert!(report_text.contains("`state.o::phy_state`"));

    let conflicting = reviewed.replace("offset = 12\nwidth = 16", "offset = 12\nwidth = 32");
    std::fs::write(&pack, conflicting).unwrap();
    let error = FunctionWorkspace::load(&reports, &pack).unwrap_err();
    assert!(error.to_string().contains("duplicate or unobserved field"));
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
    let report = directory.join("profile.json");
    let pack = directory.join("functions.toml");
    write_ir(&report);
    let reports = vec![("rom-phy".to_owned(), report)];
    let ignored = r#"schema = 2
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
