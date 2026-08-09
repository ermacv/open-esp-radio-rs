use std::{fs, path::PathBuf};

use serde_json::json;

use super::{build::build, inspect::inspect_report};
use crate::{
    project::{InterfaceWorkspacePaths, ProjectSpec},
    project_analysis::{NavigationIndexSpec, SymbolInventorySpec},
};

#[test]
fn interface_caller_and_relocated_root_join_inventory_locations() {
    let directory = std::env::temp_dir().join(format!(
        "vendor-workbench-navigation-join-{}",
        std::process::id()
    ));
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
            "schema_version": 3,
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
                "skipped_members": 0
            }],
            "code_sections": [],
            "symbols": [
                {
                    "artifact": 0,
                    "member": null,
                    "object_kind": "relocatable",
                    "name": "caller",
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
                    "candidates": []
                },
                {
                    "artifact": 0,
                    "member": null,
                    "object_kind": "relocatable",
                    "name": "g_table",
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
                    "candidates": []
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
                "code_recovery_blockers": 0
            }
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        &interfaces_path,
        serde_json::to_string(&json!({
            "schema_version": 3,
            "command": "interfaces discover",
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
                "functions": 1
            }],
            "calls": [{
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
                        "canonical": "g_table",
                        "member": null,
                        "symbol": "g_table",
                        "addend": 0,
                        "addressing": "absolute"
                    },
                    "loads": [],
                    "container_depth": 0,
                    "slot_offset": null,
                    "jalr_offset": 0
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
            "decode_failures": []
        }))
        .unwrap(),
    )
    .unwrap();

    let project = ProjectSpec {
        id: "fixture".to_owned(),
        target_spec: PathBuf::from("target.spec"),
        platform_pack: None,
        run_spec: None,
        memory_map: None,
        svd_configured: false,
        svd_paths: Vec::new(),
        symbol_inventory: Some(SymbolInventorySpec {
            output: symbols_path,
        }),
        navigation_index: Some(NavigationIndexSpec {
            output: directory.join("navigation.json"),
        }),
        ir_profiles: Vec::new(),
        registers: None,
        interfaces: Some(InterfaceWorkspacePaths {
            facts: interfaces_path,
            pack: None,
            semantic_catalogs: Vec::new(),
        }),
        functions: None,
        verification: None,
    };
    let document = build(&project).unwrap();
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
    let navigation_path = directory.join("navigation.json");
    fs::write(
        &navigation_path,
        serde_json::to_string_pretty(&document).unwrap(),
    )
    .unwrap();
    assert_eq!(inspect_report(&navigation_path).unwrap().symbols, 2);

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
