use std::{fs, path::PathBuf};

use super::*;
use crate::project::{PacBindingsOutputSpec, PacOutputSpec};

#[test]
fn publishes_and_checks_a_complete_register_project() {
    let (directory, project) = fixture_project("complete", true, true, true);

    assert!(run(CheckArgs::default(), &project, None).unwrap());
    let paths = project.registers.as_ref().unwrap();
    assert!(paths.svd_output.as_ref().unwrap().is_file());
    assert!(paths.pac.as_ref().unwrap().output.is_file());
    assert!(paths.bindings.as_ref().unwrap().output.is_file());
    assert!(run(CheckArgs { check: true }, &project, None).unwrap());

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn skips_outputs_absent_from_a_partial_project() {
    let (directory, project) = fixture_project("svd-only", true, false, false);

    assert!(run(CheckArgs::default(), &project, None).unwrap());
    let paths = project.registers.as_ref().unwrap();
    assert!(paths.svd_output.as_ref().unwrap().is_file());
    assert!(paths.pac.is_none());
    assert!(paths.bindings.is_none());
    assert!(run(CheckArgs { check: true }, &project, None).unwrap());

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn generation_failure_does_not_write_an_earlier_prepared_output() {
    let (directory, mut project) = fixture_project("preflight", true, true, false);
    project
        .registers
        .as_mut()
        .unwrap()
        .pac
        .as_mut()
        .unwrap()
        .target = "invalid".to_owned();

    assert!(!run(CheckArgs::default(), &project, None).unwrap());
    let paths = project.registers.as_ref().unwrap();
    assert!(!paths.svd_output.as_ref().unwrap().exists());
    assert!(!paths.pac.as_ref().unwrap().output.exists());

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn rejects_shared_output_paths_before_publication() {
    let (directory, mut project) = fixture_project("shared-output", true, true, true);
    let paths = project.registers.as_mut().unwrap();
    paths.pac.as_mut().unwrap().output = paths.svd_output.clone().unwrap();

    let error = run(CheckArgs::default(), &project, None).unwrap_err();
    assert!(error.to_string().contains("share"));
    assert!(
        !project
            .registers
            .as_ref()
            .unwrap()
            .svd_output
            .as_ref()
            .unwrap()
            .exists()
    );

    fs::remove_dir_all(directory).unwrap();
}

fn fixture_project(name: &str, svd: bool, pac: bool, bindings: bool) -> (PathBuf, ProjectSpec) {
    let directory = std::env::temp_dir().join(format!(
        "vendor-workbench-project-publication-{name}-{}",
        std::process::id()
    ));
    if directory.exists() {
        fs::remove_dir_all(&directory).unwrap();
    }
    fs::create_dir_all(directory.join("registers/peripherals")).unwrap();
    fs::write(
        directory.join("registers/device.toml"),
        r#"schema = 2
address-space = "cpu"
fragments = ["peripherals/radio.toml"]

[device]
name = "TEST"
version = "0.1"
description = "Publication fixture"
address-unit-bits = 8
width = 32
"#,
    )
    .unwrap();
    fs::write(
        directory.join("registers/peripherals/radio.toml"),
        r#"schema = 2

[[peripherals]]
name = "RADIO"
baseAddress = 0x1000

[[peripherals.registers]]
[peripherals.registers.register]
name = "CONTROL"
addressOffset = 0
size = 32
access = "read-write"

[[peripherals.registers.register.fields]]
name = "ENABLE"
bitOffset = 0
bitWidth = 1
"#,
    )
    .unwrap();

    let paths = RegisterWorkspacePaths {
        facts: directory.join("generated/mmio.json"),
        model: directory.join("registers/device.toml"),
        review_output: None,
        review_ir_reports: Vec::new(),
        svd_output: svd.then(|| directory.join("generated/device.svd")),
        pac: pac.then(|| PacOutputSpec {
            output: directory.join("generated/pac/src/lib.rs"),
            target: "none".to_owned(),
            edition: "2024".to_owned(),
        }),
        bindings: bindings.then(|| PacBindingsOutputSpec {
            output: directory.join("generated/device.bindings"),
            crate_name: "fixture_pac".to_owned(),
        }),
        api_pack: None,
        lint_pack: None,
        evidence_catalogs: Vec::new(),
    };
    let project = ProjectSpec {
        id: format!("publication-{name}"),
        target_spec: directory.join("target.spec"),
        platform_pack: None,
        run_spec: None,
        memory_map: None,
        svd_configured: false,
        svd_paths: Vec::new(),
        symbol_inventory: None,
        ir_profiles: Vec::new(),
        registers: Some(paths),
        interfaces: None,
        functions: None,
    };
    (directory, project)
}
