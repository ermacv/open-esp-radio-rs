//! Interface workspace validation tests.

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use super::*;
use crate::{
    ExternalArgumentDirection, ExternalArgumentSpec, ExternalFunctionSpec, ExternalReturnModel,
    ExternalSemanticSpec, ExternalTableRef, ExternalTableSpec, HarnessContractSpec,
};

const EXECUTION_ARGUMENTS: &[ExternalArgumentSpec] = &[
    ExternalArgumentSpec {
        name: "queue",
        c_type: "void *",
        direction: ExternalArgumentDirection::Input,
    },
    ExternalArgumentSpec {
        name: "item",
        c_type: "const void *",
        direction: ExternalArgumentDirection::Input,
    },
    ExternalArgumentSpec {
        name: "task_woken",
        c_type: "void *",
        direction: ExternalArgumentDirection::Output,
    },
];
const EXECUTION_FUNCTIONS: &[ExternalFunctionSpec] = &[ExternalFunctionSpec {
    id: "queue-send-from-isr",
    offset: 16,
    c_name: "queue_send_from_isr",
    argument_count: 3,
    return_model: ExternalReturnModel::Constant(1),
    semantic: ExternalSemanticSpec {
        operation: "rtos.queue.send-from-isr",
        arguments: EXECUTION_ARGUMENTS,
        return_type: "bool",
        replacement: Some("async.channel.try-send"),
        event_dispatch: None,
    },
}];
const EXECUTION_TABLE_SPEC: ExternalTableSpec = ExternalTableSpec {
    id: "fixture.services-v1",
    pointer_symbol: "g_services",
    backing_symbol: "services",
    version: 1,
    magic: 0x1234_5678,
    size: 32,
    magic_offset: 28,
    functions: EXECUTION_FUNCTIONS,
};
const EXECUTION_TABLE: ExternalTableRef = ExternalTableRef::new(&EXECUTION_TABLE_SPEC);
const EXECUTION_TABLES: &[ExternalTableRef] = &[EXECUTION_TABLE];
const EXECUTION_CONTRACTS: HarnessContractSpec = HarnessContractSpec {
    external_tables: EXECUTION_TABLES,
    entry_contracts: &[],
    diagnostic_calls: &[],
};

fn fake_digest(character: char) -> String {
    std::iter::repeat_n(character, 64).collect()
}

fn fixture_directory() -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "vendor-workbench-interface-pack-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn write_facts(path: &Path, digest: &str) {
    std::fs::write(
        path,
        format!(
            r#"{{
  "schema_version": 3,
  "command": "interfaces discover",
  "artifacts": [{{"index":0,"path":"libpp.a","sources":["libpp"],"sha256":"{digest}"}}],
  "calls": [{{
    "artifact":0,
    "member":"event.o",
    "function":"post_event",
    "function_address":"0x100",
    "site":"0x120",
    "kind":"call",
    "target":{{
      "root":{{
        "kind":"relocated-symbol",
        "member":"event.o",
        "symbol":"g_services",
        "addend":0,
        "addressing":"absolute"
      }},
      "loads":[
        {{"site":"0x110","offset":0,"width":32}},
        {{"site":"0x114","offset":16,"width":32}}
      ],
      "container_depth":1,
      "slot_offset":16,
      "jalr_offset":0
    }},
    "arguments":[
      {{"index":0,"kind":"unknown"}},
      {{"index":1,"kind":"constant","value":"0x0000002a"}},
      {{"index":2,"kind":"pointer-provenance","canonical":"arg0+4"}}
    ]
  }}],
  "table_candidates": [{{
    "artifact": 0,
    "root": {{
      "kind":"relocated-symbol",
      "member":"event.o",
      "symbol":"g_services",
      "addend":0,
      "addressing":"absolute"
    }},
    "container_path":[{{"offset":0,"width":32}}],
    "slots":[{{"offset":16,"width":32,"functions":["post_event"]}}],
    "functions":["post_event"]
  }}]
}}"#
        ),
    )
    .unwrap();
}

fn write_catalog(path: &Path) {
    std::fs::write(
        path,
        r#"schema = 1
id = "embedded"

[[operations]]
id = "rtos.queue.send-from-isr"
domain = "rtos"
summary = "Send from interrupt context"
argument-roles = ["queue", "item", "task-woken-out"]
return-role = "success"
effects = ["scheduler.queue-send"]
replacement = "async.channel.try-send"
"#,
    )
    .unwrap();
}

