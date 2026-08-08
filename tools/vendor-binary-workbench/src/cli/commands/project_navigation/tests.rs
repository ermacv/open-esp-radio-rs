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
            "schema_version": 2,
            "command": "symbols inventory",
            "artifacts": [{
                "index": 0,
                "artifact": {"path": "vendor.o", "sha256": digest},
                "sources": ["vendor"]
            }],
            "symbols": [
                {
                    "artifact": 0,
                    "member": null,
                    "name": "caller",
                    "address": "0x100",
                    "table": "static",
                    "definition": "section",
                    "kind": "text",
                    "resolution": "defined-exported"
                },
                {
                    "artifact": 0,
                    "member": null,
                    "name": "g_table",
                    "address": "0x200",
                    "table": "static",
                    "definition": "section",
                    "kind": "data",
                    "resolution": "defined-exported"
                }
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        &interfaces_path,
        serde_json::to_string(&json!({
            "schema_version": 2,
            "command": "interfaces discover",
            "artifacts": [{
                "index": 0,
                "path": "vendor.o",
                "sources": ["vendor"],
                "sha256": "11".repeat(32)
            }],
            "calls": [{
                "artifact": 0,
                "member": null,
                "function": "caller",
                "function_address": "0x100",
                "site": "0x110",
                "kind": "call",
                "target": {"root": {
                    "kind": "relocated-symbol",
                    "member": null,
                    "symbol": "g_table"
                }}
            }]
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
