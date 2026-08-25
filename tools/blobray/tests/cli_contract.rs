#![recursion_limit = "256"]

use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use sha2::{Digest, Sha256};

fn blobray() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_blobray-generic"));
    command.env_remove("RUST_LOG");
    command
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root")
}

const GENERIC_PROJECT: &str = "tools/blobray/tests/fixtures/generic-project/vendor-project.toml";

fn run(arguments: &[&str]) -> Output {
    blobray()
        .current_dir(repository_root())
        .args(arguments)
        .output()
        .expect("run blobray")
}

fn init_temporary_project(label: &str) -> (PathBuf, PathBuf) {
    let directory = std::env::temp_dir().join(format!(
        "blobray-cli-contract-{label}-{}",
        std::process::id()
    ));
    if directory.exists() {
        std::fs::remove_dir_all(&directory).unwrap();
    }
    let output = blobray()
        .current_dir(repository_root())
        .args(["project", "init", "--directory"])
        .arg(&directory)
        .args([
            "--id",
            label,
            "--mmio",
            "radio=0x20000000..0x20010000",
            "--format",
            "json",
            "--color",
            "never",
        ])
        .output()
        .expect("initialize temporary project");
    assert!(
        output.status.success(),
        "project init stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let manifest = directory.join("vendor-project.toml");
    (directory, manifest)
}

fn snapshot_tree(root: &Path) -> Vec<(PathBuf, Option<Vec<u8>>)> {
    fn collect(root: &Path, directory: &Path, snapshot: &mut Vec<(PathBuf, Option<Vec<u8>>)>) {
        let mut entries = std::fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        entries.sort();
        for path in entries {
            let relative = path.strip_prefix(root).unwrap().to_owned();
            if path.is_dir() {
                snapshot.push((relative, None));
                collect(root, &path, snapshot);
            } else {
                snapshot.push((relative, Some(std::fs::read(path).unwrap())));
            }
        }
    }

    let mut snapshot = Vec::new();
    collect(root, root, &mut snapshot);
    snapshot
}

fn write_rv32_symbol_fixture(path: &Path) {
    let bytes = include_str!("fixtures/symbols-rv32.hex")
        .split_ascii_whitespace()
        .map(|octet| u8::from_str_radix(octet, 16).unwrap())
        .collect::<Vec<_>>();
    std::fs::write(path, bytes).unwrap();
}

fn write_rv32_e2e_fixture(path: &Path) {
    let hex = include_str!("fixtures/generic-e2e-rv32.hex")
        .chars()
        .filter(|character| !character.is_ascii_whitespace())
        .collect::<String>();
    assert_eq!(hex.len() % 2, 0);
    let bytes = hex
        .as_bytes()
        .chunks_exact(2)
        .map(|octet| u8::from_str_radix(std::str::from_utf8(octet).unwrap(), 16).unwrap())
        .collect::<Vec<_>>();
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn bootstrap_reports_expose_typed_follow_up_steps() {
    let directory = std::env::temp_dir().join(format!(
        "blobray-cli-contract-typed-bootstrap-{}",
        std::process::id()
    ));
    if directory.exists() {
        std::fs::remove_dir_all(&directory).unwrap();
    }
    let initialized = blobray()
        .current_dir(repository_root())
        .args(["project", "init", "--directory"])
        .arg(&directory)
        .args([
            "--id",
            "typed-bootstrap",
            "--mmio",
            "radio=0x20000000..0x20010000",
            "--format",
            "json",
            "--color",
            "never",
        ])
        .output()
        .expect("initialize typed bootstrap project");
    assert!(
        initialized.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&initialized.stderr)
    );
    let initialized: serde_json::Value = serde_json::from_slice(&initialized.stdout).unwrap();
    assert_eq!(initialized["schema_version"], 2);
    assert!(initialized.get("next_command").is_none());
    assert_eq!(initialized["next_steps"].as_array().unwrap().len(), 2);
    let initialized_argv = initialized["next_steps"][0]["commands"][0]["argv"]
        .as_array()
        .unwrap();
    assert_eq!(
        &initialized_argv[..4],
        serde_json::json!(["blobray", "project", "inputs", "init"])
            .as_array()
            .unwrap()
    );

    let manifest = directory.join("vendor-project.toml");
    let artifact = directory.join("vendor.o");
    write_rv32_symbol_fixture(&artifact);
    let inputs = blobray()
        .current_dir(repository_root())
        .args(["project", "inputs", "init", "--project"])
        .arg(&manifest)
        .arg("--bind")
        .arg(format!("source-artifact:vendor={}", artifact.display()))
        .args(["--format", "json", "--color", "never"])
        .output()
        .expect("bind typed bootstrap inputs");
    assert!(
        inputs.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&inputs.stderr)
    );
    let inputs: serde_json::Value = serde_json::from_slice(&inputs.stdout).unwrap();
    assert_eq!(inputs["schema"], 2);
    assert!(inputs.get("next_command").is_none());
    let inputs_argv = inputs["next_steps"][0]["commands"][0]["argv"]
        .as_array()
        .unwrap();
    assert_eq!(
        &inputs_argv[..3],
        serde_json::json!(["blobray", "project", "doctor"])
            .as_array()
            .unwrap()
    );
    assert!(
        inputs["next_steps"][0]["commands"][0]["argv"]
            .as_array()
            .unwrap()
            .iter()
            .any(|argument| argument == "--run-spec")
    );

    let configured = blobray()
        .current_dir(repository_root())
        .args(["project", "configure", "--check", "--project"])
        .arg(&manifest)
        .args(["--format", "json", "--color", "never"])
        .output()
        .expect("check typed bootstrap configuration");
    assert!(
        configured.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&configured.stderr)
    );
    let configured: serde_json::Value = serde_json::from_slice(&configured.stdout).unwrap();
    assert_eq!(configured["schema_version"], 3);
    let configured_argv = configured["next_steps"][0]["commands"][0]["argv"]
        .as_array()
        .unwrap();
    assert_eq!(
        &configured_argv[..3],
        serde_json::json!(["blobray", "project", "doctor"])
            .as_array()
            .unwrap()
    );

    std::fs::remove_dir_all(directory).unwrap();
}

fn run_project_command(project: &Path, arguments: &[&str]) -> Output {
    blobray()
        .current_dir(repository_root())
        .args(arguments)
        .args(["--project"])
        .arg(project)
        .args(["--format", "json", "--color", "never"])
        .output()
        .expect("run project command")
}

fn write_rv32_archive_fixture(path: &Path) {
    let object = include_str!("fixtures/symbols-rv32.hex")
        .split_ascii_whitespace()
        .map(|octet| u8::from_str_radix(octet, 16).unwrap())
        .collect::<Vec<_>>();
    let mut archive = b"!<arch>\n".to_vec();
    writeln!(
        archive,
        "{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`",
        "vendor.o/",
        0,
        0,
        0,
        "100644",
        object.len()
    )
    .unwrap();
    archive.extend_from_slice(&object);
    if object.len() % 2 != 0 {
        archive.push(b'\n');
    }
    std::fs::write(path, archive).unwrap();
}

#[test]
fn project_status_json_is_pipe_safe_and_dependency_warnings_are_suppressed() {
    let output = run(&[
        "project",
        "status",
        "--project",
        GENERIC_PROJECT,
        "--format",
        "json",
        "--color",
        "never",
    ]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout must be one JSON document");
    assert_eq!(document["schema"], 12);
    assert_eq!(document["scope"], "blobray-pipeline");
    assert_eq!(document["command"], "project status");
    assert_eq!(document["validation"]["depth"], "shallow");
    assert_eq!(document["validation"]["freshness"], "unknown");
    assert_eq!(document["dimensions"]["freshness"]["status"], "unknown");
    assert_eq!(
        document["dimensions"]["freshness"]["validation_depth"],
        "shallow"
    );
    assert!(document["dimensions"]["research"]["status"].is_string());
    assert!(document["dimensions"]["verification"].is_string());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("Missing description for register"),
        "dependency warnings leaked to stderr: {stderr}"
    );
}

