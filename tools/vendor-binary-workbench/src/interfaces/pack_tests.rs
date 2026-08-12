//! Interface workspace validation tests.

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use super::*;
use crate::{
    ExternalCallModelSetRef, ExternalCallModelSetSpec, ExternalCallModelSpec, ExternalOutputModel,
    ExternalReturnModel, HarnessContractSpec,
};

const EXECUTION_MODELS: &[ExternalCallModelSpec] = &[ExternalCallModelSpec {
    id: "queue-send-from-isr",
    return_model: ExternalReturnModel::Constant(1),
    outputs: &[],
}];
const EXECUTION_MODEL_SET_SPEC: ExternalCallModelSetSpec = ExternalCallModelSetSpec {
    id: "fixture.services-v1",
    models: EXECUTION_MODELS,
};
const EXECUTION_MODEL_SET: ExternalCallModelSetRef =
    ExternalCallModelSetRef::new(&EXECUTION_MODEL_SET_SPEC);
const EXECUTION_MODEL_SETS: &[ExternalCallModelSetRef] = &[EXECUTION_MODEL_SET];
const EXECUTION_CONTRACTS: HarnessContractSpec = HarnessContractSpec {
    external_call_model_sets: EXECUTION_MODEL_SETS,
    entry_contracts: &[],
    diagnostic_calls: &[],
};
const INVALID_OUTPUTS: &[ExternalOutputModel] = &[ExternalOutputModel::PrivateStack {
    pointer_argument: 1,
    width: 8,
}];
const INVALID_EXECUTION_MODELS: &[ExternalCallModelSpec] = &[ExternalCallModelSpec {
    id: "queue-send-from-isr",
    return_model: ExternalReturnModel::SymbolicU32,
    outputs: INVALID_OUTPUTS,
}];
const INVALID_EXECUTION_MODEL_SET_SPEC: ExternalCallModelSetSpec = ExternalCallModelSetSpec {
    id: "fixture.services-v1",
    models: INVALID_EXECUTION_MODELS,
};
const INVALID_EXECUTION_MODEL_SET: ExternalCallModelSetRef =
    ExternalCallModelSetRef::new(&INVALID_EXECUTION_MODEL_SET_SPEC);
const INVALID_EXECUTION_MODEL_SETS: &[ExternalCallModelSetRef] = &[INVALID_EXECUTION_MODEL_SET];
const INVALID_EXECUTION_CONTRACTS: HarnessContractSpec = HarnessContractSpec {
    external_call_model_sets: INVALID_EXECUTION_MODEL_SETS,
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
  "schema_version": 5,
  "command": "interfaces discover",
  "analysis_scope": {{
    "architecture":"riscv32",
    "calling_convention":"riscv-ilp32",
    "evidence":"control-flow-merged register provenance",
    "relocation_evidence":["absolute","pc-relative","got"],
    "semantic_claim":false,
    "table_layout_claim":false,
    "linker_resolution_claim":false,
    "completeness_claim":false
  }},
  "artifacts": [{{"index":0,"path":"libpp.a","roles":[],"sources":["libpp"],"sha256":"{digest}","container":"archive","functions":1,"reviewed_boundaries":0}}],
  "calls": [{{
    "artifact":0,
    "member":"event.o",
    "function":"post_event",
    "function_address":"0x100",
    "site":"0x120",
    "kind":"call",
    "link_register":1,
    "target":{{
      "canonical":"event.o::g_services[0][16]",
      "root":{{
        "kind":"relocated-symbol",
        "canonical":"event.o::g_services",
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
    "root_linkage":{{
      "mode":"association-only",
      "symbols":["g_services"],
      "resolutions":["defined"],
      "candidates":[]
    }},
    "arguments":[
      {{"index":0,"kind":"unknown"}},
      {{"index":1,"kind":"constant","value":"0x0000002a"}},
      {{
        "index":2,
        "kind":"pointer-provenance",
        "canonical":"arg0+4",
        "root":{{"kind":"function-argument","canonical":"arg0","argument":0}},
        "loads":[],
        "post_offset":4
      }}
    ]
  }}],
  "table_candidates": [{{
    "artifact": 0,
    "root": {{
      "kind":"relocated-symbol",
      "canonical":"event.o::g_services",
      "member":"event.o",
      "symbol":"g_services",
      "addend":0,
      "addressing":"absolute"
    }},
    "container_path":[{{"offset":0,"width":32}}],
    "slots":[{{"offset":16,"width":32,"functions":["post_event"],"call_sites":1}}],
    "functions":["post_event"],
    "call_sites":1
  }}],
  "decode_blockers":[],
  "analysis_failures":[]
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

#[test]
fn stored_interface_facts_reject_unknown_and_missing_fields() {
    let directory = fixture_directory();
    let facts = directory.join("facts.json");
    write_facts(&facts, &fake_digest('a'));
    let input = std::fs::read_to_string(&facts).unwrap();

    let mut unknown: serde_json::Value = serde_json::from_str(&input).unwrap();
    unknown["calls"][0]["target"]["legacy_field"] = serde_json::json!(true);
    let error = crate::artifacts::parse_interface_facts(&unknown.to_string()).unwrap_err();
    assert!(error.to_string().contains("unknown field `legacy_field`"));

    let mut missing: serde_json::Value = serde_json::from_str(&input).unwrap();
    missing["artifacts"][0]
        .as_object_mut()
        .unwrap()
        .remove("container");
    let error = crate::artifacts::parse_interface_facts(&missing.to_string()).unwrap_err();
    assert!(error.to_string().contains("missing field `container`"));
    std::fs::remove_dir_all(directory).unwrap();
}

fn reviewed_pack(digest: &str, semantic: &str) -> String {
    format!(
        r#"schema = 2
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
        pointer_cell_symbols: Vec::new(),
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
fn execution_model_output_requires_a_reviewed_output_pointer_argument() {
    let directory = fixture_directory();
    let facts = directory.join("facts.json");
    let pack = directory.join("pack.toml");
    let catalog = directory.join("semantics.toml");
    let digest = fake_digest('7');
    write_facts(&facts, &digest);
    write_catalog(&catalog);
    std::fs::write(&pack, executable_reviewed_pack(&digest)).unwrap();

    let error = InterfaceWorkspace::load(
        &facts,
        &pack,
        &[catalog],
        "riscv-ilp32",
        Some(&INVALID_EXECUTION_CONTRACTS),
    )
    .unwrap_err();
    std::fs::remove_dir_all(directory).unwrap();

    assert!(
        error
            .to_string()
            .contains("argument a1 has non-output ABI type \"const-ptr\"")
    );
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
            r#"{"offset":16,"width":32,"functions":["post_event"],"call_sites":1}"#,
            r#"{"offset":0,"width":32,"selector":{"argument":0,"scale":4,"addend":0,"canonical":"arg0*4+0x0"},"functions":["post_event"],"call_sites":1}"#,
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
fn sparse_pack_keeps_generated_backlog_outside_reviewed_toml() {
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
    assert_eq!(workspace.summary().unreviewed_anchors, 1);
    assert_eq!(workspace.summary().unreviewed_slots, 1);
    let observation = &workspace.unreviewed_observations()[0];
    assert_eq!(observation.offset, 16);
    assert_eq!(observation.width, 32);
    assert_eq!(observation.functions, ["post_event"]);
    assert_eq!(observation.call_sites, [0x120]);
    assert_eq!(observation.contract, "unmatched:libpp:relocated-symbol");
    assert!(overwrite.contains("refusing to overwrite"));
    std::fs::remove_dir_all(directory).unwrap();
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
        r#"schema = 2
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
