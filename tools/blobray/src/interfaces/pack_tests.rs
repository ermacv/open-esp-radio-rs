//! Interface workspace validation tests.

use std::collections::BTreeSet;
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use super::*;
use crate::{
    ExternalCallModelSetRef, ExternalCallModelSetSpec, ExternalCallModelSpec, ExternalOutputModel,
    ExternalReturnModel, KnowledgeContractSpec,
    analysis::{
        DiscoveredInterfaceAssignment, LinkageArtifact, ProjectInterfaceDiscovery,
        ProjectLinkageInventory,
    },
    artifact::ArtifactContainerKind,
    interface_discovery::{InterfaceRoot, InterfaceSlotAssignment},
};

const EXECUTION_MODELS: &[ExternalCallModelSpec] = &[
    ExternalCallModelSpec {
        id: "queue-send-from-isr",
        return_model: ExternalReturnModel::Constant(1),
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "allocate-u32",
        return_model: ExternalReturnModel::Allocated { size_argument: 0 },
        outputs: &[],
    },
];
const EXECUTION_MODEL_SET_SPEC: ExternalCallModelSetSpec = ExternalCallModelSetSpec {
    id: "fixture.services-v1",
    models: EXECUTION_MODELS,
};
const EXECUTION_MODEL_SET: ExternalCallModelSetRef =
    ExternalCallModelSetRef::new(&EXECUTION_MODEL_SET_SPEC);
