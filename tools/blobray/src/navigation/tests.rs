use std::{fs, path::PathBuf};

use serde_json::json;

use super::{build::build, inspect::inspect_report};
use crate::{
    project::{InterfaceWorkspacePaths, ProjectSpec},
    project_analysis::{NavigationIndexSpec, SymbolInventorySpec},
};

#[test]
fn interface_caller_and_relocated_root_join_inventory_locations() {
    let directory =
        std::env::temp_dir().join(format!("blobray-navigation-join-{}", std::process::id()));
    if directory.exists() {
        fs::remove_dir_all(&directory).unwrap();
    }
    fs::create_dir_all(&directory).unwrap();
    let digest = "11".repeat(32);
    let symbols_path = directory.join("symbols.json");
    let interfaces_path = directory.join("interfaces.json");
    fs::write(
        &symbols_path,
        serde_json::to_string(&json!({
            "schema_version": 7,
            "command": "symbols inventory",
            "linkage_mode": "association-only",
            "linker_resolution_claim": false,
            "artifacts": [{
                "index": 0,
                "artifact": {"path": "vendor.o", "sha256": digest},
                "roles": ["vendor"],
                "sources": ["vendor"],
                "container": "object",
                "objects": 1,
                "skipped_members": 0,
                "members": []
            }],
            "code_sections": [],
            "symbols": [
                {
                    "artifact": 0,
                    "member": null,
                    "object_kind": "relocatable",
                    "name": "caller",
                    "location": {"object": {"kind": "standalone"}, "table": "static", "index": 1},
                    "section_index": 1,
                    "address": "0x100",
                    "table": "static",
                    "binding": "global",
                    "visibility": "default",
                    "definition": "section",
                    "kind": "text",
                    "section": ".text",
                    "size": 4,
                    "scope": "linkage",
                    "resolution": "defined-exported",
                    "candidates": [],
                    "origin_association": "not-applicable",
                    "origin_candidates": []
                },
                {
                    "artifact": 0,
                    "member": null,
                    "object_kind": "relocatable",
                    "name": "g_table",
                    "location": {"object": {"kind": "standalone"}, "table": "static", "index": 2},
                    "section_index": 2,
                    "address": "0x200",
                    "table": "static",
                    "binding": "global",
                    "visibility": "default",
                    "definition": "section",
                    "kind": "data",
                    "section": ".data",
                    "size": 4,
                    "scope": "linkage",
                    "resolution": "defined-exported",
                    "candidates": [],
                    "origin_association": "not-applicable",
                    "origin_candidates": []
                }
            ],
            "summary": {
                "artifacts": 1,
                "symbol_facts": 2,
                "emitted": 2,
                "exported_definitions": 2,
                "undefined": 0,
                "unresolved_or_associated": 0,
                "executable_sections": 0,
                "executable_bytes": 0,
                "symbol_covered_bytes": 0,
                "uncovered_executable_bytes": 0,
                "named_zero_sized_code_symbols": 0,
                "function_boundary_candidates": 0,
                "code_recovery_blockers": 0,
                "link_unit_definitions": 0,
                "unique_archive_origins": 0,
                "ambiguous_archive_origins": 0,
                "missing_archive_origins": 0
            }
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        &interfaces_path,
        serde_json::to_string(&json!({
            "schema_version": 11,
            "command": "interfaces discover",
            "limits": {"max_state_updates":4096,"max_value_alternatives":64},
            "gaps": [],
            "analysis_scope": {
                "architecture": "riscv32",
                "calling_convention": "riscv-ilp32",
                "evidence": "control-flow-merged register provenance",
                "relocation_evidence": ["absolute", "pc-relative", "got"],
                "semantic_claim": false,
                "table_layout_claim": false,
                "linker_resolution_claim": false,
                "completeness_claim": false
            },
            "artifacts": [{
                "index": 0,
                "path": "vendor.o",
                "roles": ["vendor"],
                "sources": ["vendor"],
                "sha256": "11".repeat(32),
                "container": "object",
                "functions": 1,
                "reviewed_boundaries": 0
            }],
            "assignments": [],
            "calls": [{
                "owner":{"kind":"symbol","artifact_sha256":digest,"location":{"object":{"kind":"standalone"},"table":"static","index":1}},
                "artifact": 0,
                "member": null,
                "function": "caller",
                "function_address": "0x100",
                "site": "0x110",
                "kind": "call",
                "link_register": 1,
                "target": {
                    "canonical": "g_table",
                    "root": {
                        "kind": "relocated-symbol",
                        "reference":{"kind":"captured","artifact_sha256":digest,"location":{"object":{"kind":"standalone"},"table":"static","index":2},"binding":"local-definition"},
                        "canonical": "g_table",
                        "member": null,
                        "symbol": "g_table",
                        "addend": 0,
                        "addressing": "absolute"
                    },
                    "loads": [],
                    "container_depth": 0,
                    "slot_offset": null,
                    "post_offset": 0, "jalr_offset": 0
                },
                "root_linkage": {
                    "mode": "association-only",
                    "symbols": ["g_table"],
                    "resolutions": ["defined-exported"],
                    "candidates": []
                },
                "arguments": []
            }],
            "table_candidates": [],
            "decode_blockers": [],
            "analysis_failures": []
        }))
        .unwrap(),
    )
    .unwrap();

    let mut evidence: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&interfaces_path).unwrap()).unwrap();
    let gap = crate::interface_discovery::InterfaceAnalysisGap {
        owner: crate::artifact::ArtifactSymbolDefinition::synthetic_identity(
            module_path!(),
            &None,
            "unmatched_owner",
            0x200,
        ),
        member: None,
        function: "unmatched_owner".into(),
        site: 0x210,
        reason: crate::interface_discovery::InterfaceGapReason::UnresolvedCallTarget,
        registers: (0..32)
            .map(
                |register| crate::interface_discovery::InterfaceRegisterValue {
                    register,
                    value: crate::interface_discovery::InterfaceArgumentValue::Unknown,
                },
            )
            .collect(),
    };
    evidence["gaps"] = json!([{"artifact":0,"evidence":gap}]);
    evidence["calls"][0]["arguments"] = json!([{"index":0,"value":{"kind":"alternatives","value":[{"kind":"unknown"},{"kind":"constant","value":42}]}}]);
    fs::write(&interfaces_path, evidence.to_string()).unwrap();
    let expected_observations = crate::interfaces::InterfaceFacts::load(&interfaces_path).unwrap();
    let project = ProjectSpec {
        manifest: std::path::PathBuf::from("project.toml"),
        loaded_model_inputs: Default::default(),
        id: "fixture".to_owned(),
        target_spec: PathBuf::from("target.toml"),
        ecosystem_packs: Vec::new(),
        chip_pack: None,
        analysis_provider: None,
        run_spec: None,
        memory_map: None,
        svd_paths: Vec::new(),
        reviewed_knowledge: Vec::new(),
        reviewed_knowledge_default: None,
        review_context: open_radio_vendor_contracts::ApplicabilityContext::default(),
        symbol_inventory: Some(SymbolInventorySpec {
            output: symbols_path,
        }),
        navigation_index: Some(NavigationIndexSpec {
            output: directory.join("navigation.json"),
        }),
        code: None,
        ir_profiles: Vec::new(),
        analysis_symbol_families: Vec::new(),
        registers: None,
        interfaces: Some(InterfaceWorkspacePaths {
            facts: interfaces_path,
            pack: None,
            capability_context: None,
            semantic_catalogs: Vec::new(),
            capability_packs: Vec::new(),
            interface_template_packs: Vec::new(),
        }),
        functions: None,
        review: None,
        verification: None,
    };
    let document = build(&project).unwrap();
    assert_eq!(document.schema_version, 7);
    assert_eq!(
        document.interface_observations.as_ref(),
        Some(&expected_observations)
    );
    assert_eq!(
        document.interface_observations.as_ref().unwrap().gaps[0].evidence,
        gap
    );
    let caller = document
        .symbols
        .iter()
        .find(|symbol| symbol.name == "caller")
        .unwrap();
    let root = document
        .symbols
        .iter()
        .find(|symbol| symbol.name == "g_table")
        .unwrap();
    assert_eq!(caller.interface_calls.len(), 1);
    assert_eq!(root.interface_roots.len(), 1);
    assert_eq!(document.summary.interface_callers, 1);
    assert_eq!(document.summary.interface_roots, 1);
    assert_eq!(document.summary.unmatched_interface_roots, 0);
    assert!(
        document
            .inputs
            .iter()
            .all(|input| !PathBuf::from(&input.path).is_absolute())
    );
    let navigation_path = directory.join("navigation.json");
    fs::write(
        &navigation_path,
        serde_json::to_string_pretty(&document).unwrap(),
    )
    .unwrap();
    assert_eq!(inspect_report(&navigation_path).unwrap().symbols, 2);

    let mut altered = serde_json::to_value(&document).unwrap();
    altered["interface_observations"]["calls"][0]["arguments"][0]["value"] =
        json!({"kind":"unknown"});
    fs::write(&navigation_path, altered.to_string()).unwrap();
    assert!(
        inspect_report(&navigation_path)
            .unwrap_err()
            .to_string()
            .contains("observations disagree")
    );

    let mut stale_schema = serde_json::to_value(&document).unwrap();
    stale_schema["legacy_field"] = json!(true);
    fs::write(
        &navigation_path,
        serde_json::to_string_pretty(&stale_schema).unwrap(),
    )
    .unwrap();
    assert!(
        inspect_report(&navigation_path)
            .unwrap_err()
            .to_string()
            .contains("unknown field")
    );
    fs::remove_dir_all(directory).unwrap();
}
