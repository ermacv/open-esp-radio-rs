use std::fs;

use super::*;
use crate::{MemoryMap, MmioRegisterMap, TargetSpec, project::ProjectSpec};

#[test]
fn status_options_keep_ci_gate_and_output_check_independent() {
    assert_eq!(
        parse_options(vec![
            "--json-report".to_owned(),
            "status.json".to_owned(),
            "--check".to_owned(),
            "--deny-incomplete".to_owned(),
        ])
        .unwrap(),
        Options {
            json_report: Some(PathBuf::from("status.json")),
            check: true,
            deny_incomplete: true,
        }
    );
    assert!(
        parse_options(vec!["--check".to_owned()])
            .unwrap_err()
            .to_string()
            .contains("requires --json-report")
    );
}

#[test]
fn initialized_project_reports_incomplete_without_mutating_owned_outputs() {
    let root = std::env::temp_dir().join(format!(
        "vendor-workbench-project-status-{}",
        std::process::id()
    ));
    if root.exists() {
        fs::remove_dir_all(&root).unwrap();
    }
    let directory = root.join("project");
    super::super::project_init::run(vec![
        "--directory".to_owned(),
        directory.display().to_string(),
        "--id".to_owned(),
        "status-fixture".to_owned(),
        "--mmio".to_owned(),
        "radio=0x20000000..0x20010000".to_owned(),
    ])
    .unwrap();
    let manifest = directory.join("vendor-project.toml");
    let project = ProjectSpec::load(&manifest).unwrap();
    let mut target = TargetSpec::load(&project.target_spec).unwrap();
    project
        .platform_pack
        .as_ref()
        .unwrap()
        .apply_to_target(&mut target)
        .unwrap();
    let memory = MemoryMap::load(project.memory_map.as_deref().unwrap()).unwrap();
    let svd_paths = Vec::new();
    let svd = MmioRegisterMap::load_all(&[]).unwrap();
    let output = root.join("status.json");
    let context = || ProjectContext {
        project_path: &manifest,
        project: &project,
        target_path: &project.target_spec,
        target: &target,
        run_spec_path: None,
        run_spec: None,
        memory_map: Some(&memory),
        svd_paths: &svd_paths,
        svd: &svd,
    };
    assert!(
        run(
            vec!["--json-report".to_owned(), output.display().to_string(),],
            context(),
        )
        .unwrap()
    );
    assert!(
        run(
            vec![
                "--json-report".to_owned(),
                output.display().to_string(),
                "--check".to_owned(),
            ],
            context(),
        )
        .unwrap()
    );
    assert!(!run(vec!["--deny-incomplete".to_owned()], context()).unwrap());
    let document: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&output).unwrap()).unwrap();
    assert_eq!(document["overall"], "incomplete");
    assert_eq!(document["phases"]["configuration"]["status"], "ready");
    assert_eq!(
        document["phases"]["verification"]["status"],
        "not-configured"
    );
    assert_eq!(document["phases"]["publication"]["status"], "incomplete");
    assert!(!directory.join("generated/svd/device.svd").exists());
    assert!(!directory.join("generated/pac/src/lib.rs").exists());

    fs::remove_dir_all(root).unwrap();
}