#[test]
fn project_status_human_details_explain_shallow_validation_and_render_fields() {
    let output = run(&[
        "project",
        "status",
        "--project",
        GENERIC_PROJECT,
        "--details",
        "--color",
        "never",
    ]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    for expected in [
        "Validation: shallow project-status inspection",
        "Freshness:    unknown — run project doctor or project check",
        "Research:",
        "Verification:",
        "Artifact readiness (not research completeness)",
        "Deep validation:",
        "Component details",
        "Field",
        "Value",
    ] {
        assert!(stdout.contains(expected), "missing {expected:?}: {stdout}");
    }
    assert!(
        !stdout.contains("CONFIGURED GATES READY"),
        "stdout: {stdout}"
    );
}

#[test]
fn status_and_doctor_next_steps_preserve_explicit_resolution_context() {
    let (directory, manifest) = init_temporary_project("follow-up-context");
    let target = directory.join("explicit target's.toml");
    std::fs::copy(directory.join("target.toml"), &target).unwrap();
    let run_spec = directory.join("explicit-run.toml");
    let artifact = directory.join("explicit-vendor.o");
    write_rv32_symbol_fixture(&artifact);
    std::fs::write(
        &run_spec,
        format!(
            "schema = 1\n\n[[inputs]]\nrole = \"source-artifact:fixture\"\npath = {:?}\n",
            artifact.display().to_string(),
        ),
    )
    .unwrap();
    let svd = directory.join("explicit-registers.svd");
    std::fs::write(
        &svd,
        r#"<?xml version="1.0" encoding="UTF-8"?>
<device schemaVersion="1.3" xmlns:xs="http://www.w3.org/2001/XMLSchema-instance">
  <name>FOLLOW_UP</name>
  <version>1</version>
  <description>follow-up context fixture</description>
  <addressUnitBits>8</addressUnitBits>
  <width>32</width>
  <peripherals/>
</device>
"#,
    )
    .unwrap();
    let quoted_target = format!(
        "'{}'",
        target.display().to_string().replace('\'', "'\"'\"'")
    );
    let expected = format!(
        "blobray project analyze --project {} --target-spec {} --run-spec {} --svd {}",
        manifest.display(),
        quoted_target,
        run_spec.display(),
        svd.display(),
    );
    let expected_argv = vec![
        "blobray",
        "project",
        "analyze",
        "--project",
        manifest.to_str().unwrap(),
        "--target-spec",
        target.to_str().unwrap(),
        "--run-spec",
        run_spec.to_str().unwrap(),
        "--svd",
        svd.to_str().unwrap(),
    ];

    let machine = blobray()
        .current_dir(repository_root())
        .args(["project", "status", "--project"])
        .arg(&manifest)
        .args(["--target-spec"])
        .arg(&target)
        .args(["--run-spec"])
        .arg(&run_spec)
        .args(["--svd"])
        .arg(&svd)
        .args(["--format", "json", "--color", "never"])
        .output()
        .expect("render project status with explicit context");
    assert!(
        machine.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&machine.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&machine.stdout).unwrap();
    let next_steps = document["phases"]
        .as_object()
        .unwrap()
        .values()
        .flat_map(|phase| phase["components"].as_array().unwrap())
        .filter_map(|component| component.get("next_step"))
        .flat_map(|step| step["commands"].as_array().unwrap())
        .collect::<Vec<_>>();
    assert!(
        next_steps
            .iter()
            .any(|action| action["argv"] == serde_json::json!(expected_argv)),
        "{next_steps:#?}"
    );

    for workflow in ["status", "doctor"] {
        let human = blobray()
            .current_dir(repository_root())
            .args(["project", workflow, "--project"])
            .arg(&manifest)
            .args(["--target-spec"])
            .arg(&target)
            .args(["--run-spec"])
            .arg(&run_spec)
            .args(["--svd"])
            .arg(&svd)
            .args(["--color", "never"])
            .output()
            .expect("render human follow-up context");
        let stdout = String::from_utf8_lossy(&human.stdout);
        assert!(stdout.contains(&expected), "{workflow} stdout: {stdout}");
    }

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn status_human_next_steps_preserve_shell_quoted_spaces_and_apostrophes() {
    let (directory, _) = init_temporary_project("quoted-follow-up-context");
    let quoted_directory = directory.with_file_name(format!(
        "{}  owner's project",
        directory.file_name().unwrap().to_string_lossy()
    ));
    std::fs::rename(&directory, &quoted_directory).unwrap();
    let manifest = quoted_directory.join("vendor-project.toml");
    let quoted_manifest = format!(
        "'{}'",
        manifest.display().to_string().replace('\'', "'\"'\"'")
    );
    let expected = format!("blobray project inputs init --project {quoted_manifest} --help");

    let output = blobray()
        .current_dir(repository_root())
        .args(["project", "status", "--project"])
        .arg(&manifest)
        .args(["--color", "never", "--progress", "never"])
        .output()
        .expect("render shell-safe status follow-up");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(&expected), "stdout: {stdout}");

    std::fs::remove_dir_all(quoted_directory).unwrap();
}

#[test]
fn status_input_repair_targets_the_explicit_run_spec() {
    let (directory, manifest) = init_temporary_project("explicit-run-repair");
    let project = std::fs::read_to_string(&manifest).unwrap().replace(
        "chip-pack = \"chip.toml\"\n",
        "chip-pack = \"chip.toml\"\nverification-addon = \"verification-addon.toml\"\n",
    );
    std::fs::write(&manifest, project).unwrap();
    std::fs::write(
        directory.join("verification-addon.toml"),
        r#"schema = 3
id = "explicit-run-repair"
report = "generated/verification.json"
evidence-index = "generated/vendor-evidence.json"

[[suites]]
id = "fixture"
rust-artifact-role = "rust-artifact:fixture"
rust-prefix = "fixture_"
profiles = []
dispositions = ["dispositions.toml"]
baselines = []
gate = "informational"

[[suites.vendor]]
source = "vendor"
prefix = "fixture_"
"#,
    )
    .unwrap();
    std::fs::write(
        directory.join("dispositions.toml"),
        "schema = 3\ndefault-disposition = \"not-yet-ported\"\ndefault-protocol = \"unknown\"\n",
    )
    .unwrap();
    let artifact = directory.join("vendor.o");
    write_rv32_symbol_fixture(&artifact);
    let run_spec = directory.join("explicit-run.toml");
    std::fs::write(
        &run_spec,
        format!(
            "schema = 1\n\n[[inputs]]\nrole = \"source-artifact:vendor\"\npath = {:?}\n",
            artifact.display().to_string(),
        ),
    )
    .unwrap();

    let output = blobray()
        .current_dir(repository_root())
        .args(["project", "status", "--project"])
        .arg(&manifest)
        .arg("--run-spec")
        .arg(&run_spec)
        .args([
            "--format",
            "json",
            "--color",
            "never",
            "--progress",
            "never",
        ])
        .output()
        .expect("render explicit run-spec repair action");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let component = document["phases"]["verification"]["components"]
        .as_array()
        .unwrap()
        .iter()
        .find(|component| component["name"] == "suite-inputs")
        .unwrap();
    assert_eq!(
        component["next_step"]["instruction"],
        format!("bind the missing roles in {}", run_spec.display())
    );
    assert_eq!(
        component["next_step"]["commands"][0]["argv"],
        serde_json::json!([
            "blobray",
            "project",
            "inputs",
            "init",
            "--project",
            manifest.to_str().unwrap(),
            "--output",
            run_spec.to_str().unwrap(),
            "--help"
        ])
    );

    let help = blobray()
        .current_dir(repository_root())
        .args(["project", "inputs", "init", "--project"])
        .arg(&manifest)
        .arg("--output")
        .arg(&run_spec)
        .arg("--help")
        .output()
        .expect("run the suggested binding help command");
    assert!(help.status.success());

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn project_doctor_json_is_one_complete_typed_report() {
    let output = run(&[
        "project",
        "doctor",
        "--project",
        GENERIC_PROJECT,
        "--format",
        "json",
        "--color",
        "never",
    ]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("doctor stdout must be one JSON document");
    let report = &document;
    assert_eq!(report["schema"], 4);
    assert_eq!(report["command"], "project doctor");
    assert_eq!(report["status"], "valid-with-warnings");
    assert_eq!(report["validation"]["depth"], "deep");
    assert_eq!(report["validation"]["freshness"], "unknown");
    assert!(report["duration_ms"].is_u64());
    assert_eq!(report["timings"].as_array().unwrap().len(), 9);
    assert_eq!(report["project"]["id"], "generic-rv32-fixture");
    assert_eq!(report["target"]["id"], "generic-rv32");
    assert!(report["capabilities"].as_array().unwrap().len() > 10);
    assert_eq!(report["ir_build"]["status"], "not-configured");
    assert_eq!(report["function_workspace"]["status"], "not-configured");
    assert_eq!(report["run_spec"]["status"], "not-configured");
    assert!(report["inputs"].is_array());
    assert!(report["errors"].is_u64());
    assert!(report["warnings"].is_u64());
    assert!(report["next_steps"].as_array().is_some_and(|steps| {
        steps.iter().all(|step| {
            step["instruction"].is_string()
                && step["commands"].as_array().is_some_and(|commands| {
                    commands.iter().all(|command| command["argv"].is_array())
                })
        })
    }));

    let revisions = report["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|capability| capability["name"] == "revision-workflow")
        .expect("revision-workflow capability");
    assert_eq!(revisions["status"], "baseline-missing");

    let memory = report["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|capability| capability["name"] == "memory-map")
        .expect("memory-map capability");
    assert!(memory["details"]["mmio-regions"].is_u64());

    let jsonl = run(&[
        "project",
        "doctor",
        "--project",
        GENERIC_PROJECT,
        "--format",
        "jsonl",
        "--color",
        "never",
    ]);
    assert!(!jsonl.status.success());
    assert!(jsonl.stdout.is_empty());
    assert!(String::from_utf8_lossy(&jsonl.stderr).contains("invalid value 'jsonl'"));
}

#[test]
fn obsolete_revision_state_is_a_hard_cutover_in_cli_status_and_doctor() {
    let help = run(&["project", "revision", "prepare-update", "--help"]);
    assert!(help.status.success());
    assert!(!String::from_utf8_lossy(&help.stdout).contains("migrate-legacy-scope"));

    let removed = run(&[
        "project",
        "revision",
        "prepare-update",
        "--migrate-legacy-scope",
        "map.toml",
    ]);
    assert!(!removed.status.success());
    assert!(String::from_utf8_lossy(&removed.stderr).contains("unexpected argument"));

    let (directory, manifest) = init_temporary_project("revision-cutover");
    let ledger = directory.join("revisions/ledger.toml");
    std::fs::create_dir_all(ledger.parent().unwrap()).unwrap();
    std::fs::write(&ledger, "schema = 1\nproject = \"revision-cutover\"\n").unwrap();

    let status = run_project_command(&manifest, &["project", "status"]);
    assert!(!status.status.success());
    let status: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    let revision = &status["phases"]["revision-workflow"]["components"][0];
    assert_eq!(revision["status"], "invalid");
    assert!(
        revision["diagnostic"]
            .as_str()
            .unwrap()
            .contains("create a new current state")
    );
    assert!(
        revision["next_step"]["commands"][0]["argv"]
            .as_array()
            .unwrap()
            .windows(4)
            .any(|arguments| arguments == ["project", "revision", "snapshot", "CURRENT"])
    );

    let doctor = run_project_command(&manifest, &["project", "doctor"]);
    let doctor: serde_json::Value = serde_json::from_slice(&doctor.stdout).unwrap();
    let revision = doctor["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|capability| capability["name"] == "revision-workflow")
        .unwrap();
    assert_eq!(revision["status"], "invalid");
    assert!(
        revision["details"]["error"]
            .as_str()
            .unwrap()
            .contains("create a new current state")
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn root_help_is_project_first_and_project_files_is_typed() {
    let help = run(&["--help"]);
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).unwrap();
    for command in ["project", "inspect", "registers", "advanced", "tooling"] {
        assert!(
            help.contains(command),
            "root help lacks {command:?}: {help}"
        );
    }
    assert!(help.contains("START HERE"));

    let files = run(&[
        "project",
        "files",
        "--project",
        GENERIC_PROJECT,
        "--format",
        "json",
        "--color",
        "never",
    ]);
    assert!(files.status.success());
    let report: serde_json::Value = serde_json::from_slice(&files.stdout).unwrap();
    assert_eq!(report["schema"], 4);
    assert_eq!(report["project_id"], "generic-rv32-fixture");
    assert_eq!(report["workflow_state"], "blocked");
    assert!(report["required_missing"].is_u64());
    assert!(report["next_steps"].as_array().is_some_and(|steps| {
        steps.iter().all(|step| {
            step["instruction"].is_string()
                && step["commands"].as_array().is_some_and(|commands| {
                    commands.iter().all(|command| command["argv"].is_array())
                })
        })
    }));
    assert!(report.get("next_actions").is_none());
    assert!(report["files"].as_array().unwrap().iter().any(|file| {
        file["role"] == "project-manifest"
            && file["ownership"] == "entrypoint"
            && file["layer"] == "composition"
    }));
}

#[test]
fn fresh_project_files_exposes_only_the_executable_bootstrap_frontier() {
    let (directory, manifest) = init_temporary_project("files-frontier");

    let machine = run_project_command(&manifest, &["project", "files"]);
    assert!(machine.status.success());
    let document: serde_json::Value = serde_json::from_slice(&machine.stdout).unwrap();
    assert_eq!(document["schema"], 4);
    assert_eq!(document["workflow_state"], "blocked");
    assert!(document["required_missing"].as_u64().unwrap() > 0);
    let steps = document["next_steps"].as_array().unwrap();
    assert_eq!(steps.len(), 1);
    assert!(steps[0]["instruction"].as_str().unwrap().contains("input"));
    let commands = steps[0]["commands"].as_array().unwrap();
    assert_eq!(commands.len(), 1);
    let argv = commands[0]["argv"].as_array().unwrap();
    assert_eq!(
        argv.iter()
            .take(4)
            .map(|argument| argument.as_str().unwrap())
            .collect::<Vec<_>>(),
        ["blobray", "project", "inputs", "init"]
    );
    assert_eq!(
        argv.last().and_then(|argument| argument.as_str()),
        Some("--help")
    );
    assert!(
        argv.iter()
            .all(|argument| argument.as_str() != Some("publish"))
    );
    assert!(document.get("next_actions").is_none());

    let human = blobray()
        .current_dir(repository_root())
        .args(["project", "files", "--project"])
        .arg(&manifest)
        .args(["--color", "never"])
        .output()
        .expect("render fresh project files");
    assert!(human.status.success());
    let stdout = String::from_utf8_lossy(&human.stdout);
    assert!(stdout.contains("BOOTSTRAP BLOCKED"), "stdout: {stdout}");
    assert!(stdout.contains("project inputs init"), "stdout: {stdout}");
    assert!(!stdout.contains("project publish"), "stdout: {stdout}");

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn generic_second_target_runs_analysis_review_and_pac_publication_end_to_end() {
    let root = std::env::temp_dir().join(format!("blobray-generic-e2e-{}", std::process::id()));
    if root.exists() {
        std::fs::remove_dir_all(&root).unwrap();
    }
    std::fs::create_dir_all(&root).unwrap();
    let directory = root.join("project");
    let initialized = blobray()
        .current_dir(repository_root())
        .args(["project", "init", "--directory"])
        .arg(&directory)
        .args([
            "--id",
            "generic-e2e",
            "--mmio",
            "device=0x40000000..0x40001000",
            "--format",
            "json",
            "--color",
            "never",
        ])
        .output()
        .expect("initialize generic e2e project");
    assert!(
        initialized.status.success(),
        "project init stderr: {}",
        String::from_utf8_lossy(&initialized.stderr)
    );

    let manifest = directory.join("vendor-project.toml");
    let artifact = directory.join("vendor.elf");
    write_rv32_e2e_fixture(&artifact);
    let binding = format!("source-artifact:vendor={}", artifact.display());
    let inputs = blobray()
        .current_dir(repository_root())
        .args(["project", "inputs", "init", "--project"])
        .arg(&manifest)
        .args(["--bind", &binding, "--format", "json", "--color", "never"])
        .output()
        .expect("bind generic artifact");
    assert!(
        inputs.status.success(),
        "project inputs stderr: {}",
        String::from_utf8_lossy(&inputs.stderr)
    );

    for arguments in [
        ["advanced", "symbols", "inventory"].as_slice(),
        ["advanced", "code", "init-pack"].as_slice(),
        ["advanced", "interfaces", "discover"].as_slice(),
        ["advanced", "interfaces", "init-pack"].as_slice(),
    ] {
        let output = run_project_command(&manifest, arguments);
        assert!(
            output.status.success(),
            "{} stderr: {}",
            arguments.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let interface_pack = directory.join("interfaces/reviewed.toml");
    let mut interfaces = std::fs::read_to_string(&interface_pack).unwrap();
    let artifact_sha = format!("{:x}", Sha256::digest(std::fs::read(&artifact).unwrap()));
    interfaces.push_str(&format!(
        r#"
[[anchors]]
id = "fixture-table"
status = "reviewed"
origin = "observed"
source = "vendor"
root-kind = "absolute-address"
address = 0x2000002c
container-path = []
layout-version = "fixture-v1"
pointer-width = 32
layout-size = 4
slot-stride = 4

[[anchors.guards]]
kind = "artifact-sha256"
sha256 = "{artifact_sha}"

[[anchors.slots]]
offset = 0
width = 32
status = "reviewed"
origin = "observed"
name = "fixture_callback"
arguments = []
return = "void"
"#,
    ));
    std::fs::write(interface_pack, interfaces).unwrap();

    let reviewed_path = directory.join("reviewed/project-facts.toml");
    let mut reviewed = std::fs::read_to_string(&reviewed_path).unwrap();
    reviewed.push_str(
        r#"

[[assertions]]
id = "device.control.identity"
subject = "register:generic-e2e-chip/cpu/0x40000000/32"
kind = "register-identity"
value = "DEVICE.CONTROL"
[[assertions.evidence]]
source = "fixture"
locator = "synthetic control register"
"#,
    );
    std::fs::write(reviewed_path, reviewed).unwrap();

    let mut project = std::fs::read_to_string(&manifest).unwrap();
    project.push_str(
        r#"
[review]
output = "generated/findings/review-scopes.json"
publication-scopes = ["fixture"]

[[review.scopes]]
id = "fixture"
protocols = ["shared"]
profiles = ["vendor"]
roots = ["vendor:fixture_entry"]
include-reachable = true
"#,
    );
    std::fs::write(&manifest, project).unwrap();

    for arguments in [
        ["advanced", "ir", "build"].as_slice(),
        ["advanced", "functions", "init-pack"].as_slice(),
        ["project", "analyze"].as_slice(),
        ["project", "publish"].as_slice(),
        ["project", "analyze", "--check"].as_slice(),
        ["project", "publish", "--check"].as_slice(),
    ] {
        let output = run_project_command(&manifest, arguments);
        assert!(
            output.status.success(),
            "{} stderr: {}\nstdout: {}",
            arguments.join(" "),
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
    }

    let inspected =
        run_project_command(&manifest, &["inspect", "function", "vendor:fixture_entry"]);
    assert!(inspected.status.success());
    let report: serde_json::Value = serde_json::from_slice(&inspected.stdout).unwrap();
    let pseudo = report["semantics"][0]["pseudo"].as_str().unwrap();
    assert!(pseudo.contains("mmio.read32(0x40000000)"));
    assert!(pseudo.contains("mmio.write32(0x40000000"));
    assert!(pseudo.contains("fence(fm=0x0, pred=0xf, succ=0xf)"));
    assert!(pseudo.contains("reviewed_abi.fixture_callback()"));
    assert_eq!(
        report["semantics"][0]["calls"][0]["kind"],
        "reviewed-external"
    );

    let unknown = run_project_command(
        &manifest,
        &["inspect", "function", "vendor:fixture_unknown"],
    );
    assert!(unknown.status.success());
    let unknown: serde_json::Value = serde_json::from_slice(&unknown.stdout).unwrap();
    assert_eq!(unknown["runtime"]["unsupported_instructions"], 1);

    for path in [
        "generated/svd/device.svd",
        "generated/svd/device.bindings.toml",
        "generated/pac-raw/src/lib.rs",
        "generated/pac/src/generated.rs",
    ] {
        assert!(directory.join(path).is_file(), "missing publication {path}");
    }
    let svd = std::fs::read_to_string(directory.join("generated/svd/device.svd")).unwrap();
    assert!(svd.contains("<name>CONTROL</name>"));
    let pac = std::fs::read_to_string(directory.join("generated/pac-raw/src/lib.rs")).unwrap();
    assert!(pac.contains("pub const fn control"));

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn project_leaf_help_explains_validity_readiness_and_next_steps() {
    for (command, expected) in [
        ("init", "Next:"),
        ("configure", "Next:"),
        ("doctor", "validity, not workflow readiness"),
        ("status", "project doctor"),
        ("analyze", "--check"),
        ("publish", "--check"),
    ] {
        let output = run(&["project", command, "--help"]);
        assert!(output.status.success(), "help failed for {command}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains(expected),
            "project {command} help lacks {expected:?}: {stdout}"
        );
    }
}

#[test]
fn focused_investigation_commands_are_part_of_the_typed_cli() {
    let function = run(&["inspect", "function", "--help"]);
    assert!(function.status.success());
    let function = String::from_utf8(function.stdout).unwrap();
    assert!(function.contains("<SOURCE:SYMBOL>"));
    assert!(function.contains("Authoritative linked image"));
    assert!(function.contains("Raw archives used as source inventory"));

    let scope = run(&["inspect", "scope", "--help"]);
    assert!(scope.status.success());
    let scope = String::from_utf8(scope.stdout).unwrap();
    assert!(scope.contains("<SCOPE>"));
    assert!(scope.contains("Exact project review-scope ID"));
}

#[test]
fn artifact_wide_jobs_default_to_four_and_zero_is_rejected() {
    let help = run(&["project", "analyze", "--help"]);
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).unwrap();
    assert!(help.contains("[default: 4]"), "unexpected help: {help}");

    let output = run(&[
        "project",
        "analyze",
        "--project",
        GENERIC_PROJECT,
        "--jobs",
        "0",
    ]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid value '0'"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn runtime_errors_do_not_emit_usage_or_an_empty_json_result() {
    let output = run(&[
        "project",
        "init",
        "--directory",
        "/tmp/blobray-invalid-cli-contract",
        "--id",
        "fixture",
        "--mmio",
        "first=0x20000000..0x20001000",
        "--mmio",
        "second=0x20000800..0x20002000",
        "--format",
        "json",
        "--color",
        "never",
    ]);
    assert!(!output.status.success());
    assert!(
        output.stdout.is_empty(),
        "failed command emitted stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("MMIO ranges \"first\" and \"second\" overlap"));
    assert!(!stderr.contains("Usage:"));
}

#[test]
fn project_manifest_diagnostics_highlight_the_nested_physical_value() {
    let directory = std::env::temp_dir().join(format!(
        "blobray-project-diagnostic-contract-{}",
        std::process::id()
    ));
    if directory.exists() {
        std::fs::remove_dir_all(&directory).unwrap();
    }
    std::fs::create_dir_all(&directory).unwrap();
    let manifest = directory.join("vendor-project.toml");
    std::fs::write(
        &manifest,
        "schema = 4\nid = \"fixture\"\ntarget-spec = \"target.toml\"\n[[analysis.ir]]\nid = \"known\"\nroots = \"all\"\noutput = \"known.json\"\n[functions]\npack = \"functions.toml\"\nprofiles = [\"missing\"]\n",
    )
    .unwrap();

    let output = blobray()
        .current_dir(repository_root())
        .args(["project", "status", "--project"])
        .arg(&manifest)
        .args(["--format", "json", "--color", "never"])
        .output()
        .expect("diagnose invalid project manifest");
    std::fs::remove_dir_all(directory).unwrap();

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("project functions refers to unknown IR profile \"missing\""));
    assert!(stderr.contains("profiles = [\"missing\"]"));
    assert!(stderr.contains("invalid project configuration"));
    assert!(stderr.contains("vendor-project.toml"));
    assert!(!stderr.contains("Usage:"));
}

#[test]
fn composed_manifest_diagnostics_highlight_the_physical_value() {
    let directory = std::env::temp_dir().join(format!(
        "blobray-composed-diagnostic-contract-{}",
        std::process::id()
    ));
    if directory.exists() {
        std::fs::remove_dir_all(&directory).unwrap();
    }
    std::fs::create_dir_all(&directory).unwrap();
    let project = directory.join("vendor-project.toml");
    std::fs::write(
        directory.join("target.toml"),
        "schema = 3\nid = \"fixture\"\narchitecture = \"riscv32\"\ncalling-convention = \"riscv-ilp32\"\nendianness = \"little\"\npointer-width = 32\nrust-target = \"riscv32imac-unknown-none-elf\"\n",
    )
    .unwrap();
    std::fs::write(
        directory.join("chip.toml"),
        "schema = 3\nid = \"fixture\"\nknowledge-provider = [\"bad\"]\n",
    )
    .unwrap();
    std::fs::write(
        &project,
        "schema = 4\nid = \"fixture\"\ntarget-spec = \"target.toml\"\nchip-pack = \"chip.toml\"\n",
    )
    .unwrap();

    let platform_output = blobray()
        .current_dir(repository_root())
        .args(["project", "status", "--project"])
        .arg(&project)
        .args(["--format", "json", "--color", "never"])
        .output()
        .expect("diagnose invalid chip pack");
    assert!(!platform_output.status.success());
    assert!(platform_output.stdout.is_empty());
    let platform_stderr = String::from_utf8_lossy(&platform_output.stderr);
    assert!(platform_stderr.contains("knowledge-provider = [\"bad\"]"));
    assert!(platform_stderr.contains("chip.toml"));
    assert!(!platform_stderr.contains("Usage:"));

    std::fs::write(
        directory.join("memory.toml"),
        "schema = 1\ndefault-address-space = \"cpu\"\n[[address-spaces]]\nid = \"cpu\"\naddress-width = 32\nendianness = \"little\"\n[[regions]]\nname = \"radio\"\naddress-space = \"cpu\"\nkind = \"peripheral\"\nstart = 0x1000\nend-exclusive = 0x2000\n",
    )
    .unwrap();
    std::fs::write(
        directory.join("chip.toml"),
        "schema = 3\nid = \"fixture-chip\"\nmemory-map = \"memory.toml\"\n",
    )
    .unwrap();
    std::fs::write(
        &project,
        "schema = 4\nid = \"fixture\"\ntarget-spec = \"target.toml\"\nchip-pack = \"chip.toml\"\n",
    )
    .unwrap();
    let memory_output = blobray()
        .current_dir(repository_root())
        .args(["project", "status", "--project"])
        .arg(&project)
        .args(["--format", "json", "--color", "never"])
        .output()
        .expect("diagnose invalid memory map");
    std::fs::remove_dir_all(directory).unwrap();

    assert!(!memory_output.status.success());
    assert!(memory_output.stdout.is_empty());
    let memory_stderr = String::from_utf8_lossy(&memory_output.stderr);
    assert!(memory_stderr.contains("kind = \"peripheral\""));
    assert!(memory_stderr.contains("memory.toml"));
    assert!(!memory_stderr.contains("Usage:"));
}

#[test]
fn malformed_composite_values_are_rejected_by_the_leaf_clap_grammar() {
    let output = run(&[
        "project",
        "init",
        "--directory",
        "/tmp/blobray-invalid-value-contract",
        "--id",
        "fixture",
        "--mmio",
        "invalid-range",
        "--format",
        "json",
        "--color",
        "never",
    ]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid value 'invalid-range' for '--mmio"));
    assert!(stderr.contains("For more information, try '--help'."));
}

#[test]
fn quiet_overrides_an_explicit_rust_log_filter() {
    let output = blobray()
        .current_dir(repository_root())
        .env("RUST_LOG", "warn")
        .args([
            "project",
            "status",
            "--project",
            GENERIC_PROJECT,
            "--format",
            "json",
            "--quiet",
            "--color",
            "never",
        ])
        .output()
        .expect("run quiet blobray");
    assert!(output.status.success());
    assert!(
        output.stderr.is_empty(),
        "quiet stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice::<serde_json::Value>(&output.stdout)
        .expect("quiet stdout must remain valid JSON");
}

#[test]
fn verify_source_json_is_one_typed_function_report() {
    let artifact = std::env::temp_dir().join(format!(
        "blobray-verify-source-contract-{}.o",
        std::process::id()
    ));
    write_rv32_symbol_fixture(&artifact);
    let output = blobray()
        .current_dir(repository_root())
        .args([
            "advanced",
            "verify",
            "source",
            "--project",
            GENERIC_PROJECT,
            "--vendor-artifact",
        ])
        .arg(&artifact)
        .arg("--rust-artifact")
        .arg(&artifact)
        .args([
            "--vendor-prefix",
            "fixture_",
            "--rust-prefix",
            "fixture_",
            "--format",
            "json",
            "--color",
            "never",
        ])
        .output()
        .expect("run source verification");
    std::fs::remove_file(artifact).unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("verification stdout must be valid JSON");
    let report = &document;
    assert_eq!(report["command"], "verify source");
    assert_eq!(report["passed"], true);
    assert_eq!(report["summary"]["vendor_functions"], 1);
    assert_eq!(report["sources"].as_array().unwrap().len(), 1);
    assert_eq!(report["sources"][0]["source"], "vendor");
    assert_eq!(
        report["sources"][0]["functions"].as_array().unwrap().len(),
        1
    );
    assert_eq!(
        report["sources"][0]["functions"][0]["vendor_symbol"],
        "fixture_entry"
    );
    assert_eq!(report["sources"][0]["functions"][0]["status"], "match");
}

#[test]
fn verify_inventory_json_combines_results_and_publication_in_one_report() {
    let suffix = std::process::id();
    let artifact =
        std::env::temp_dir().join(format!("blobray-verify-inventory-contract-{suffix}.o"));
    let report_path =
        std::env::temp_dir().join(format!("blobray-verify-inventory-contract-{suffix}.json"));
    write_rv32_symbol_fixture(&artifact);
    let output = blobray()
        .current_dir(repository_root())
        .args([
            "advanced",
            "verify",
            "inventory",
            "--project",
            GENERIC_PROJECT,
            "--source-artifact",
        ])
        .arg(format!("fixture={}", artifact.display()))
        .args(["--source-prefix", "fixture=fixture_", "--rust-artifact"])
        .arg(&artifact)
        .args(["--rust-prefix", "fixture_", "--output"])
        .arg(&report_path)
        .args(["--format", "json", "--color", "never"])
        .output()
        .expect("run inventory verification");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("verification stdout must be valid JSON");
    let report = &document;
    assert_eq!(report["command"], "verify inventory");
    assert_eq!(report["passed"], true);
    assert_eq!(report["inventory"][0]["source"], "fixture");
    assert_eq!(report["inventory"][0]["symbols"], 1);
    assert_eq!(report["sources"][0]["functions"][0]["status"], "match");
    assert_eq!(report["report"]["status"], "written");
    assert_eq!(report["report"]["path"], report_path.display().to_string());

    let persistent: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report_path).unwrap()).unwrap();
    assert_eq!(persistent["schema_version"], 16);
    assert_eq!(persistent["command"], "verify inventory");
    assert_eq!(persistent["sources"][0]["functions"][0]["status"], "match");
    std::fs::remove_file(artifact).unwrap();
    std::fs::remove_file(report_path).unwrap();
}

#[test]
fn project_verify_executes_typed_suites_and_reproduces_the_aggregate_report() {
    let suffix = std::process::id();
    let directory = std::env::temp_dir().join(format!("blobray-project-verify-contract-{suffix}"));
    if directory.exists() {
        std::fs::remove_dir_all(&directory).unwrap();
    }
    std::fs::create_dir_all(&directory).unwrap();
    let artifact = directory.join("fixture.o");
    write_rv32_symbol_fixture(&artifact);
    std::fs::write(
        directory.join("memory.toml"),
        include_str!("fixtures/generic-project/memory.toml"),
    )
    .unwrap();
    std::fs::write(
        directory.join("target.toml"),
        include_str!("fixtures/generic-project/target.toml"),
    )
    .unwrap();
    std::fs::write(
        directory.join("dispositions.toml"),
        "schema = 3\ndefault-disposition = \"not-yet-ported\"\ndefault-protocol = \"unknown\"\nfunctions = []\n",
    )
    .unwrap();
    std::fs::write(
        directory.join("baseline.toml"),
        "schema = 2\n\n[[evidence]]\nsource = \"fixture\"\nsymbol = \"fixture_entry\"\nkind = \"symbolic\"\n",
    )
    .unwrap();
    let manifest = directory.join("vendor-project.toml");
    std::fs::write(
        &manifest,
        r#"schema = 4
id = "project-verify-fixture"
target-spec = "target.toml"
chip-pack = "chip.toml"
verification-addon = "verification-addon.toml"
"#,
    )
    .unwrap();
    std::fs::write(
        directory.join("chip.toml"),
        "schema = 3\nid = \"fixture-chip\"\nmemory-map = \"memory.toml\"\nknowledge-packs = []\n",
    )
    .unwrap();
    std::fs::write(
        directory.join("verification-addon.toml"),
        r#"schema = 3
id = "fixture-verification"
report = "verification.json"
evidence-index = "vendor-evidence.json"

[[suites]]
id = "fixture"
rust-artifact-role = "rust-artifact:fixture"
rust-prefix = "fixture_"
profiles = []
dispositions = ["dispositions.toml"]
baselines = ["baseline.toml"]
gate = "completion"

[[suites.vendor]]
source = "fixture"
prefix = "fixture_"
"#,
    )
    .unwrap();
    let run_spec = directory.join("local.toml");
    std::fs::write(
        &run_spec,
        format!(
            "schema = 1\n\n[[inputs]]\nrole = \"source-artifact:fixture\"\npath = {:?}\n\n[[inputs]]\nrole = \"rust-artifact:fixture\"\npath = {:?}\n",
            artifact, artifact
        ),
    )
    .unwrap();
    let candidates = directory.join("candidates");
    std::fs::create_dir(&candidates).unwrap();

    let output = blobray()
        .args(["project", "verify", "--project"])
        .arg(&manifest)
        .args(["--run-spec"])
        .arg(&run_spec)
        .args(["--candidate-evidence-dir"])
        .arg(&candidates)
        .args([
            "--format",
            "json",
            "--color",
            "never",
            "--progress",
            "never",
        ])
        .output()
        .expect("run project verification");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["command"], "project verify");
    assert_eq!(document["schema_version"], 16);
    assert!(
        document["suites"][0]["artifacts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|artifact| std::path::Path::new(artifact["path"].as_str().unwrap()).is_absolute())
    );
    assert_eq!(document["passed"], true);
    assert_eq!(document["complete_project_run"], true);
    assert_eq!(document["suites"][0]["id"], "fixture");
    assert_eq!(document["suites"][0]["summary"]["matched"], 1);
    assert_eq!(
        document["replacement_graph"]["summary"]["vendor_functions"],
        1
    );
    assert_eq!(
        document["replacement_graph"]["summary"]["behavioral_matches"],
        1
    );
    assert_eq!(
        document["replacement_graph"]["summary"]["probe_only_matches"],
        1
    );
    assert_eq!(
        document["rust_component_index"]["summary"]["reviewed_components"],
        0
    );
    assert_eq!(
        document["replacement_graph"]["replacements"][0]["proofs"][0]["suite"],
        "fixture"
    );
    let candidate = std::fs::read_to_string(candidates.join("fixture.toml")).unwrap();
    assert!(candidate.contains("symbol = \"fixture_entry\""));
    assert!(candidate.contains("kind = \"symbolic\""));
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(
            &std::fs::read_to_string(directory.join("verification.json")).unwrap()
        )
        .unwrap(),
        document
    );
    let evidence_index: serde_json::Value =
        serde_json::from_slice(&std::fs::read(directory.join("vendor-evidence.json")).unwrap())
            .unwrap();
    assert_eq!(evidence_index["schema_version"], 1);
    assert_eq!(
        evidence_index["entries"][0]["evidence_class"],
        "static-analysis"
    );
    assert_eq!(evidence_index["entries"][0]["release_eligible"], false);
    assert!(
        evidence_index["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| {
                entry["source_hashes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|source| {
                        !std::path::Path::new(source["path"].as_str().unwrap()).is_absolute()
                    })
                    && entry["artifact_hashes"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .all(|artifact| artifact.get("path").is_none())
            })
    );

    let check = blobray()
        .args(["project", "verify", "--project"])
        .arg(&manifest)
        .args(["--run-spec"])
        .arg(&run_spec)
        .args([
            "--check",
            "--format",
            "json",
            "--color",
            "never",
            "--progress",
            "never",
        ])
        .output()
        .expect("check project verification");
    assert!(
        check.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&check.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&check.stdout).unwrap(),
        document
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn direct_target_audit_json_is_one_typed_report() {
    let artifact = std::env::temp_dir().join(format!(
        "blobray-direct-target-audit-contract-{}.o",
        std::process::id()
    ));
    write_rv32_symbol_fixture(&artifact);
    let mut image = std::fs::read(&artifact).unwrap();
    image[16..18].copy_from_slice(&2_u16.to_le_bytes());
    std::fs::write(&artifact, image).unwrap();
    let output = blobray()
        .current_dir(repository_root())
        .args([
            "advanced",
            "image",
            "audit-targets",
            "--project",
            GENERIC_PROJECT,
            "--artifact",
        ])
        .arg(&artifact)
        .args([
            "--forbid",
            "vendor-rom=0x40000000..0x40001000",
            "--format",
            "json",
            "--color",
            "never",
        ])
        .output()
        .expect("run direct-target audit");
    std::fs::remove_file(artifact).unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["command"], "image audit-targets");
    assert_eq!(document["passed"], true);
    assert_eq!(document["forbidden_targets"], serde_json::json!([]));
}

#[test]
fn project_analysis_and_ci_check_are_distinct_typed_entry_points() {
    let help = run(&["project", "analyze", "--help"]);
    assert!(help.status.success());
    let help = String::from_utf8_lossy(&help.stdout);
    assert!(help.contains("--plan"));
    assert!(help.contains("--check"));
    assert!(help.contains("--deny-unreviewed"));
    assert!(help.contains("--progress"));

    let check_help = run(&["project", "check", "--help"]);
    assert!(check_help.status.success());
    let check_help = String::from_utf8_lossy(&check_help.stdout);
    assert!(check_help.contains("--deny-unreviewed"));
    assert!(check_help.contains("--jobs"));

    let removed = run(&["project", "build"]);
    assert_eq!(removed.status.code(), Some(2));
    assert!(removed.stdout.is_empty());
    assert!(String::from_utf8_lossy(&removed.stderr).contains("unrecognized subcommand"));
}

#[test]
fn project_analysis_emits_a_typed_summary_when_inputs_are_blocked() {
    let (directory, manifest) = init_temporary_project("blocked-analysis");
    let output = blobray()
        .current_dir(repository_root())
        .args(["project", "analyze", "--check", "--project"])
        .arg(&manifest)
        .args(["--format", "json", "--color", "never"])
        .output()
        .expect("run blocked project analysis");
    assert_eq!(output.status.code(), Some(2));
    let document: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("analysis stdout must be valid JSON");
    assert_eq!(document["schema"], 6);
    assert_eq!(document["command"], "project analyze");
    assert_eq!(document["mode"], "check");
    assert_eq!(document["status"], "failed");
    assert!(document.get("duration_ms").is_none());
    assert!(
        document["stages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|stage| stage.get("duration_ms").is_none())
    );
    assert!(document["blocked"].as_u64().unwrap() > 0);
    assert!(document["stages"].is_array());
    assert_eq!(
        &document["next_steps"][0]["commands"][0]["argv"]
            .as_array()
            .unwrap()[..3],
        serde_json::json!(["blobray", "project", "doctor"])
            .as_array()
            .unwrap()
    );
    assert!(document.get("next_actions").is_none());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("Project analysis"));
    assert!(!stderr.contains("Project stage:"));
    assert!(
        document["stages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|stage| { stage["name"] == "symbol-inventory" && stage["status"] == "blocked" })
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn project_analysis_blocked_by_preflight_does_not_create_persistent_state() {
    let (directory, manifest) = init_temporary_project("blocked-write-analysis");
    let before = snapshot_tree(&directory);

    let output = run_project_command(&manifest, &["project", "analyze"]);
    assert_eq!(output.status.code(), Some(2));
    let document: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("analysis stdout must be valid JSON");
    assert_eq!(document["mode"], "write");
    assert_eq!(document["status"], "failed");
    assert_eq!(document["written"], 0);
    assert_eq!(document["verified"], 0);
    assert_eq!(document["up-to-date"], 0);
    assert!(document["blocked"].as_u64().unwrap() > 0);
    assert!(
        document["stages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|stage| stage.get("duration_ms").is_none())
    );
    assert_eq!(snapshot_tree(&directory), before);
    assert!(!directory.join("generated/.blobray-cache").exists());

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn project_analysis_plan_is_deterministic_and_read_only_when_blocked() {
    let (directory, manifest) = init_temporary_project("blocked-analysis-plan");
    let before = snapshot_tree(&directory);

    let first = run_project_command(&manifest, &["project", "analyze", "--plan"]);
    assert_eq!(first.status.code(), Some(2));
    let document: serde_json::Value =
        serde_json::from_slice(&first.stdout).expect("analysis plan stdout must be valid JSON");
    assert_eq!(document["schema"], 2);
    assert_eq!(document["command"], "project analyze --plan");
    assert_eq!(document["mode"], "write");
    assert_eq!(document["read_only"], true);
    assert_eq!(document["status"], "failed");
    let stages = document["stages"].as_array().unwrap();
    assert_eq!(stages.len(), 15);
    assert_eq!(stages[0]["order"], 1);
    assert_eq!(stages[0]["name"], "symbol-inventory");
    assert_eq!(stages[0]["action"], "blocked");
    assert!(
        stages[0]["cause"]
            .as_str()
            .unwrap()
            .contains("run-spec is not configured")
    );
    assert_eq!(snapshot_tree(&directory), before);
    assert!(!directory.join("generated/.blobray-cache").exists());

    let second = run_project_command(&manifest, &["project", "analyze", "--plan"]);
    assert_eq!(second.status.code(), Some(2));
    assert_eq!(second.stdout, first.stdout);
    assert_eq!(snapshot_tree(&directory), before);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn project_analysis_plan_fails_closed_for_a_missing_bound_input_in_every_mode() {
    let (directory, manifest) = init_temporary_project("missing-analysis-plan-input");
    let missing = directory.join("private/missing-vendor.a");
    std::fs::write(
        directory.join("local.toml"),
        format!(
            "schema = 1\n\n[[inputs]]\nrole = \"source-artifact:vendor\"\npath = {:?}\n",
            missing.display().to_string()
        ),
    )
    .unwrap();
    let before = snapshot_tree(&directory);

    for arguments in [
        ["project", "analyze", "--plan"].as_slice(),
        ["project", "analyze", "--plan", "--check"].as_slice(),
    ] {
        let output = run_project_command(&manifest, arguments);
        assert_eq!(
            output.status.code(),
            Some(2),
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let document: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(document["status"], "failed");
        let symbols = document["stages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|stage| stage["name"] == "symbol-inventory")
            .unwrap();
        assert_eq!(symbols["action"], "failed");
        assert!(
            symbols["cause"]
                .as_str()
                .unwrap()
                .contains("analysis input")
        );
        assert!(
            symbols["cause"]
                .as_str()
                .unwrap()
                .contains("missing-vendor.a")
        );
        for name in ["mmio-discovery", "interface-discovery", "linked-ir"] {
            let dependant = document["stages"]
                .as_array()
                .unwrap()
                .iter()
                .find(|stage| stage["name"] == name)
                .unwrap();
            assert!(
                dependant["depends-on"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|dependency| dependency == "symbol-inventory"),
                "{name} must expose its generated symbol dependency"
            );
        }
        assert_eq!(snapshot_tree(&directory), before);
    }

    assert!(!directory.join("generated/.blobray-cache").exists());
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn project_analysis_plan_tracks_soft_symbol_materialization_for_linked_ir() {
    let directory = std::env::temp_dir().join(format!(
        "blobray-soft-symbol-plan-contract-{}",
        std::process::id()
    ));
    if directory.exists() {
        std::fs::remove_dir_all(&directory).unwrap();
    }
    std::fs::create_dir_all(&directory).unwrap();
    let target = repository_root().join("tools/blobray/tests/fixtures/generic-project/target.toml");
    let manifest = directory.join("vendor-project.toml");
    std::fs::write(
        &manifest,
        format!(
            "schema = 4\nid = \"soft-symbol-plan\"\ntarget-spec = {:?}\n\n[analysis.symbols]\noutput = \"generated/symbols.json\"\n\n[[analysis.ir]]\nid = \"fixture\"\nsources = [\"fixture\"]\nroots = \"all\"\ninclude-reachable = true\nentry-contract = \"none\"\noutput = \"generated/fixture.ir\"\n",
            target.display().to_string()
        ),
    )
    .unwrap();
    let artifact = directory.join("vendor.o");
    write_rv32_symbol_fixture(&artifact);
    std::fs::write(
        directory.join("local.toml"),
        format!(
            "schema = 1\n\n[[inputs]]\nrole = \"source-artifact:fixture\"\npath = {:?}\n",
            artifact.display().to_string()
        ),
    )
    .unwrap();
    let before = snapshot_tree(&directory);

    let output = run_project_command(&manifest, &["project", "analyze", "--plan"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let linked = document["stages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|stage| stage["name"] == "linked-ir")
        .unwrap();
    assert_eq!(linked["action"], "deferred");
    assert_eq!(linked["depends-on"], serde_json::json!([]));
    assert_eq!(
        linked["optional-depends-on"],
        serde_json::json!(["symbol-inventory"])
    );
    let item = &linked["work-items"][0];
    assert_eq!(item["action"], "deferred");
    assert!(
        item["cause"]
            .as_str()
            .is_some_and(|cause| cause.len() < 256 && cause.contains("1 generated input"))
    );
    assert_eq!(
        item["awaiting-inputs"][0]["producer-stage"],
        "symbol-inventory"
    );
    assert!(
        item["awaiting-inputs"][0]["path"]
            .as_str()
            .is_some_and(|path| path.ends_with("generated/symbols.json"))
    );
    assert_eq!(snapshot_tree(&directory), before);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn project_analysis_plan_preflights_inputs_independent_of_generated_predecessors() {
    let directory = std::env::temp_dir().join(format!(
        "blobray-independent-plan-input-{}",
        std::process::id()
    ));
    if directory.exists() {
        std::fs::remove_dir_all(&directory).unwrap();
    }
    std::fs::create_dir_all(&directory).unwrap();
    let target = repository_root().join("tools/blobray/tests/fixtures/generic-project/target.toml");
    let missing_code_pack = directory.join("missing-code-pack.toml");
    let manifest = directory.join("vendor-project.toml");
    std::fs::write(
        &manifest,
        format!(
            "schema = 4\nid = \"independent-plan-input\"\ntarget-spec = {:?}\n\n[analysis.symbols]\noutput = \"generated/symbols.json\"\n\n[code]\npack = {:?}\n\n[[analysis.ir]]\nid = \"fixture\"\nsources = [\"fixture\"]\nroots = \"all\"\ninclude-reachable = true\nentry-contract = \"none\"\noutput = \"generated/fixture.ir\"\n",
            target.display().to_string(),
            missing_code_pack.display().to_string(),
        ),
    )
    .unwrap();
    let artifact = directory.join("vendor.o");
    write_rv32_symbol_fixture(&artifact);
    std::fs::write(
        directory.join("local.toml"),
        format!(
            "schema = 1\n\n[[inputs]]\nrole = \"source-artifact:fixture\"\npath = {:?}\n",
            artifact.display().to_string()
        ),
    )
    .unwrap();
    let before = snapshot_tree(&directory);

    let output = run_project_command(&manifest, &["project", "analyze", "--plan"]);
    assert_eq!(
        output.status.code(),
        Some(2),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["status"], "failed");
    let linked = document["stages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|stage| stage["name"] == "linked-ir")
        .unwrap();
    assert_eq!(linked["action"], "failed");
    assert!(
        linked["cause"]
            .as_str()
            .unwrap()
            .contains("missing-code-pack.toml")
    );
    assert_eq!(snapshot_tree(&directory), before);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn discovery_plan_ignores_missing_run_bindings_that_the_stage_does_not_consume() {
    let directory = std::env::temp_dir().join(format!(
        "blobray-stage-specific-run-inputs-{}",
        std::process::id()
    ));
    if directory.exists() {
        std::fs::remove_dir_all(&directory).unwrap();
    }
    std::fs::create_dir_all(&directory).unwrap();
    let fixture = repository_root().join("tools/blobray/tests/fixtures/generic-project");
    let manifest = directory.join("vendor-project.toml");
    std::fs::write(
        &manifest,
        format!(
            "schema = 4\nid = \"stage-input-plan\"\ntarget-spec = {:?}\nchip-pack = {:?}\n\n[registers]\nfacts = \"generated/mmio.json\"\nmodel = \"reviewed/registers.toml\"\nowned-ranges = [\"fixture-mmio\"]\n\n[interfaces]\nfacts = \"generated/interfaces.json\"\n",
            fixture.join("target.toml").display().to_string(),
            fixture.join("chip.toml").display().to_string(),
        ),
    )
    .unwrap();
    let artifact = directory.join("vendor.o");
    write_rv32_symbol_fixture(&artifact);
    std::fs::write(
        directory.join("local.toml"),
        format!(
            "schema = 1\n\n[[inputs]]\nrole = \"source-artifact:fixture\"\npath = {:?}\n\n[[inputs]]\nrole = \"source-companion:unused\"\npath = {:?}\n",
            artifact.display().to_string(),
            directory.join("missing-unused.debug").display().to_string(),
        ),
    )
    .unwrap();
    std::fs::create_dir_all(directory.join("reviewed/peripherals")).unwrap();
    std::fs::write(
        directory.join("reviewed/registers.toml"),
        "schema = 3\nchip = \"fixture-chip\"\nfragments = [\"peripherals/fixture.toml\"]\n\n[device]\nname = \"fixture\"\nversion = \"1\"\ndescription = \"fixture\"\naddress-unit-bits = 8\nwidth = 32\n",
    )
    .unwrap();
    std::fs::write(
        directory.join("reviewed/peripherals/fixture.toml"),
        "schema = 2\n\n[[peripherals]]\nname = \"FIXTURE\"\nbaseAddress = 0x20000000\n",
    )
    .unwrap();

    let output = run_project_command(&manifest, &["project", "analyze", "--plan"]);
    assert!(
        output.status.success(),
        "stderr: {}\nstdout: {}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout),
    );
    let document: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    for name in ["mmio-discovery", "interface-discovery"] {
        let stage = document["stages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|stage| stage["name"] == name)
            .unwrap();
        assert_eq!(stage["action"], "compute", "{name}: {stage}");
    }
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn project_analysis_plan_distinguishes_current_and_restorable_outputs_without_restoring() {
    let directory = std::env::temp_dir().join(format!(
        "blobray-analysis-plan-cache-contract-{}",
        std::process::id()
    ));
    if directory.exists() {
        std::fs::remove_dir_all(&directory).unwrap();
    }
    std::fs::create_dir_all(&directory).unwrap();
    let target = repository_root().join("tools/blobray/tests/fixtures/generic-project/target.toml");
    let manifest = directory.join("vendor-project.toml");
    std::fs::write(
        &manifest,
        format!(
            "schema = 4\nid = \"analysis-plan-cache\"\ntarget-spec = {:?}\n\n[analysis.symbols]\noutput = \"generated/symbols.json\"\n",
            target.display().to_string()
        ),
    )
    .unwrap();
    let artifact = directory.join("vendor.o");
    write_rv32_symbol_fixture(&artifact);
    let inputs = blobray()
        .current_dir(repository_root())
        .args(["project", "inputs", "init", "--project"])
        .arg(&manifest)
        .arg("--bind")
        .arg(format!("source-artifact:fixture={}", artifact.display()))
        .args(["--format", "json", "--color", "never"])
        .output()
        .expect("initialize analysis plan inputs");
    assert!(
        inputs.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&inputs.stderr)
    );
    let generated = run_project_command(&manifest, &["project", "analyze"]);
    assert!(
        generated.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&generated.stderr)
    );

    let before_current = snapshot_tree(&directory);
    let current = run_project_command(&manifest, &["project", "analyze", "--plan"]);
    assert!(
        current.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&current.stderr)
    );
    let current: serde_json::Value = serde_json::from_slice(&current.stdout).unwrap();
    let symbols = current["stages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|stage| stage["name"] == "symbol-inventory")
        .unwrap();
    assert_eq!(symbols["action"], "current");
    assert!(symbols["work-items"][0]["signature"].is_string());
    assert_eq!(snapshot_tree(&directory), before_current);

    let symbols_path = directory.join("generated/symbols.json");
    std::fs::remove_file(&symbols_path).unwrap();
    let before_restore = snapshot_tree(&directory);
    let restore = run_project_command(&manifest, &["project", "analyze", "--plan"]);
    assert!(restore.status.success());
    let restore: serde_json::Value = serde_json::from_slice(&restore.stdout).unwrap();
    let symbols = restore["stages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|stage| stage["name"] == "symbol-inventory")
        .unwrap();
    assert_eq!(symbols["action"], "restore");
    assert!(
        symbols["cause"]
            .as_str()
            .unwrap()
            .contains("missing or differ")
    );
    assert!(!symbols_path.exists());
    assert_eq!(snapshot_tree(&directory), before_restore);

    let restored = run_project_command(&manifest, &["project", "analyze"]);
    assert!(
        restored.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&restored.stderr)
    );
    let restored: serde_json::Value = serde_json::from_slice(&restored.stdout).unwrap();
    assert_eq!(restored["schema"], 6);
    assert_eq!(restored["written"], 0);
    assert_eq!(restored["restored"], 1);
    assert_eq!(restored["up-to-date"], 0);
    let symbols = restored["stages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|stage| stage["name"] == "symbol-inventory")
        .unwrap();
    assert_eq!(symbols["status"], "restored");
    assert!(symbols_path.is_file());

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn project_analysis_reports_nothing_configured_as_a_non_successful_noop() {
    let directory = std::env::temp_dir().join(format!(
        "blobray-nothing-configured-contract-{}",
        std::process::id()
    ));
    if directory.exists() {
        std::fs::remove_dir_all(&directory).unwrap();
    }
    std::fs::create_dir_all(&directory).unwrap();
    let target = repository_root().join("tools/blobray/tests/fixtures/generic-project/target.toml");
    let manifest = directory.join("vendor-project.toml");
    std::fs::write(
        &manifest,
        format!(
            "schema = 4\nid = \"nothing-configured\"\ntarget-spec = {:?}\n",
            target.display().to_string()
        ),
    )
    .unwrap();

    let human = blobray()
        .current_dir(repository_root())
        .args(["project", "analyze", "--project"])
        .arg(&manifest)
        .args(["--color", "never"])
        .output()
        .expect("run nothing-configured project analysis");
    assert_eq!(human.status.code(), Some(2));
    let stdout = String::from_utf8_lossy(&human.stdout);
    assert!(stdout.contains("NOTHING CONFIGURED"), "stdout: {stdout}");
    assert!(!stdout.contains("READY"), "stdout: {stdout}");
    assert!(
        stdout.contains("Duration: not measured"),
        "stdout: {stdout}"
    );
    assert!(stdout.contains("Duration"), "stdout: {stdout}");

    let json = blobray()
        .current_dir(repository_root())
        .args(["project", "analyze", "--project"])
        .arg(&manifest)
        .args(["--format", "json", "--color", "never"])
        .output()
        .expect("run typed nothing-configured project analysis");
    assert_eq!(json.status.code(), Some(2));
    let document: serde_json::Value =
        serde_json::from_slice(&json.stdout).expect("analysis stdout must be valid JSON");
    assert_eq!(document["schema"], 6);
    assert_eq!(document["command"], "project analyze");
    assert_eq!(document["status"], "nothing-configured");
    assert_eq!(document["not-configured"], 15);
    assert_eq!(document["written"], 0);
    assert_eq!(document["verified"], 0);
    assert_eq!(document["up-to-date"], 0);
    assert_eq!(
        &document["next_steps"][0]["commands"][0]["argv"]
            .as_array()
            .unwrap()[..3],
        serde_json::json!(["blobray", "project", "analyze"])
            .as_array()
            .unwrap()
    );
    assert!(document.get("duration_ms").is_none());
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn project_inputs_validate_elf_and_archive_roles_before_writing() {
    let directory = std::env::temp_dir().join(format!(
        "blobray-project-inputs-contract-{}",
        std::process::id()
    ));
    if directory.exists() {
        std::fs::remove_dir_all(&directory).unwrap();
    }
    std::fs::create_dir_all(&directory).unwrap();
    let target = repository_root().join("tools/blobray/tests/fixtures/generic-project/target.toml");
    let manifest = directory.join("vendor-project.toml");
    std::fs::write(
        &manifest,
        format!(
            "schema = 4\nid = \"input-contract\"\ntarget-spec = {:?}\n\n[[analysis.ir]]\nid = \"fixture\"\nsources = [\"fixture\"]\nroots = \"all\"\noutput = \"generated/fixture.ir\"\n",
            target.display().to_string()
        ),
    )
    .unwrap();
    let artifact = directory.join("vendor.o");
    let archive = directory.join("vendor.a");
    write_rv32_symbol_fixture(&artifact);
    write_rv32_archive_fixture(&archive);

    let output = blobray()
        .current_dir(repository_root())
        .args(["project", "inputs", "init", "--project"])
        .arg(&manifest)
        .arg("--bind")
        .arg(format!("source-artifact:fixture={}", artifact.display()))
        .arg("--bind")
        .arg(format!("source-inventory:fixture={}", archive.display()))
        .args(["--format", "json", "--color", "never"])
        .output()
        .expect("initialize typed ELF/archive bindings");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["bindings"][0]["container"], "elf32");
    assert_eq!(report["bindings"][1]["container"], "archive");

    let invalid = blobray()
        .current_dir(repository_root())
        .args(["project", "inputs", "init", "--force", "--project"])
        .arg(&manifest)
        .arg("--bind")
        .arg(format!("source-artifact:fixture={}", archive.display()))
        .args(["--format", "json", "--color", "never"])
        .output()
        .expect("reject archive bound as a linked artifact");
    assert!(!invalid.status.success());
    assert!(invalid.stdout.is_empty());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("requires elf32"));

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn project_analyze_check_does_not_create_persistent_query_store() {
    let directory = std::env::temp_dir().join(format!(
        "blobray-read-only-analysis-contract-{}",
        std::process::id()
    ));
    if directory.exists() {
        std::fs::remove_dir_all(&directory).unwrap();
    }
    std::fs::create_dir_all(&directory).unwrap();
    let target = repository_root().join("tools/blobray/tests/fixtures/generic-project/target.toml");
    let manifest = directory.join("vendor-project.toml");
    std::fs::write(
        &manifest,
        format!(
            "schema = 4\nid = \"read-only-analysis\"\ntarget-spec = {:?}\n\n[analysis.symbols]\noutput = \"generated/symbols.json\"\n\n[analysis.navigation]\noutput = \"generated/navigation.json\"\n\n[[analysis.ir]]\nid = \"fixture\"\nsources = [\"fixture\"]\nroots = \"all\"\ninclude-reachable = true\nentry-contract = \"none\"\noutput = \"generated/fixture.ir\"\n",
            target.display().to_string()
        ),
    )
    .unwrap();
    let artifact = directory.join("vendor.o");
    write_rv32_symbol_fixture(&artifact);

    let inputs = blobray()
        .current_dir(repository_root())
        .args(["project", "inputs", "init", "--project"])
        .arg(&manifest)
        .arg("--bind")
        .arg(format!("source-artifact:fixture={}", artifact.display()))
        .args(["--format", "json", "--color", "never"])
        .output()
        .expect("initialize project inputs");
    assert!(
        inputs.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&inputs.stderr)
    );

    let generated = run_project_command(&manifest, &["project", "analyze"]);
    assert!(
        generated.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&generated.stderr)
    );
    let cache = directory.join("generated/.blobray-cache");
    assert!(
        cache.is_dir(),
        "write mode should initialize the query store"
    );
    std::fs::remove_dir_all(&cache).unwrap();

    let checked = run_project_command(&manifest, &["project", "analyze", "--check"]);
    assert!(
        checked.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&checked.stderr)
    );
    assert!(
        !cache.exists(),
        "project analyze --check created persistent query-store state"
    );

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn project_symbol_inventory_writes_and_checks_its_manifest_owned_report() {
    let directory = std::env::temp_dir().join(format!(
        "blobray-symbol-project-contract-{}",
        std::process::id()
    ));
    if directory.exists() {
        std::fs::remove_dir_all(&directory).unwrap();
    }
    std::fs::create_dir_all(&directory).unwrap();
    let target = repository_root().join("tools/blobray/tests/fixtures/generic-project/target.toml");
    let manifest = directory.join("vendor-project.toml");
    std::fs::write(
        &manifest,
        format!(
            "schema = 4\nid = \"symbol-contract\"\ntarget-spec = {:?}\n\n[analysis.symbols]\noutput = \"generated/symbols.json\"\n\n[analysis.navigation]\noutput = \"generated/navigation.json\"\n\n[[analysis.ir]]\nid = \"fixture\"\nsources = [\"fixture\"]\nroots = \"all\"\ninclude-reachable = true\nentry-contract = \"none\"\noutput = \"generated/fixture.ir\"\n\n[interfaces]\nfacts = \"generated/interfaces.json\"\n",
            target.display().to_string()
        ),
    )
    .unwrap();
    let artifact = directory.join("vendor.o");
    write_rv32_symbol_fixture(&artifact);
    let run_spec = directory.join("local.toml");
    let inputs = blobray()
        .current_dir(repository_root())
        .args(["project", "inputs", "init", "--project"])
        .arg(&manifest)
        .arg("--bind")
        .arg(format!("source-artifact:fixture={}", artifact.display()))
        .args(["--format", "json", "--color", "never"])
        .output()
        .expect("initialize project inputs");
    assert!(
        inputs.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&inputs.stderr)
    );
    let inputs: serde_json::Value = serde_json::from_slice(&inputs.stdout).unwrap();
    assert_eq!(inputs["command"], "project inputs init");
    assert_eq!(inputs["status"], "written");
    assert_eq!(inputs["output"], run_spec.display().to_string());
    assert_eq!(inputs["bindings"][0]["role"], "source-artifact:fixture");

    let checked_inputs = blobray()
        .current_dir(repository_root())
        .args(["project", "inputs", "init", "--check", "--project"])
        .arg(&manifest)
        .arg("--bind")
        .arg(format!("source-artifact:fixture={}", artifact.display()))
        .args(["--format", "json", "--color", "never"])
        .output()
        .expect("check project inputs");
    assert!(checked_inputs.status.success());
    let checked_inputs: serde_json::Value = serde_json::from_slice(&checked_inputs.stdout).unwrap();
    assert_eq!(checked_inputs["status"], "verified");

    for (check, expected_stage_status) in [
        (false, "written"),
        (false, "up-to-date"),
        (true, "verified"),
    ] {
        let mut command = blobray();
        command
            .current_dir(repository_root())
            .args(["project", "analyze", "--project"])
            .arg(&manifest)
            .args(["--format", "json", "--color", "never"]);
        if check {
            command.arg("--check");
        }
        let output = command.output().expect("run project symbol inventory");
        assert!(
            output.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let document: serde_json::Value = serde_json::from_slice(&output.stdout)
            .expect("project analysis stdout must be valid JSON");
        assert_eq!(document["schema"], 6);
        assert_eq!(document["command"], "project analyze");
        assert_eq!(document["status"], "ok");
        let measured_total = document["stages"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|stage| stage.get("duration_ms").and_then(serde_json::Value::as_u64))
            .sum::<u64>();
        assert_eq!(document["duration_ms"], measured_total);
        if expected_stage_status == "up-to-date" {
            assert_eq!(document["written"], 0);
            assert_eq!(document["verified"], 0);
            assert!(document["up-to-date"].as_u64().unwrap() > 0);
        }
        let symbol_stage = document["stages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|stage| stage["name"] == "symbol-inventory")
            .expect("symbol-inventory stage");
        assert_eq!(symbol_stage["status"], expected_stage_status);
        let navigation_stage = document["stages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|stage| stage["name"] == "navigation-index")
            .expect("navigation-index stage");
        assert_eq!(navigation_stage["status"], expected_stage_status);
    }

    let manifest_contents = std::fs::read_to_string(&manifest).unwrap()
        + "\n[code]\npack = \"code/boundaries.toml\"\n\n[code.review]\noutput = \"generated/code-boundaries.md\"\n";
    std::fs::write(&manifest, manifest_contents).unwrap();
    for (command, expected) in [
        ("init-pack", "created"),
        ("validate", "valid"),
        ("review", "written"),
    ] {
        let output = blobray()
            .current_dir(repository_root())
            .args(["advanced", "code", command, "--project"])
            .arg(&manifest)
            .args(["--format", "json", "--color", "never"])
            .output()
            .expect("run reviewed code-boundary lifecycle");
        assert!(
            output.status.success(),
            "code {command} stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["status"], expected);
    }

    let checked = blobray()
        .current_dir(repository_root())
        .args(["project", "analyze", "--check", "--project"])
        .arg(&manifest)
        .args(["--format", "json", "--color", "never"])
        .output()
        .expect("check project analysis with reviewed code boundaries");
    assert!(
        checked.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&checked.stderr)
    );
    let checked: serde_json::Value = serde_json::from_slice(&checked.stdout).unwrap();
    for name in ["code-boundary-validation", "code-boundary-review"] {
        assert!(
            checked["stages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|stage| stage["name"] == name && stage["status"] == "verified")
        );
    }

    let ir_build = blobray()
        .current_dir(repository_root())
        .args(["advanced", "ir", "build", "--check", "--project"])
        .arg(&manifest)
        .args(["--format", "json", "--color", "never"])
        .output()
        .expect("run typed IR build report");
    assert!(
        ir_build.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&ir_build.stderr)
    );
    let ir_build: serde_json::Value =
        serde_json::from_slice(&ir_build.stdout).expect("IR build stdout must be valid JSON");
    assert_eq!(ir_build["command"], "ir build");
    assert_eq!(ir_build["mode"], "check");
    assert_eq!(ir_build["status"], "verified");
    assert_eq!(ir_build["profiles"][0]["id"], "fixture");

    let manifest_contents = std::fs::read_to_string(&manifest).unwrap().replace(
        "[interfaces]\nfacts = \"generated/interfaces.json\"\n",
        "[interfaces]\nfacts = \"generated/interfaces.json\"\npack = \"interfaces/reviewed.toml\"\n\n[interfaces.capability-context]\noutput = \"generated/interface-capability-context.json\"\n\n[functions]\npack = \"functions/reviewed.toml\"\nprofiles = [\"fixture\"]\n",
    );
    std::fs::write(&manifest, manifest_contents).unwrap();
    std::fs::create_dir_all(directory.join("interfaces")).unwrap();
    std::fs::create_dir_all(directory.join("functions")).unwrap();

    for domain in ["interfaces", "functions"] {
        let output = blobray()
            .current_dir(repository_root())
            .arg("advanced")
            .args([domain, "init-pack", "--project"])
            .arg(&manifest)
            .args(["--format", "json", "--color", "never"])
            .output()
            .expect("initialize reviewed workspace pack");
        assert!(
            output.status.success(),
            "{domain} init stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let output: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(output["status"], "created");
    }

    for domain in ["interfaces", "functions"] {
        let output = blobray()
            .current_dir(repository_root())
            .arg("advanced")
            .args([domain, "validate", "--project"])
            .arg(&manifest)
            .args(["--format", "json", "--color", "never"])
            .output()
            .expect("validate reviewed workspace pack");
        assert!(
            output.status.success(),
            "{domain} validate stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let output: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(output["status"], "valid");
    }

    let denied = blobray()
        .current_dir(repository_root())
        .args([
            "advanced",
            "functions",
            "validate",
            "--deny-unreviewed",
            "--project",
        ])
        .arg(&manifest)
        .args(["--format", "json", "--color", "never"])
        .output()
        .expect("enforce reviewed function coverage");
    assert_eq!(denied.status.code(), Some(2));
    let denied: serde_json::Value = serde_json::from_slice(&denied.stdout).unwrap();
    assert_eq!(denied["status"], "unreviewed");
    assert_eq!(denied["deny_unreviewed"], true);

    let report = directory.join("generated/symbols.json");
    let document: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report).unwrap()).unwrap();
    assert_eq!(document["schema_version"], 5);
    assert_eq!(document["command"], "symbols inventory");
    assert!(document["summary"]["symbol_facts"].as_u64().unwrap() > 0);
    assert!(document["summary"]["executable_bytes"].as_u64().unwrap() > 0);
    assert!(!document["code_sections"].as_array().unwrap().is_empty());
    assert!(document["summary"]["function_boundary_candidates"].is_number());
    assert!(
        document["code_sections"]
            .as_array()
            .unwrap()
            .iter()
            .all(|section| section["function_candidates"].is_array()
                && section["recovery_blockers"].is_array())
    );
    let navigation: serde_json::Value = serde_json::from_slice(
        &std::fs::read(directory.join("generated/navigation.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(navigation["schema_version"], 3);
    assert_eq!(navigation["summary"]["linked_ir_functions"], 1);
    let fixture = navigation["symbols"]
        .as_array()
        .unwrap()
        .iter()
        .find(|symbol| symbol["name"] == "fixture_entry")
        .expect("fixture_entry navigation symbol");
    assert_eq!(fixture["inventory"].as_array().unwrap().len(), 1);
    assert_eq!(fixture["linked_ir"].as_array().unwrap().len(), 1);
    assert!(fixture["id"].as_str().unwrap().starts_with("symbol-v1:"));

    let status = blobray()
        .current_dir(repository_root())
        .args(["project", "status", "--project"])
        .arg(&manifest)
        .args(["--format", "json", "--color", "never"])
        .output()
        .expect("run project status");
    assert!(status.status.success());
    let status: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    let symbol_component = status["phases"]["analysis"]["components"]
        .as_array()
        .unwrap()
        .iter()
        .find(|component| component["name"] == "symbol_inventory")
        .expect("symbol_inventory component");
    assert_eq!(symbol_component["status"], "ready");
    assert!(symbol_component["bytes"].as_u64().unwrap() > 0);
    assert_eq!(symbol_component["validation_depth"], "shallow");
    assert_eq!(symbol_component["freshness"], "unknown");
    assert_eq!(
        symbol_component["deep_validation"],
        "project doctor / project check"
    );
    let navigation_component = status["phases"]["analysis"]["components"]
        .as_array()
        .unwrap()
        .iter()
        .find(|component| component["name"] == "navigation_index")
        .expect("navigation_index component");
    assert_eq!(navigation_component["status"], "ready");
    assert!(navigation_component["bytes"].as_u64().unwrap() > 0);
    assert_eq!(navigation_component["validation_depth"], "shallow");
    assert_eq!(navigation_component["freshness"], "unknown");
    assert_eq!(
        navigation_component["deep_validation"],
        "project doctor / project check"
    );

    std::fs::OpenOptions::new()
        .append(true)
        .open(directory.join("generated/fixture.ir/manifest.json"))
        .unwrap()
        .write_all(b" ")
        .unwrap();
    let stale = blobray()
        .current_dir(repository_root())
        .args(["project", "status", "--project"])
        .arg(&manifest)
        .args(["--format", "json", "--color", "never"])
        .output()
        .expect("run project status with stale navigation");
    assert!(stale.status.success());
    let stale: serde_json::Value = serde_json::from_slice(&stale.stdout).unwrap();
    let navigation_component = stale["phases"]["analysis"]["components"]
        .as_array()
        .unwrap()
        .iter()
        .find(|component| component["name"] == "navigation_index")
        .expect("stale navigation_index component");
    assert_eq!(navigation_component["status"], "ready");
    assert_eq!(navigation_component["validation_depth"], "shallow");
    assert_eq!(navigation_component["freshness"], "unknown");
    assert_eq!(
        navigation_component["deep_validation"],
        "project doctor / project check"
    );

    let strict = blobray()
        .current_dir(repository_root())
        .args(["project", "analyze", "--check", "--project"])
        .arg(&manifest)
        .args(["--format", "json", "--color", "never"])
        .output()
        .expect("strictly reproduce project analysis");
    assert!(!strict.status.success());
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn project_publication_json_is_one_typed_report() {
    let (directory, manifest) = init_temporary_project("publication-report");
    std::fs::OpenOptions::new()
        .append(true)
        .open(&manifest)
        .unwrap()
        .write_all(
            b"\n[review]\noutput = \"generated/findings/review-scopes.json\"\npublication-scopes = [\"publication\"]\n\n[[review.scopes]]\nid = \"publication\"\nprotocols = [\"shared\"]\nprofiles = [\"vendor\"]\nroots = [\"vendor:fixture_entry\"]\ninclude-reachable = true\n",
        )
        .unwrap();
    let review_output = directory.join("generated/findings/review-scopes.json");
    std::fs::create_dir_all(review_output.parent().unwrap()).unwrap();
    std::fs::write(
        review_output,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 12,
            "command": "project review scopes",
            "project": "publication-report",
            "scopes": [{
                "id": "publication",
                "protocols": ["shared"],
                "publication": true,
                "analysis_inventory_complete": true,
                "profiles": ["vendor"],
                "roots": 1,
                "functions": 0,
                "replacement_functions": 0,
                "replacement_function_keys": [],
                "transaction_functions": 0,
                "transaction_keys": [],
                "transactions": [],
                "function_identities": [],
                "function_keys": [],
                "complete_functions": 0,
                "mmio_registers": 0,
                "linked_mmio_registers": 0,
                "static_mmio_registers": 0,
                "mmio": [],
                "table_calls": 0,
                "context_fields": 0,
                "memory_fields": 0,
                "decode_blockers": 0,
                "decode_blocker_functions": 0,
                "direct_blockers": 0,
                "call_graph_blockers": 0,
                "reference_blockers": 0,
                "unresolved_calls": 0,
                "review_queue": []
            }]
        }))
        .unwrap(),
    )
    .unwrap();
    let written = blobray()
        .current_dir(repository_root())
        .args(["project", "publish", "--project"])
        .arg(&manifest)
        .args(["--format", "json", "--color", "never"])
        .output()
        .expect("publish temporary project");
    assert!(
        written.status.success(),
        "initial publication stderr: {}",
        String::from_utf8_lossy(&written.stderr)
    );
    let output = blobray()
        .current_dir(repository_root())
        .args(["project", "publish", "--check", "--project"])
        .arg(&manifest)
        .args(["--format", "json", "--color", "never"])
        .output()
        .expect("check temporary project publication");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("publication stdout must be valid JSON");
    assert_eq!(document["schema"], 2);
    assert_eq!(document["command"], "project publish");
    assert_eq!(document["mode"], "check");
    assert_eq!(document["status"], "ok");
    let measured_total = document["stages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|stage| stage.get("duration_ms").and_then(serde_json::Value::as_u64))
        .sum::<u64>();
    assert_eq!(document["duration_ms"], measured_total);
    let stages = document["stages"].as_array().unwrap();
    assert_eq!(stages.len(), 5);
    assert!(stages.iter().any(|stage| {
        stage["name"] == "pac-api-publication"
            && stage["status"] == "verified"
            && stage["duration_ms"].is_number()
    }));
    let human = blobray()
        .current_dir(repository_root())
        .args(["project", "publish", "--check", "--project"])
        .arg(&manifest)
        .args(["--color", "never"])
        .output()
        .expect("render publication timing for humans");
    assert!(human.status.success());
    let human = String::from_utf8_lossy(&human.stdout);
    assert!(human.contains("Duration:"), "stdout: {human}");
    assert!(human.contains(" ms"), "stdout: {human}");
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn tooling_assets_come_from_the_canonical_cli_without_a_project() {
    let root = std::env::temp_dir().join(format!("blobray-tooling-assets-{}", std::process::id()));
    if root.exists() {
        std::fs::remove_dir_all(&root).unwrap();
    }
    std::fs::create_dir_all(&root).unwrap();
    let completion = root.join("blobray.bash");
    let manpage = root.join("blobray.1");

    for (arguments, expected_kind, output_path) in [
        (
            vec!["tooling", "completions", "bash", "--output"],
            "shell-completion",
            &completion,
        ),
        (vec!["tooling", "manpage", "--output"], "manpage", &manpage),
    ] {
        let mut command = blobray();
        command
            .current_dir(&root)
            .args(arguments)
            .arg(output_path)
            .args(["--format", "json", "--color", "never"]);
        let output = command.output().expect("generate tooling asset");
        assert!(
            output.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let document: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(document["kind"], expected_kind);
        assert_eq!(document["status"], "written");
        assert!(output_path.metadata().unwrap().len() > 100);
    }

    let completion_text = std::fs::read_to_string(&completion).unwrap();
    assert!(completion_text.contains("blobray"));
    let manpage_text = std::fs::read_to_string(&manpage).unwrap();
    assert!(manpage_text.contains(".TH blobray"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn revision_diff_is_a_typed_project_workflow() {
    let (directory, manifest) = init_temporary_project("revision-diff");
    let features = ["mmio:0x1000/32"];
    let fingerprint = {
        let mut hash = Sha256::new();
        hash.update(b"blobray/revision-feature/v1\0");
        hash.update(serde_json::to_vec(&features).unwrap());
        format!("{:x}", hash.finalize())
    };
    let revision = |name: &str, function: &str| {
        serde_json::json!({
            "schema_version": 4,
            "command": "revision snapshot",
            "name": name,
            "project": "revision-diff",
            "artifact_scope": "vendor-inputs",
            "artifacts": [],
            "applicability": {},
            "functions": [{
                "id": function,
                "source": "vendor",
                "member": null,
                "symbol": function,
                "profiles": ["all"],
                "fingerprint": fingerprint.clone(),
                "features": features,
                "completeness": {
                    "body": true,
                    "call_targets": true,
                    "transitive_effects": true,
                    "executable": true
                },
                "blocker_roots": []
            }],
            "registers": [],
            "interfaces": [],
            "assertions": [],
            "vendor_bugs": [],
            "bindings": []
        })
    };
    let old = directory.join("old.json");
    let new = directory.join("new.json");
    std::fs::write(&old, revision("old", "vendor:old").to_string()).unwrap();
    std::fs::write(&new, revision("new", "vendor:new").to_string()).unwrap();

    let output = blobray()
        .current_dir(repository_root())
        .args(["project", "revision", "diff"])
        .arg(&old)
        .arg(&new)
        .args(["--project"])
        .arg(&manifest)
        .args(["--format", "json", "--color", "never"])
        .output()
        .expect("diff revision snapshots");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["schema_version"], 2);
    assert_eq!(document["command"], "revision diff");
    assert_eq!(document["summary"]["moved"], 1);
    assert_eq!(
        document["functions"]["remapped"][0],
        serde_json::json!({"before":"vendor:old","after":"vendor:new"})
    );
    assert_eq!(
        document["invalidated_research"][0]["area"],
        "evidence-locations"
    );
    assert_eq!(document["changes"][0]["classification"], "moved");

    let human = blobray()
        .current_dir(repository_root())
        .args(["project", "revision", "diff"])
        .arg(&old)
        .arg(&new)
        .args(["--project"])
        .arg(&manifest)
        .args(["--details", "--color", "never"])
        .output()
        .expect("render revision diff");
    assert!(human.status.success());
    let human = String::from_utf8(human.stdout).unwrap();
    for heading in [
        "Function delta",
        "Research invalidation",
        "Invalidation details",
        "Affected functions",
    ] {
        assert!(human.contains(heading), "missing {heading:?}: {human}");
    }
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn revision_snapshot_creates_a_durable_immutable_ledger() {
    let directory = std::env::temp_dir().join(format!(
        "blobray-cli-contract-revision-ledger-{}",
        std::process::id()
    ));
    if directory.exists() {
        std::fs::remove_dir_all(&directory).unwrap();
    }
    std::fs::create_dir_all(&directory).unwrap();
    let target = repository_root().join("tools/blobray/tests/fixtures/generic-project/target.toml");
    let manifest = directory.join("vendor-project.toml");
    std::fs::write(
        &manifest,
        format!(
            "schema = 4\nid = \"revision-ledger\"\ntarget-spec = {:?}\n\n[[analysis.ir]]\nid = \"fixture\"\nsources = [\"fixture\"]\nroots = \"all\"\ninclude-reachable = true\nentry-contract = \"none\"\noutput = \"generated/fixture.ir\"\n",
            target.display().to_string()
        ),
    )
    .unwrap();
    let artifact = directory.join("vendor.o");
    write_rv32_symbol_fixture(&artifact);
    let inputs = blobray()
        .current_dir(repository_root())
        .args(["project", "inputs", "init", "--project"])
        .arg(&manifest)
        .arg("--bind")
        .arg(format!("source-artifact:fixture={}", artifact.display()))
        .args(["--format", "json", "--color", "never"])
        .output()
        .expect("initialize revision fixture inputs");
    assert!(
        inputs.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&inputs.stderr)
    );
    let analyzed = run_project_command(&manifest, &["project", "analyze"]);
    assert!(
        analyzed.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&analyzed.stderr)
    );

    // Exercise the normal repository workflow where --project is relative to
    // the current directory. The implicit snapshot destination must not add
    // the manifest parent twice.
    let output = blobray()
        .current_dir(&directory)
        .args([
            "project",
            "revision",
            "snapshot",
            "vendor-1",
            "--project",
            "vendor-project.toml",
            "--format",
            "json",
            "--color",
            "never",
            "--progress",
            "never",
        ])
        .output()
        .expect("snapshot temporary project through a relative manifest");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["command"], "revision snapshot");
    assert_eq!(document["status"], "written");
    assert_eq!(document["artifact_bindings_verified"], 1);
    assert!(directory.join("revisions/ledger.toml").is_file());
    assert!(
        directory
            .join("revisions/snapshots/vendor-1.json.gz")
            .is_file()
    );
    let ledger = std::fs::read_to_string(directory.join("revisions/ledger.toml")).unwrap();
    assert!(ledger.contains("schema = 4"));
    assert!(ledger.contains("baseline = \"vendor-1\""));
    assert!(ledger.contains("current = \"vendor-1\""));
    assert!(ledger.contains("snapshot-sha256"));
    assert!(!ledger.contains("disassembly"));

    let checked = run_project_command(
        &manifest,
        &["project", "revision", "snapshot", "vendor-1", "--check"],
    );
    assert!(
        checked.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&checked.stderr)
    );
    let ledger_before_live_diff = std::fs::read(directory.join("revisions/ledger.toml")).unwrap();
    let live_diff = run_project_command(
        &manifest,
        &["project", "revision", "diff", "vendor-1", "@live"],
    );
    assert!(
        live_diff.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&live_diff.stderr)
    );
    let live_diff: serde_json::Value = serde_json::from_slice(&live_diff.stdout).unwrap();
    assert_eq!(live_diff["schema_version"], 2);
    assert_eq!(live_diff["from"], "vendor-1");
    assert_eq!(live_diff["to"], "@live");
    assert_eq!(live_diff["artifacts_changed"], false);
    assert!(live_diff["changes"].as_array().unwrap().is_empty());
    assert_eq!(
        std::fs::read(directory.join("revisions/ledger.toml")).unwrap(),
        ledger_before_live_diff,
        "a live diff must not advance or rewrite the revision ledger"
    );
    let prepared = run_project_command(&manifest, &["project", "revision", "prepare-update"]);
    assert!(
        prepared.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&prepared.stderr)
    );
    let prepared: serde_json::Value = serde_json::from_slice(&prepared.stdout).unwrap();
    assert_eq!(prepared["status"], "prepared");
    assert_eq!(prepared["artifact_bindings_verified"], 1);
    let status = run_project_command(&manifest, &["project", "status"]);
    let status_document: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(
        status_document["phases"]["revision-workflow"]["status"],
        "ready"
    );
    std::fs::remove_dir_all(directory).unwrap();
}
