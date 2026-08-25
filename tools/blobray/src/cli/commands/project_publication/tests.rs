use std::{fs, path::PathBuf};

use super::*;
use crate::project::{
    PacBindingsOutputSpec, PacRawOutputSpec, RegisterWorkspacePaths, ReviewWorkspaceSpec,
};

#[test]
fn publishes_and_checks_a_complete_register_project() {
    let (directory, project) = fixture_project("complete", true, true, true);

    assert!(run(CheckArgs::default(), &project, None).unwrap());
    let paths = project.registers.as_ref().unwrap();
    assert!(paths.svd_output.as_ref().unwrap().is_file());
    assert!(paths.pac_raw.as_ref().unwrap().output.is_file());
    assert!(paths.api_output.as_ref().unwrap().is_file());
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
    assert!(paths.pac_raw.is_none());
    assert!(paths.api_output.is_none());
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
        .pac_raw
        .as_mut()
        .unwrap()
        .target = "invalid".to_owned();

    assert!(!run(CheckArgs::default(), &project, None).unwrap());
    let paths = project.registers.as_ref().unwrap();
    assert!(!paths.svd_output.as_ref().unwrap().exists());
    assert!(!paths.pac_raw.as_ref().unwrap().output.exists());
    assert!(!paths.api_output.as_ref().unwrap().exists());

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn rejects_shared_output_paths_before_publication() {
    let (directory, mut project) = fixture_project("shared-output", true, true, true);
    let paths = project.registers.as_mut().unwrap();
    paths.pac_raw.as_mut().unwrap().output = paths.svd_output.clone().unwrap();

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
        "blobray-project-publication-{name}-{}",
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
    if pac {
        fs::write(
            directory.join("registers/api.toml"),
            "schema = 3\n\n[options]\n",
        )
        .unwrap();
    }

    let paths = RegisterWorkspacePaths {
        facts: directory.join("generated/mmio.json"),
        model: directory.join("registers/device.toml"),
        owned_ranges: vec!["radio".to_owned()],
        non_operational_functions: Vec::new(),
        review_output: None,
        review_ir_reports: Vec::new(),
        svd_output: svd.then(|| directory.join("generated/device.svd")),
        pac_raw: pac.then(|| PacRawOutputSpec {
            output: directory.join("generated/pac-raw/src/lib.rs"),
            target: "none".to_owned(),
            edition: "2024".to_owned(),
        }),
        bindings: bindings.then(|| PacBindingsOutputSpec {
            output: directory.join("generated/device.bindings.toml"),
            crate_name: "fixture_pac".to_owned(),
        }),
        api_pack: pac.then(|| directory.join("registers/api.toml")),
        api_output: pac.then(|| directory.join("generated/pac/src/generated.rs")),
        lint_pack: None,
        evidence_catalogs: Vec::new(),
        reviewed_knowledge: Vec::new(),
        review_context: open_radio_vendor_review::ApplicabilityContext::default(),
    };
    let project_id = format!("publication-{name}");
    let review_output = directory.join("generated/review-scopes.json");
    fs::create_dir_all(review_output.parent().unwrap()).unwrap();
    fs::write(
        &review_output,
        serde_json::to_string_pretty(&serde_json::json!({
            "schema_version": crate::review_scopes::REVIEW_SCOPES_SCHEMA,
            "command": "project review scopes",
            "project": project_id,
            "scopes": [],
        }))
        .unwrap(),
    )
    .unwrap();
    let project = ProjectSpec {
        id: project_id,
        target_spec: directory.join("target.toml"),
        ecosystem_packs: Vec::new(),
        chip_pack: None,
        analysis_provider: None,
        run_spec: None,
        memory_map: None,
        svd_paths: Vec::new(),
        reviewed_knowledge: Vec::new(),
        reviewed_knowledge_default: None,
        review_context: open_radio_vendor_review::ApplicabilityContext::default(),
        symbol_inventory: None,
        navigation_index: None,
        code: None,
        ir_profiles: Vec::new(),
        registers: Some(paths),
        interfaces: None,
        functions: None,
        review: Some(ReviewWorkspaceSpec {
            output: review_output,
            publication_scopes: Vec::new(),
            scopes: Vec::new(),
        }),
        verification: None,
    };
    (directory, project)
}