const EXECUTION_MODEL_SETS: &[ExternalCallModelSetRef] = &[EXECUTION_MODEL_SET];
const EXECUTION_CONTRACTS: KnowledgeContractSpec = KnowledgeContractSpec {
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
const INVALID_EXECUTION_CONTRACTS: KnowledgeContractSpec = KnowledgeContractSpec {
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
        "blobray-interface-pack-{}-{}",
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
  "schema_version": 11,
  "command": "interfaces discover",
  "limits": {{"max_state_updates":4096,"max_value_alternatives":64}},
  "gaps": [],
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
    "owner":{{"kind":"symbol","artifact_sha256":"{digest}","location":{{"object":{{"kind":"archive-member","ordinal":0}},"table":"static","index":1}}}},
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
        "reference":{{"kind":"captured","artifact_sha256":"{digest}","location":{{"object":{{"kind":"archive-member","ordinal":0}},"table":"static","index":3}},"binding":"local-definition"}},
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
      "post_offset":0,
      "jalr_offset":0
    }},
    "root_linkage":{{
      "mode":"association-only",
      "symbols":["g_services"],
      "resolutions":["defined"],
      "candidates":[]
    }},
    "arguments":[
      {{"index":0,"value":{{"kind":"unknown"}}}},
      {{"index":1,"value":{{"kind":"constant","value":42}}}},
      {{"index":2,"value":{{"kind":"pointer","value":{{
        "root":{{"kind":"function-argument","owner":{{"kind":"symbol","artifact_sha256":"{digest}","location":{{"object":{{"kind":"archive-member","ordinal":0}},"table":"static","index":1}}}},"index":0}},
        "loads":[],"post_offset":4
      }}}}}}
    ]
  }}],
  "assignments": [{{
    "owner":{{"kind":"symbol","artifact_sha256":"{digest}","location":{{"object":{{"kind":"archive-member","ordinal":0}},"table":"static","index":2}}}},
    "artifact":0,
    "member":"event.o",
    "function":"init_services",
    "function_address":"0x80",
    "site":"0x98",
    "root":{{
      "kind":"relocated-symbol",
        "reference":{{"kind":"captured","artifact_sha256":"{digest}","location":{{"object":{{"kind":"archive-member","ordinal":0}},"table":"static","index":3}},"binding":"local-definition"}},
      "canonical":"event.o::g_services",
      "member":"event.o",
      "symbol":"g_services",
      "addend":0,
      "addressing":"absolute"
    }},
    "target_loads":[],
    "target_offset":0,
    "container_path":[{{"site":"0x90","offset":0,"width":32}}],
    "offset":16,
    "width":32,
    "target":{{
      "kind":"relocated-symbol",
        "reference":{{"kind":"captured","artifact_sha256":"{digest}","location":{{"object":{{"kind":"archive-member","ordinal":0}},"table":"static","index":3}},"binding":"local-definition"}},
      "canonical":"init.o::queue_send_from_isr",
      "member":"init.o",
      "symbol":"queue_send_from_isr",
      "addend":0,
      "addressing":"absolute"
    }}
  }}],
  "table_candidates": [{{
    "artifact": 0,
    "root": {{
      "kind":"relocated-symbol",
        "reference":{{"kind":"captured","artifact_sha256":"{digest}","location":{{"object":{{"kind":"archive-member","ordinal":0}},"table":"static","index":3}},"binding":"local-definition"}},
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

fn bounded_data_root(symbol: &str, address: u32, size: u32, width: Option<u8>) -> InterfaceRoot {
    use open_radio_vendor_contracts::{DataAddressCandidate, DataAddressResolution, DataIdentity};
    InterfaceRoot::AbsoluteAddress {
        address,
        data_address: DataAddressResolution::from_candidates(
            address,
            width,
            vec![DataAddressCandidate {
                identity: DataIdentity::Synthetic {
                    namespace: module_path!().to_owned(),
                    key: symbol.to_owned(),
                },
                member: None,
                symbol: symbol.to_owned(),
                symbol_address: address,
                symbol_size: size,
                exported: true,
                offset: 0,
            }],
        ),
    }
}

fn bounded_assignment_document(directory: &Path) -> serde_json::Value {
    let artifact_path = directory.join("bounded-assignment.elf");
    std::fs::write(&artifact_path, b"bounded assignment fixture").unwrap();
    let discovery = ProjectInterfaceDiscovery {
        limits: Default::default(),
        gaps: Vec::new(),
        linkage: ProjectLinkageInventory {
            artifacts: vec![LinkageArtifact {
                sha256: crate::artifact_sha256(&artifact_path).unwrap(),
                path: artifact_path,
                roles: Vec::new(),
                sources: vec!["fixture".to_owned()],
                container: ArtifactContainerKind::Elf32,
                objects: 1,
                skipped_members: 0,
                members: Vec::new(),
                code_sections: Vec::new(),
            }],
            symbols: Vec::new(),
        },
        functions: vec![1],
        reviewed_boundaries: vec![0],
        calls: Vec::new(),
        assignments: vec![DiscoveredInterfaceAssignment {
            artifact: 0,
            assignment: InterfaceSlotAssignment {
                owner: crate::artifact::CodeIdentity::Synthetic {
                    namespace: module_path!().into(),
                    key: format!("fixture:{}", line!()),
                },
                target_loads: Vec::new(),
                target_offset: 0,
                member: None,
                function: "install_static_table".to_owned(),
                function_address: 0x1000,
                site: 0x1010,
                root: bounded_data_root("table_cell", 0x2000, 4, Some(32)),
                container_loads: Vec::new(),
                offset: 0,
                width: 32,
                target: bounded_data_root("static_table", 0x3000, 0x20, None),
            },
        }],
        decode_blockers: Vec::new(),
        failures: Vec::new(),
    };
    serde_json::to_value(crate::artifacts::build_interface_facts(&discovery).unwrap()).unwrap()
}

fn load_bounded_assignment_document(
    directory: &Path,
    document: &serde_json::Value,
) -> crate::Result<InterfaceFacts> {
    let facts_path = directory.join("bounded-facts.json");
    std::fs::write(&facts_path, document.to_string()).unwrap();
    InterfaceFacts::load(&facts_path)
}

fn bounded_assignment_error(mutate: impl FnOnce(&mut serde_json::Value)) -> String {
    let directory = fixture_directory();
    let mut document = bounded_assignment_document(&directory);
    mutate(&mut document);
    let error = load_bounded_assignment_document(&directory, &document).unwrap_err();
    std::fs::remove_dir_all(&directory).unwrap();
    error.to_string()
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
fn interface_roots_require_captured_reference_or_explicit_unknown_and_scoped_arguments() {
    let directory = fixture_directory();
    let path = directory.join("facts.json");
    write_facts(&path, &fake_digest('a'));
    let original: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let mut missing = original.clone();
    missing["calls"][0]["target"]["root"]
        .as_object_mut()
        .unwrap()
        .remove("reference");
    std::fs::write(&path, missing.to_string()).unwrap();
    assert!(InterfaceFacts::load(&path).is_err());
    let mut foreign = original.clone();
    foreign["calls"][0]["target"]["root"]["reference"]["artifact_sha256"] =
        serde_json::json!(fake_digest('b'));
    std::fs::write(&path, foreign.to_string()).unwrap();
    assert!(
        InterfaceFacts::load(&path)
            .unwrap_err()
            .to_string()
            .contains("reference belongs to another artifact")
    );
    let mut foreign_argument = original.clone();
    foreign_argument["calls"][0]["arguments"][2]["value"]["value"]["root"]["owner"]["location"]["index"] =
        serde_json::json!(99);
    std::fs::write(&path, foreign_argument.to_string()).unwrap();
    assert!(
        InterfaceFacts::load(&path)
            .unwrap_err()
            .to_string()
            .contains("root belongs to another function")
    );
    let mut unknown = original;
    let reference =
        serde_json::json!({"kind":"unknown","reason":"source supplied no physical relocation"});
    unknown["calls"][0]["target"]["root"]["reference"] = reference.clone();
    unknown["table_candidates"][0]["root"]["reference"] = reference.clone();
    std::fs::write(&path, unknown.to_string()).unwrap();
    let facts = InterfaceFacts::load(&path).unwrap();
    let InterfaceFactRoot::RelocatedSymbol {
        reference: retained,
        ..
    } = &facts.calls[0].root
    else {
        panic!("relocated root")
    };
    assert_eq!(serde_json::to_value(retained).unwrap(), reference);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn physical_owners_keep_identical_display_sites_distinct_and_reject_foreign_artifacts() {
    let directory = fixture_directory();
    let path = directory.join("facts.json");
    write_facts(&path, &fake_digest('a'));
    let mut document: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    for group in ["calls", "assignments"] {
        let mut other = document[group][0].clone();
        other["owner"]["location"]["object"]["ordinal"] = serde_json::json!(1);
        if group == "calls" {
            other["arguments"][2]["value"]["value"]["root"]["owner"] = other["owner"].clone();
        }
        document[group].as_array_mut().unwrap().push(other);
    }
    document["decode_blockers"] = serde_json::json!([{
        "owner":document["calls"][0]["owner"], "artifact":0, "member":"event.o", "function":"post_event",
        "address":"0x124", "width":32, "raw":"0", "class":"unsupported", "linear_control_flow":false
    }]);
    document["analysis_failures"] = serde_json::json!([{
        "owner":document["calls"][1]["owner"], "artifact":0, "member":"event.o", "function":"post_event", "error":"incomplete body"
    }]);
    std::fs::write(&path, document.to_string()).unwrap();
    let facts = InterfaceFacts::load(&path).unwrap();
    assert_eq!(facts.calls.len(), 2);
    assert_eq!(facts.assignments.len(), 2);
    assert_ne!(facts.calls[0].owner, facts.calls[1].owner);
    assert_eq!(facts.calls[0].site, facts.calls[1].site);
    assert_eq!(facts.calls[0].function, facts.calls[1].function);
    for group in [
        "calls",
        "assignments",
        "decode_blockers",
        "analysis_failures",
    ] {
        let mut foreign = document.clone();
        foreign[group][0]["owner"]["artifact_sha256"] = serde_json::json!(fake_digest('b'));
        std::fs::write(&path, foreign.to_string()).unwrap();
        assert!(
            InterfaceFacts::load(&path)
                .unwrap_err()
                .to_string()
                .contains("owner belongs to another artifact")
        );
        let mut missing = document.clone();
        missing[group][0].as_object_mut().unwrap().remove("owner");
        std::fs::write(&path, missing.to_string()).unwrap();
        assert!(InterfaceFacts::load(&path).is_err());
    }
    document["schema_version"] = serde_json::json!(9);
    std::fs::write(&path, document.to_string()).unwrap();
    assert!(InterfaceFacts::load(&path).is_err());
    std::fs::remove_dir_all(directory).unwrap();
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

#[test]
fn bounded_data_assignment_round_trips_through_schema_v11() {
    let directory = fixture_directory();
    let document = bounded_assignment_document(&directory);

    assert_eq!(document["schema_version"], serde_json::json!(11));
    crate::artifacts::parse_interface_facts(&document.to_string()).unwrap();
    let facts = load_bounded_assignment_document(&directory, &document).unwrap();
    std::fs::remove_dir_all(directory).unwrap();

    assert_eq!(facts.assignments.len(), 1);
    let assignment = &facts.assignments[0];
    assert_eq!(assignment.offset, 0);
    assert_eq!(assignment.width, 32);
    for (root, name, expected) in [
        (&assignment.root, "table_cell", 0x2000),
        (&assignment.target, "static_table", 0x3000),
    ] {
        let InterfaceFactRoot::AbsoluteAddress {
            address,
            data_address,
        } = root
        else {
            panic!("numeric root");
        };
        assert_eq!(*address, expected);
        assert_eq!(data_address.candidates().len(), 1);
        assert_eq!(data_address.candidates()[0].symbol, name);
    }
}

#[test]
fn bounded_data_assignment_rejects_out_of_bounds_address() {
    let error = bounded_assignment_error(|document| {
        document["assignments"][0]["target"]["address"] = serde_json::json!("0x00003020");
    });
    assert!(
        error.contains(
            "interface assignment target: data-address evidence does not match the observed access"
        ),
        "{error}"
    );
}

#[test]
fn bounded_data_assignment_rejects_overflowing_symbol_range() {
    let error = bounded_assignment_error(|document| {
        let root = &mut document["assignments"][0]["root"];
        root["address"] = serde_json::json!("0xfffffffc");
        root["data_address"]["address"] = serde_json::json!(0xfffffffc_u32);
        root["data_address"]["candidate"]["symbol_address"] = serde_json::json!(0xfffffffc_u32);
        root["data_address"]["candidate"]["symbol_size"] = serde_json::json!(8);
    });
    assert!(
        error.contains("interface assignment root: data-address access overflows"),
        "{error}"
    );
}

#[test]
fn bounded_data_assignment_rejects_nonzero_root_offset() {
    let error = bounded_assignment_error(|document| {
        document["assignments"][0]["offset"] = serde_json::json!(4);
    });
    assert!(
        error.contains(
            "interface assignment root: data-address evidence does not match the observed access"
        ),
        "{error}"
    );
}

#[test]
fn bounded_data_pointer_cell_retains_an_indirect_object_field_offset() {
    let directory = fixture_directory();
    let mut document = bounded_assignment_document(&directory);
    document["assignments"][0]["container_path"] = serde_json::json!([{
        "site": "0x00001008",
        "offset": 0,
        "width": 32
    }]);
    document["assignments"][0]["offset"] = serde_json::json!(12);

    let facts = load_bounded_assignment_document(&directory, &document).unwrap();
    std::fs::remove_dir_all(directory).unwrap();
    assert_eq!(facts.assignments[0].container_path.len(), 1);
    assert_eq!(facts.assignments[0].offset, 12);
}

#[test]
fn bounded_data_assignment_rejects_store_crossing_symbol_end() {
    let error = bounded_assignment_error(|document| {
        document["assignments"][0]["root"]["data_address"]["candidate"]["symbol_size"] =
            serde_json::json!(2);
    });
    assert!(
        error.contains(
            "interface assignment root: data-address access lies outside its data-symbol range"
        ),
        "{error}"
    );
}

#[test]
fn interface_assignment_requires_explicit_address_evidence() {
    let error = bounded_assignment_error(|document| {
        document["assignments"][0]["target"] = serde_json::json!({
            "kind": "absolute-address",
            "canonical": "0x00003000",
            "address": "0x00003000"
        });
    });
    assert!(error.contains("missing field `data_address`"), "{error}");
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

fn reusable_template(name: &str) -> String {
    let revision = ["0123456789abcdef", "0123456789abcdef", "01234567"].concat();
    format!(
        r#"schema = 1
id = "fixture.templates"

[[templates]]
id = "vendor.services-v9"
provenance = {{ repository = "https://example.com/vendor/sdk", revision = "{revision}", path = "include/services.h" }}
layout-version = "vendor-v9"
pointer-width = 32
layout-size = 32
slot-stride = 4

[[templates.slots]]
offset = 16
width = 32
name = "{name}"
arguments = ["opaque-handle", "const-ptr", "out-ptr"]
return = "bool"
semantic = "rtos.queue.send-from-isr"
"#
    )
}

fn templated_reviewed_pack(digest: &str, override_offset: i32) -> String {
    format!(
        r#"schema = 3
id = "fixture"
calling-convention = "riscv-ilp32"

[[anchors]]
id = "wifi-osi"
template = "vendor.services-v9"
status = "reviewed"
origin = "observed"
source = "libpp"
root-kind = "relocated-symbol"
member = "event.o"
symbol = "g_services"
addend = 0
addressing = "absolute"
container-path = [{{ offset = 0, width = 32 }}]
execution-contract = "fixture.services-v1"

[[anchors.guards]]
kind = "artifact-sha256"
sha256 = "{digest}"

[[anchors.overrides]]
offset = {override_offset}
reason = "Bind the exact project provider model."
origin = "observed"
execution-model = "queue-send-from-isr"
"#
    )
}

#[test]
fn reusable_template_materializes_public_layout_under_exact_project_binding() {
    let directory = fixture_directory();
    let facts = directory.join("facts.json");
    let pack = directory.join("pack.toml");
    let catalog = directory.join("semantics.toml");
    let template = directory.join("templates.toml");
    let digest = fake_digest('e');
    write_facts(&facts, &digest);
    write_catalog(&catalog);
    std::fs::write(&template, reusable_template("queue_send_from_isr")).unwrap();
    std::fs::write(&pack, templated_reviewed_pack(&digest, 16)).unwrap();

    let workspace = InterfaceWorkspace::load_with_templates(
        &facts,
        &pack,
        std::slice::from_ref(&catalog),
        std::slice::from_ref(&template),
        "riscv-ilp32",
        Some(&EXECUTION_CONTRACTS),
    )
    .unwrap();
    std::fs::remove_dir_all(&directory).unwrap();

    assert_eq!(workspace.summary().interface_templates, 1);
    assert_eq!(workspace.summary().templated_anchors, 1);
    assert_eq!(workspace.bindings()[0].name, "queue_send_from_isr");
    assert_eq!(workspace.bindings()[0].layout_version, "vendor-v9");
    assert_eq!(
        workspace.bindings()[0]
            .execution_model
            .as_ref()
            .map(|model| model.model.as_str()),
        Some("queue-send-from-isr")
    );

    std::fs::create_dir_all(&directory).unwrap();
    write_facts(&facts, &digest);
    write_catalog(&catalog);
    std::fs::write(&template, reusable_template("queue_send_from_isr")).unwrap();
    std::fs::write(
        &pack,
        templated_reviewed_pack(&digest, 16).replace("origin = \"observed\"\n", ""),
    )
    .unwrap();
    let error = InterfaceWorkspace::load_with_templates(
        &facts,
        &pack,
        std::slice::from_ref(&catalog),
        std::slice::from_ref(&template),
        "riscv-ilp32",
        Some(&EXECUTION_CONTRACTS),
    )
    .unwrap_err();
    std::fs::remove_dir_all(directory).unwrap();
    assert!(error.to_string().contains("manual slot"));
    assert!(error.to_string().contains("now observed"));
}

#[test]
fn reusable_template_composition_rejects_conflicts_and_unknown_overrides() {
    let directory = fixture_directory();
    let facts = directory.join("facts.json");
    let pack = directory.join("pack.toml");
    let catalog = directory.join("semantics.toml");
    let template_a = directory.join("templates-a.toml");
    let template_b = directory.join("templates-b.toml");
    let digest = fake_digest('e');
    write_facts(&facts, &digest);
    write_catalog(&catalog);
    std::fs::write(&template_a, reusable_template("queue_send_from_isr")).unwrap();
    std::fs::write(
        &template_b,
        reusable_template("renamed_slot").replace(
            "id = \"vendor.services-v9\"",
            "id = \"vendor.other-services-v9\"",
        ),
    )
    .unwrap();
    std::fs::write(&pack, templated_reviewed_pack(&digest, 16)).unwrap();

    let duplicate_pack = InterfaceWorkspace::load_with_templates(
        &facts,
        &pack,
        std::slice::from_ref(&catalog),
        &[&template_a, &template_b],
        "riscv-ilp32",
        Some(&EXECUTION_CONTRACTS),
    )
    .unwrap_err();
    assert!(
        duplicate_pack
            .to_string()
            .contains("duplicate interface template pack id")
    );

    std::fs::write(
        &template_b,
        reusable_template("renamed_slot").replace(
            "id = \"fixture.templates\"",
            "id = \"fixture.more-templates\"",
        ),
    )
    .unwrap();
    let conflict = InterfaceWorkspace::load_with_templates(
        &facts,
        &pack,
        std::slice::from_ref(&catalog),
        &[&template_a, &template_b],
        "riscv-ilp32",
        Some(&EXECUTION_CONTRACTS),
    )
    .unwrap_err();
    assert!(
        conflict
            .to_string()
            .contains("conflicts with an earlier pack")
    );

    std::fs::write(&pack, templated_reviewed_pack(&digest, 20)).unwrap();
    let unknown = InterfaceWorkspace::load_with_templates(
        &facts,
        &pack,
        std::slice::from_ref(&catalog),
        std::slice::from_ref(&template_a),
        "riscv-ilp32",
        Some(&EXECUTION_CONTRACTS),
    )
    .unwrap_err();
    std::fs::remove_dir_all(directory).unwrap();
    assert!(unknown.to_string().contains("unknown template slot offset"));
}

#[test]
fn reusable_template_requires_an_artifact_digest_binding() {
    let directory = fixture_directory();
    let facts = directory.join("facts.json");
    let pack = directory.join("pack.toml");
    let catalog = directory.join("semantics.toml");
    let template = directory.join("templates.toml");
    let digest = fake_digest('e');
    write_facts(&facts, &digest);
    write_catalog(&catalog);
    std::fs::write(&template, reusable_template("queue_send_from_isr")).unwrap();
    let input = templated_reviewed_pack(&digest, 16).replace(
        &format!(
            "[[anchors.guards]]\nkind = \"artifact-sha256\"\nsha256 = \"{digest}\"\n"
        ),
        "[[anchors.guards]]\nkind = \"runtime-value\"\npurpose = \"version\"\noffset = 0\nwidth = 32\nmask = 0xffffffff\nvalue = 9\n",
    );
    std::fs::write(&pack, input).unwrap();
    let error = InterfaceWorkspace::load_with_templates(
        &facts,
        &pack,
        std::slice::from_ref(&catalog),
        std::slice::from_ref(&template),
        "riscv-ilp32",
        Some(&EXECUTION_CONTRACTS),
    )
    .unwrap_err();
    std::fs::remove_dir_all(directory).unwrap();
    assert!(error.to_string().contains("pinned by exactly one artifact"));
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
            "offset = 0\nwidth = 32\nstatus = \"reviewed\"\norigin = \"reviewed\"\nname = \"slot_zero\"",
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
origin = "reviewed"
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
            .contains("requires a configured compiled knowledge provider")
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
fn allocation_model_accepts_the_reviewed_u32_size_abi() {
    let directory = fixture_directory();
    let facts = directory.join("facts.json");
    let pack = directory.join("pack.toml");
    let catalog = directory.join("semantics.toml");
    let digest = fake_digest('8');
    write_facts(&facts, &digest);
    write_catalog(&catalog);
    let input = reviewed_pack(&digest, "rtos.queue.send-from-isr")
        .replace(
            "slot-stride = 4\n",
            "slot-stride = 4\nexecution-contract = \"fixture.services-v1\"\n",
        )
        .replace(
            "arguments = [\"opaque-handle\", \"const-ptr\", \"out-ptr\"]\nreturn = \"bool\"\nsemantic = \"rtos.queue.send-from-isr\"\n",
            "arguments = [\"u32\", \"u32\"]\nreturn = \"mut-ptr\"\nexecution-model = \"allocate-u32\"\n",
        );
    std::fs::write(&pack, input).unwrap();

    let workspace = InterfaceWorkspace::load(
        &facts,
        &pack,
        &[catalog],
        "riscv-ilp32",
        Some(&EXECUTION_CONTRACTS),
    )
    .unwrap();
    std::fs::remove_dir_all(directory).unwrap();

    assert_eq!(
        workspace.bindings()[0]
            .execution_model
            .as_ref()
            .map(|model| model.return_model),
        Some(ExternalReturnModel::Allocated { size_argument: 0 })
    );
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
    assert_eq!(workspace.bindings()[0].assignments.len(), 1);
    assert_eq!(
        workspace.bindings()[0].assignments[0].target_symbol,
        "queue_send_from_isr"
    );
    assert_eq!(workspace.bindings()[0].calls[0].observation.site, 0x120);
    assert_eq!(
        workspace.bindings()[0].calls[0].observation.arguments[1]
            .value
            .canonical(),
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
    alternate["arguments"][1]["value"]["value"] = serde_json::json!(43);
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
        crate::error::BlobrayError::ManifestSource { span, .. } => span.offset(),
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
fn linked_view_reuses_the_reviewed_source_inventory_anchor() {
    for ambiguous in [false, true] {
        let directory = fixture_directory();
        let facts_path = directory.join("facts.json");
        let pack_path = directory.join("pack.toml");
        let catalog_path = directory.join("semantics.toml");
        let digest = fake_digest('a');
        write_facts(&facts_path, &digest);
        let mut facts: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&facts_path).unwrap()).unwrap();
        facts["artifacts"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "index": 1,
                "path": "linked.elf",
                "roles": [],
                "sources": ["libpp"],
                "sha256": fake_digest('b'),
                "container": "elf32",
                "functions": 1,
                "reviewed_boundaries": 0
            }));
        facts["table_candidates"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "artifact": 1,
            "root": {
                "kind": "absolute-address",
                "canonical": "synthetic linked-view address",
                "address": "0x00002000",
                "data_address": match bounded_data_root("g_services", 0x2000, 4, Some(32)) {
                    InterfaceRoot::AbsoluteAddress { data_address, .. } => data_address,
                    _ => unreachable!(),
                }
            },
            "container_path": [{"offset": 0, "width": 32}],
            "slots": [
                {"offset": 16, "width": 32, "functions": ["linked_post_event"], "call_sites": 1},
                {"offset": 20, "width": 32, "functions": ["linked_post_event"], "call_sites": 1}
            ],
            "functions": ["linked_post_event"],
            "call_sites": 2
        }));
        if ambiguous {
            let resolution = &mut facts["table_candidates"][1]["root"]["data_address"];
            let first = resolution["candidate"].clone();
            let mut other = first.clone();
            other["identity"]["key"] = serde_json::json!("other-physical-owner");
            other["symbol"] = serde_json::json!("other_table");
            resolution.as_object_mut().unwrap().remove("candidate");
            resolution["status"] = serde_json::json!("ambiguous");
            resolution["candidates"] = serde_json::json!([first, other]);
        }
        std::fs::write(&facts_path, facts.to_string()).unwrap();
        write_catalog(&catalog_path);
        std::fs::write(
            &pack_path,
            reviewed_pack(&digest, "rtos.queue.send-from-isr"),
        )
        .unwrap();

        let workspace = InterfaceWorkspace::load(
            &facts_path,
            &pack_path,
            std::slice::from_ref(&catalog_path),
            "riscv-ilp32",
            None,
        )
        .unwrap();
        std::fs::remove_dir_all(directory).unwrap();

        if ambiguous {
            assert_eq!(workspace.summary().reviewed_slots, 1);
            assert_eq!(workspace.summary().unreviewed_slots, 2);
            assert_eq!(workspace.unreviewed_observations().len(), 2);
            assert_eq!(
                workspace
                    .unreviewed_observations()
                    .iter()
                    .map(|observation| observation.offset)
                    .collect::<BTreeSet<_>>(),
                BTreeSet::from([16, 20])
            );
        } else {
            assert_eq!(workspace.summary().reviewed_slots, 2);
            assert_eq!(workspace.summary().unreviewed_slots, 1);
            assert_eq!(workspace.unreviewed_observations().len(), 1);
            let observation = &workspace.unreviewed_observations()[0];
            assert_eq!(observation.contract, "fixture::wifi-osi");
            assert_eq!(observation.offset, 20);
            assert_eq!(observation.functions, ["linked_post_event"]);
        }
    }
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
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("catalogs/neutral-embedded.toml");
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

#[test]
fn stored_interface_evidence_keeps_alternatives_sites_offsets_and_gaps() {
    use crate::interface_discovery::{
        InterfaceAnalysisGap, InterfaceArgumentValue as V, InterfaceGapReason, InterfaceLoad,
        InterfacePointer, InterfaceRegisterValue, InterfaceRoot,
    };
    let directory = fixture_directory();
    let path = directory.join("facts.json");
    write_facts(&path, &fake_digest('a'));
    let mut document: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let argument_owner: crate::artifact::CodeIdentity =
        serde_json::from_value(document["calls"][0]["owner"].clone()).unwrap();
    let pointer = InterfacePointer {
        root: InterfaceRoot::FunctionArgument {
            owner: argument_owner.clone(),
            index: 0,
        },
        loads: vec![InterfaceLoad {
            site: 0x108,
            offset: 4,
            width: 32,
            selector: None,
        }],
        post_offset: 8,
    };
    let argument = V::Alternatives(vec![V::Unknown, V::Pointer(pointer.clone())]);
    document["calls"][0]["arguments"][0]["value"] = serde_json::to_value(&argument).unwrap();
    document["calls"][0]["target"]["post_offset"] = serde_json::json!(4);
    let mut other = document["calls"][0].clone();
    other["target"]["loads"][0]["site"] = serde_json::json!("0x10c");
    document["calls"].as_array_mut().unwrap().push(other);
    document["assignments"][0]["target_loads"] =
        serde_json::json!([{"site":"0x88","offset":4,"width":32}]);
    document["assignments"][0]["target_offset"] = serde_json::json!(8);
    let gap = InterfaceAnalysisGap {
        owner: argument_owner.clone(),
        member: Some("event.o".into()),
        function: "post_event".into(),
        site: 0x120,
        reason: InterfaceGapReason::UnresolvedCallTarget,
        registers: (0..32)
            .map(|register| InterfaceRegisterValue {
                register,
                value: if register == 10 {
                    argument.clone()
                } else {
                    V::Unknown
                },
            })
            .collect(),
    };
    document["gaps"] = serde_json::json!([{"artifact":0,"evidence":gap}]);
    std::fs::write(&path, document.to_string()).unwrap();
    let facts = InterfaceFacts::load(&path).unwrap();
    assert_eq!(facts.calls.len(), 2);
    assert_eq!(facts.calls[0].loads[0].site, Some(0x110));
    assert_eq!(facts.calls[1].loads[0].site, Some(0x10c));
    assert_eq!(facts.calls[0].arguments[0].value, argument);
    assert_eq!(facts.calls[0].target_offset, 4);
    assert_eq!(facts.assignments[0].target_loads[0].site, Some(0x88));
    assert_eq!(facts.assignments[0].target_offset, 8);
    assert_eq!(facts.gaps[0].evidence, gap);
    document["calls"][0]["target"]["loads"][0]
        .as_object_mut()
        .unwrap()
        .remove("site");
    std::fs::write(&path, document.to_string()).unwrap();
    assert!(
        InterfaceFacts::load(&path)
            .unwrap_err()
            .to_string()
            .contains("site")
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn shifted_call_and_loaded_assignment_remain_evidence_without_direct_review_binding() {
    let directory = fixture_directory();
    let facts_path = directory.join("facts.json");
    let pack_path = directory.join("pack.toml");
    let catalog = directory.join("semantics.toml");
    let digest = fake_digest('a');
    write_facts(&facts_path, &digest);
    write_catalog(&catalog);
    std::fs::write(
        &pack_path,
        reviewed_pack(&digest, "rtos.queue.send-from-isr"),
    )
    .unwrap();
    let mut document: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&facts_path).unwrap()).unwrap();
    document["calls"][0]["target"]["post_offset"] = serde_json::json!(4);
    document["assignments"][0]["target_loads"] =
        serde_json::json!([{"site":"0x88","offset":0,"width":32}]);
    std::fs::write(&facts_path, document.to_string()).unwrap();
    let facts = InterfaceFacts::load(&facts_path).unwrap();
    assert_eq!(facts.calls.len(), 1);
    assert_eq!(facts.assignments.len(), 1);
    let workspace =
        InterfaceWorkspace::load(&facts_path, &pack_path, &[catalog], "riscv-ilp32", None).unwrap();
    assert_eq!(workspace.bindings().len(), 1);
    assert!(workspace.bindings()[0].calls.is_empty());
    assert!(workspace.bindings()[0].assignments.is_empty());
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn reviewed_calls_keep_distinct_load_sites_and_typed_unknown_alternatives() {
    use crate::interface_discovery::{
        InterfaceArgumentValue as V, InterfaceLoad, InterfacePointer, InterfaceRoot,
    };
    let directory = fixture_directory();
    let facts_path = directory.join("facts.json");
    let pack_path = directory.join("pack.toml");
    let catalog = directory.join("semantics.toml");
    let digest = fake_digest('a');
    write_facts(&facts_path, &digest);
    write_catalog(&catalog);
    std::fs::write(
        &pack_path,
        reviewed_pack(&digest, "rtos.queue.send-from-isr"),
    )
    .unwrap();
    let mut document: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&facts_path).unwrap()).unwrap();
    let argument_owner: crate::artifact::CodeIdentity =
        serde_json::from_value(document["calls"][0]["owner"].clone()).unwrap();
    let value = |site| {
        V::Alternatives(vec![
            V::Unknown,
            V::Pointer(InterfacePointer {
                root: InterfaceRoot::FunctionArgument {
                    owner: argument_owner.clone(),
                    index: 0,
                },
                loads: vec![InterfaceLoad {
                    site,
                    offset: 4,
                    width: 32,
                    selector: None,
                }],
                post_offset: 8,
            }),
        ])
    };
    assert_eq!(value(0x104).canonical(), value(0x108).canonical());
    document["calls"][0]["arguments"][0]["value"] = serde_json::to_value(value(0x104)).unwrap();
    let mut alternate = document["calls"][0].clone();
    alternate["target"]["loads"][0]["site"] = serde_json::json!("0x10c");
    alternate["arguments"][0]["value"] = serde_json::to_value(value(0x108)).unwrap();
    document["calls"].as_array_mut().unwrap().push(alternate);
    document["decode_blockers"] = serde_json::json!([{"owner":document["calls"][0]["owner"],"artifact":0,"member":"event.o","function":"post_event","address":"0x10e","width":16,"raw":"0x0000","class":"zero-fill-or-illegal-trap","linear_control_flow":false}]);
    document["analysis_failures"] = serde_json::json!([{"owner":document["calls"][0]["owner"],"artifact":0,"member":"other.o","function":"unknown_body","error":"fixture decode failure"}]);
    std::fs::write(&facts_path, document.to_string()).unwrap();
    let workspace =
        InterfaceWorkspace::load(&facts_path, &pack_path, &[catalog], "riscv-ilp32", None).unwrap();
    let calls = &workspace.bindings()[0].calls;
    assert_eq!(calls.len(), 2);
    for call in calls {
        assert!(workspace.facts().calls.contains(&call.observation));
    }
    assert_eq!(workspace.facts().decode_blockers[0].address, 0x10e);
    assert_eq!(
        workspace.facts().analysis_failures[0].function,
        "unknown_body"
    );
    let encoded = serde_json::to_value(calls).unwrap();
    assert_ne!(
        encoded[0]["observation"]["arguments"],
        encoded[1]["observation"]["arguments"]
    );
    let query = serde_json::to_string(workspace.facts()).unwrap();
    let restored: InterfaceFacts = serde_json::from_str(&query).unwrap();
    restored.validate().unwrap();
    assert_eq!(&restored, workspace.facts());
    std::fs::remove_dir_all(directory).unwrap();
}