fn reviewed_pack(digest: &str, semantic: &str) -> String {
    format!(
        r#"schema = 1
id = "fixture"
calling-convention = "riscv-ilp32"

[[anchors]]
id = "wifi-osi"
status = "reviewed"
origin = "observed"
source = "libpp"
root-kind = "relocated-symbol"
member = "event.o"
symbol = "g_services"
addend = 0
addressing = "absolute"
container-path = [{{ offset = 0, width = 32 }}]
layout-version = "vendor-v9"
pointer-width = 32
layout-size = 32
slot-stride = 4

[[anchors.guards]]
kind = "artifact-sha256"
sha256 = "{digest}"

[[anchors.slots]]
offset = 16
width = 32
status = "reviewed"
origin = "observed"
name = "queue_send_from_isr"
arguments = ["opaque-handle", "const-ptr", "out-ptr"]
return = "bool"
semantic = "{semantic}"
"#
    )
}

fn executable_reviewed_pack(digest: &str) -> String {
    reviewed_pack(digest, "rtos.queue.send-from-isr")
        .replace(
            "slot-stride = 4\n",
            "slot-stride = 4\nexecution-contract = \"fixture.services-v1\"\n",
        )
        .replace(
            "semantic = \"rtos.queue.send-from-isr\"\n",
            "semantic = \"rtos.queue.send-from-isr\"\nexecution-model = \"queue-send-from-isr\"\n",
        )
}

