use super::*;

fn write_ir(path: &std::path::Path) {
    let digest = "a".repeat(64);
    std::fs::write(
        path,
        r#"{
  "schema_version": 30,
  "command": "ir-export",
  "completeness_claim": false,
  "artifacts": [
    {
      "source": "rom",
      "artifact": {
        "path": "rom.elf",
        "sha256": "__ARTIFACT_DIGEST__"
      }
    }
  ],
  "functions": [
    {
      "source": "rom",
      "identity": "rom::vendor_irq",
      "member": null,
      "symbol": "vendor_irq",
      "selection": "symbol-prefix-root",
      "complete": true,
      "calls": [
        {
          "kind": "internal",
          "target": "rom::vendor_helper",
          "semantic_operation": null,
          "site": "0x00000080",
          "arguments": [],
          "cfg_guard_paths": [{"expression": "true"}]
        }
      ],
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
            "write_mask": "0xffffffff"
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
      "calls": [
        {
          "kind": "external",
          "target": "wifi_osi::queue_send_from_isr",
          "semantic_operation": "rtos.queue.send-from-isr",
          "site": "0x00000120",
          "arguments": ["?", "0x0000002a"],
          "cfg_guard_paths": [
            {"expression": "(arg0 & 0x00000001) != 0"}
          ]
        }
      ],
      "pseudo": "fn vendor_helper() { semantic.rtos_queue_send_from_isr(); }",
      "effect_summary": {
        "call_graph_closed": true,
        "context_projection_complete": true,
        "context_projection_blockers": [],
        "reachable_functions": [],
        "context_fields": [],
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
        "vendor-validator-function-pack-template-{}",
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
        "vendor-validator-function-pack-reviewed-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let report = directory.join("profile.json");
    let pack = directory.join("functions.toml");
    write_ir(&report);
    let reports = vec![("rom-phy".to_owned(), report)];
    let reviewed = r#"schema = 1
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
"#
    .replace("__ARTIFACT_DIGEST__", &"a".repeat(64));
    std::fs::write(&pack, reviewed).unwrap();
    let workspace = FunctionWorkspace::load(&reports, &pack).unwrap();
    let summary = workspace.summary();
    assert_eq!(summary.reviewed_functions, 1);
    assert_eq!(summary.reviewed_contexts, 1);
    assert_eq!(summary.reviewed_fields, 1);
    let binding = crate::interfaces::ResolvedInterfaceSlot {
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
        functions: ["vendor_helper".to_owned()].into(),
        calls: vec![crate::interfaces::ResolvedInterfaceCall {
            artifact: 0,
            member: None,
            function: "vendor_helper".to_owned(),
            function_address: 0x100,
            site: 0x120,
            kind: "call".to_owned(),
            jalr_offset: 0,
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
    assert!(report_text.contains("Reachable internal function reading views"));
    assert!(report_text.contains("`vendor_helper` — unreviewed"));
    assert!(report_text.contains("fn vendor_irq(ctx0: *mut u8)"));

    let stale = std::fs::read_to_string(&pack)
        .unwrap()
        .replace(&"a".repeat(64), &"b".repeat(64));
    std::fs::write(&pack, stale).unwrap();
    let error = FunctionWorkspace::load(&reports, &pack).unwrap_err();
    assert!(error.to_string().contains("stale function input digest"));
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ignored_context_covers_its_observed_fields_without_claiming_names() {
    let directory = std::env::temp_dir().join(format!(
        "vendor-validator-function-pack-ignored-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let report = directory.join("profile.json");
    let pack = directory.join("functions.toml");
    write_ir(&report);
    let reports = vec![("rom-phy".to_owned(), report)];
    let ignored = r#"schema = 1
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
