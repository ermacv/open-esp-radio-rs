use std::fs;

use super::*;
use crate::{MemoryMap, MmioMap, TargetSpec, cli::ProjectInitArgs, project::ProjectSpec};

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
    super::super::project_init::run(ProjectInitArgs {
        directory: directory.clone(),
        id: "status-fixture".to_owned(),
        mmio: vec!["radio=0x20000000..0x20010000".parse().unwrap()],
        source: Vec::new(),
        rust_target: None,
        pac_raw_crate_name: None,
        import_svd: None,
    })
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
    let svd = MmioMap::load_all(&[]).unwrap();
    let output = root.join("status.json");
    let function_workspace = std::sync::OnceLock::new();
    let code_workspace = std::sync::OnceLock::new();
    let register_workspace = std::sync::OnceLock::new();
    let interface_workspace = std::sync::OnceLock::new();
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
        function_workspace: &function_workspace,
        code_workspace: &code_workspace,
        register_workspace: &register_workspace,
        interface_workspace: &interface_workspace,
    };
    assert!(
        run(
            ProjectStatusArgs {
                output: Some(output.clone()),
                ..Default::default()
            },
            context(),
        )
        .unwrap()
    );
    assert!(
        run(
            ProjectStatusArgs {
                output: Some(output.clone()),
                check: true,
                ..Default::default()
            },
            context(),
        )
        .unwrap()
    );
    assert!(
        !run(
            ProjectStatusArgs {
                deny_incomplete: true,
                ..Default::default()
            },
            context(),
        )
        .unwrap()
    );
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
    assert!(!directory.join("generated/pac-raw/src/lib.rs").exists());

    fs::remove_dir_all(root).unwrap();
}