#[test]
fn explicit_execution_model_resolves_without_promoting_semantic_annotation() {
    let directory = fixture_directory();
    let facts = directory.join("facts.json");
    let pack = directory.join("pack.toml");
    let catalog = directory.join("semantics.toml");
    let digest = fake_digest('e');
    write_facts(&facts, &digest);
    write_catalog(&catalog);
    std::fs::write(&pack, executable_reviewed_pack(&digest)).unwrap();

    let workspace = InterfaceWorkspace::load(
        &facts,
        &pack,
        &[catalog],
        "riscv-ilp32",
        Some(&EXECUTION_CONTRACTS),
    )
    .unwrap();

    assert_eq!(workspace.contracts().len(), 1);
    assert_eq!(workspace.contracts()[0].id, "fixture::wifi-osi");
    assert_eq!(
        workspace.contracts()[0]
            .execution_contract
            .as_ref()
            .unwrap()
            .id,
        "fixture.services-v1"
    );
    let slot = &workspace.bindings()[0];
    assert_eq!(slot.id, "fixture::wifi-osi@+0x10");
    assert_eq!(
        slot.semantic_annotation.as_ref().unwrap().operation,
        "rtos.queue.send-from-isr"
    );
    assert_eq!(
        slot.execution_model.as_ref().unwrap().return_model,
        ExternalReturnModel::Constant(1)
    );
    let instance = crate::execution_model::TableInstance {
        layout_id: "fixture::wifi-osi".to_owned(),
        base_address: 0x3fff_1000,
        layout_size: 32,
        pointer_cells: vec![0x3fff_0030],
        slots: vec![crate::execution_model::TableInstanceSlot {
            offset: 16,
            target: crate::execution_model::TableSlotTarget::Null,
        }],
    };
    workspace.validate_table_instance(&instance).unwrap();
    let mut stale = instance.clone();
    stale.slots[0].offset = 20;
    assert!(
        workspace
            .validate_table_instance(&stale)
            .unwrap_err()
            .to_string()
            .contains("no reviewed 32-bit slot")
    );
    stale.layout_size = 36;
    assert!(
        workspace
            .validate_table_instance(&stale)
            .unwrap_err()
            .to_string()
            .contains("requires size")
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn indexed_slot_evidence_requires_and_keeps_a_reviewed_index_domain() {
    let directory = fixture_directory();
    let facts = directory.join("facts.json");
    let pack = directory.join("pack.toml");
    let catalog = directory.join("semantics.toml");
    let digest = fake_digest('f');
    write_facts(&facts, &digest);
    let indexed = std::fs::read_to_string(&facts)
        .unwrap()
        .replace(
            r#"{"site":"0x114","offset":16,"width":32}"#,
            r#"{"site":"0x114","offset":0,"width":32,"selector":{"argument":0,"scale":4,"addend":0,"canonical":"arg0*4+0x0"}}"#,
        )
        .replace(r#""slot_offset":16"#, r#""slot_offset":null,"slot_selector":{"argument":0,"scale":4,"addend":0,"canonical":"arg0*4+0x0"}"#)
        .replace(
            r#"{"offset":16,"width":32,"functions":["post_event"]}"#,
            r#"{"offset":0,"width":32,"selector":{"argument":0,"scale":4,"addend":0,"canonical":"arg0*4+0x0"},"functions":["post_event"]}"#,
        );
    std::fs::write(&facts, indexed).unwrap();
    write_catalog(&catalog);
    let reviewed = reviewed_pack(&digest, "rtos.queue.send-from-isr")
        .replace("layout-size = 32", "layout-size = 8")
        .replace(
            "offset = 16\nwidth = 32\nstatus = \"reviewed\"\norigin = \"observed\"\nname = \"queue_send_from_isr\"",
            "offset = 0\nwidth = 32\nstatus = \"reviewed\"\norigin = \"manual\"\nname = \"slot_zero\"",
        )
        + r#"

[[anchors.index-domains]]
argument = 0
min = 0
max = 1
evidence = "reviewed caller branch plus exhaustive scenario arg-range"

[[anchors.slots]]
offset = 4
width = 32
status = "reviewed"
origin = "manual"
name = "slot_one"
arguments = ["opaque-handle", "const-ptr", "out-ptr"]
return = "bool"
semantic = "rtos.queue.send-from-isr"
"#;
    std::fs::write(&pack, &reviewed).unwrap();

    let workspace = InterfaceWorkspace::load(
        &facts,
        &pack,
        std::slice::from_ref(&catalog),
        "riscv-ilp32",
        None,
    )
    .unwrap();
    assert_eq!(workspace.summary().unreviewed_slots, 0);
    assert_eq!(workspace.summary().reviewed_slots, 1);
    assert_eq!(workspace.bindings().len(), 2);
    assert!(workspace.bindings().iter().all(|binding| {
        binding.calls.len() == 1
            && binding.calls[0].slot_selector.as_deref() == Some("arg0*4+0x0")
            && binding.calls[0].slot_index == u32::try_from(binding.offset / 4).ok()
            && binding.calls[0]
                .slot_index_domain
                .as_ref()
                .is_some_and(|domain| domain.min == 0 && domain.max == 1)
    }));
    let without_domain = reviewed.replace(
        "\n[[anchors.index-domains]]\nargument = 0\nmin = 0\nmax = 1\nevidence = \"reviewed caller branch plus exhaustive scenario arg-range\"\n",
        "",
    );
    std::fs::write(&pack, without_domain).unwrap();
    let incomplete = InterfaceWorkspace::load(
        &facts,
        &pack,
        std::slice::from_ref(&catalog),
        "riscv-ilp32",
        None,
    )
    .unwrap();
    assert_eq!(incomplete.summary().unreviewed_slots, 1);
    assert!(
        incomplete
            .bindings()
            .iter()
            .all(|binding| binding.calls.is_empty())
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn execution_model_requires_the_compiled_harness_contract() {
    let directory = fixture_directory();
    let facts = directory.join("facts.json");
    let pack = directory.join("pack.toml");
    let catalog = directory.join("semantics.toml");
    let digest = fake_digest('f');
    write_facts(&facts, &digest);
    write_catalog(&catalog);
    std::fs::write(&pack, executable_reviewed_pack(&digest)).unwrap();

    let error =
        InterfaceWorkspace::load(&facts, &pack, &[catalog], "riscv-ilp32", None).unwrap_err();
    std::fs::remove_dir_all(directory).unwrap();
    assert!(
        error
            .to_string()
            .contains("requires a configured compiled platform harness")
    );
}

#[test]
fn execution_model_identity_is_not_inferred_from_semantics_or_offset() {
    let directory = fixture_directory();
    let facts = directory.join("facts.json");
    let pack = directory.join("pack.toml");
    let catalog = directory.join("semantics.toml");
    let digest = fake_digest('9');
    write_facts(&facts, &digest);
    write_catalog(&catalog);
    let input = executable_reviewed_pack(&digest).replace(
        "execution-model = \"queue-send-from-isr\"",
        "execution-model = \"unknown-model\"",
    );
    std::fs::write(&pack, input).unwrap();

    let error = InterfaceWorkspace::load(
        &facts,
        &pack,
        &[catalog],
        "riscv-ilp32",
        Some(&EXECUTION_CONTRACTS),
    )
    .unwrap_err();
    std::fs::remove_dir_all(directory).unwrap();
    assert!(error.to_string().contains("has no call model"));
}

#[test]
fn reviewed_slot_links_observed_layout_to_reusable_semantics() {
    let directory = fixture_directory();
    let facts = directory.join("facts.json");
    let pack = directory.join("pack.toml");
    let catalog = directory.join("semantics.toml");
    let digest = fake_digest('a');
    write_facts(&facts, &digest);
    write_catalog(&catalog);
    std::fs::write(&pack, reviewed_pack(&digest, "rtos.queue.send-from-isr")).unwrap();

    let workspace =
        InterfaceWorkspace::load(&facts, &pack, &[catalog], "riscv-ilp32", None).unwrap();
    std::fs::remove_dir_all(directory).unwrap();
    assert_eq!(
        workspace.summary(),
        InterfaceWorkspaceSummary {
            fact_tables: 1,
            observed_slots: 1,
            observed_calls: 1,
            reviewed_anchors: 1,
            reviewed_slots: 1,
            semantic_links: 1,
            semantic_operations: 1,
            artifact_guards: 1,
            resolved_calls: 1,
            ..InterfaceWorkspaceSummary::default()
        }
    );
    assert_eq!(workspace.bindings().len(), 1);
    assert_eq!(workspace.bindings()[0].anchor, "wifi-osi");
    assert_eq!(workspace.bindings()[0].calls.len(), 1);
    assert_eq!(workspace.bindings()[0].calls[0].site, 0x120);
    assert_eq!(
        workspace.bindings()[0].calls[0].arguments[1].expression,
        "0x0000002a"
    );
    assert_eq!(
        workspace.bindings()[0]
            .functions
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["post_event"]
    );
}

#[test]
fn inconsistent_call_site_cannot_hide_behind_a_table_aggregate() {
    let directory = fixture_directory();
    let facts = directory.join("facts.json");
    let digest = fake_digest('a');
    write_facts(&facts, &digest);
    let inconsistent = std::fs::read_to_string(&facts)
        .unwrap()
        .replace("\"slot_offset\":16", "\"slot_offset\":20");
    std::fs::write(&facts, inconsistent).unwrap();

    let error = InterfaceFacts::load(&facts).unwrap_err();
    std::fs::remove_dir_all(directory).unwrap();
    assert!(
        error
            .to_string()
            .contains("inconsistent container/slot metadata")
    );
}

#[test]
fn one_site_may_preserve_multiple_argument_provenance_variants() {
    let directory = fixture_directory();
    let facts = directory.join("facts.json");
    let digest = fake_digest('a');
    write_facts(&facts, &digest);
    let mut document =
        serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(&facts).unwrap())
            .unwrap();
    let calls = document["calls"].as_array_mut().unwrap();
    let mut alternate = calls[0].clone();
    alternate["arguments"][1]["value"] = serde_json::json!("0x0000002b");
    calls.push(alternate);
    std::fs::write(&facts, serde_json::to_string(&document).unwrap()).unwrap();

    let loaded = InterfaceFacts::load(&facts).unwrap();
    std::fs::remove_dir_all(directory).unwrap();
    assert_eq!(loaded.calls.len(), 2);
}

#[test]
fn artifact_guard_mismatch_makes_an_observed_anchor_stale() {
    let directory = fixture_directory();
    let facts = directory.join("facts.json");
    let pack = directory.join("pack.toml");
    let catalog = directory.join("semantics.toml");
    let digest = fake_digest('a');
    write_facts(&facts, &digest);
    write_catalog(&catalog);
    let other = fake_digest('b');
    std::fs::write(&pack, reviewed_pack(&other, "rtos.queue.send-from-isr")).unwrap();

    let error =
        InterfaceWorkspace::load(&facts, &pack, &[catalog], "riscv-ilp32", None).unwrap_err();
    std::fs::remove_dir_all(directory).unwrap();
    assert!(error.to_string().contains("stale"));
    assert!(error.to_string().contains(&pack.display().to_string()));
}

#[test]
fn unknown_semantic_operation_is_not_accepted_by_name() {
    let directory = fixture_directory();
    let facts = directory.join("facts.json");
    let pack = directory.join("pack.toml");
    let catalog = directory.join("semantics.toml");
    let digest = fake_digest('a');
    write_facts(&facts, &digest);
    write_catalog(&catalog);
    let pack_text = reviewed_pack(&digest, "rtos.magic-name");
    let expected_span = pack_text.find("\"rtos.magic-name\"").unwrap();
    std::fs::write(&pack, &pack_text).unwrap();

    let error =
        InterfaceWorkspace::load(&facts, &pack, &[catalog], "riscv-ilp32", None).unwrap_err();
    assert!(error.to_string().contains("unknown semantic operation"));
    assert!(error.to_string().contains(&pack.display().to_string()));
    let actual_span = match error {
        crate::error::WorkbenchError::ManifestSource { span, .. } => span.offset(),
        error => panic!("expected source diagnostic, got {error:?}"),
    };
    assert_eq!(actual_span, expected_span);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn generated_pack_is_unreviewed_valid_and_never_overwritten() {
    let directory = fixture_directory();
    let facts_path = directory.join("facts.json");
    let pack_path = directory.join("pack.toml");
    let digest = fake_digest('a');
    write_facts(&facts_path, &digest);
    let facts = InterfaceFacts::load(&facts_path).unwrap();
    write_pack_template(&pack_path, &facts, "fixture", "riscv-ilp32").unwrap();

    let workspace = InterfaceWorkspace::load(
        &facts_path,
        &pack_path,
        &[] as &[PathBuf],
        "riscv-ilp32",
        None,
    )
    .unwrap();
    let overwrite = write_pack_template(&pack_path, &facts, "fixture", "riscv-ilp32")
        .unwrap_err()
        .to_string();
    std::fs::remove_dir_all(directory).unwrap();
    assert_eq!(workspace.summary().unreviewed_anchors, 1);
    assert_eq!(workspace.summary().unreviewed_slots, 1);
    assert!(overwrite.contains("refusing to overwrite"));
}

#[test]
fn runtime_version_guard_can_replace_artifact_pinning() {
    let directory = fixture_directory();
    let facts = directory.join("facts.json");
    let pack = directory.join("pack.toml");
    let digest = fake_digest('a');
    write_facts(&facts, &digest);
    std::fs::write(
        &pack,
        r#"schema = 1
id = "fixture"
calling-convention = "riscv-ilp32"

[[anchors]]
id = "wifi-osi"
status = "reviewed"
source = "libpp"
root-kind = "relocated-symbol"
member = "event.o"
symbol = "g_services"
addressing = "absolute"
container-path = [{ offset = 0, width = 32 }]
layout-version = "runtime-v9"
pointer-width = 32
layout-size = 32
slot-stride = 4

[[anchors.guards]]
kind = "runtime-value"
purpose = "version"
offset = 0
width = 32
mask = 0xffffffff
value = 9

[[anchors.slots]]
offset = 16
width = 32
status = "reviewed"
name = "unknown_callback"
arguments = []
return = "void"
"#,
    )
    .unwrap();

    let workspace =
        InterfaceWorkspace::load(&facts, &pack, &[] as &[PathBuf], "riscv-ilp32", None).unwrap();
    std::fs::remove_dir_all(directory).unwrap();
    assert_eq!(workspace.summary().runtime_guards, 1);
    assert_eq!(workspace.summary().reviewed_slots, 1);
}

#[test]
fn shipped_catalog_covers_the_initial_cross_platform_domains() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("catalogs/embedded-semantics.toml");
    let catalogs = SemanticCatalogs::load(&[path]).unwrap();
    for operation in [
        "rtos.queue.send-from-isr",
        "storage.nvs.read",
        "logging.record",
        "time.blocking-delay",
    ] {
        assert!(catalogs.get(operation).is_some(), "missing {operation}");
    }
}
