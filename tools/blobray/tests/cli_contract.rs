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
    assert_eq!(document["schema"], 7);
    assert_eq!(document["scope"], "blobray-pipeline");
    assert_eq!(document["command"], "project status");
    assert_eq!(document["validation"]["depth"], "shallow");
    assert_eq!(document["validation"]["freshness"], "unknown");
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
        "Freshness:  unknown unless a component states otherwise",
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
    assert_eq!(report["schema"], 2);
    assert_eq!(report["command"], "project doctor");
    assert_eq!(report["status"], "valid-with-warnings");
    assert_eq!(report["project"]["id"], "generic-rv32-fixture");
    assert_eq!(report["target"]["id"], "generic-rv32");
    assert!(report["capabilities"].as_array().unwrap().len() > 10);
    assert_eq!(report["ir_build"]["status"], "not-configured");
    assert_eq!(report["function_workspace"]["status"], "not-configured");
    assert_eq!(report["run_spec"]["status"], "not-configured");
    assert!(report["inputs"].is_array());
    assert!(report["errors"].is_u64());
    assert!(report["warnings"].is_u64());

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
    assert_eq!(report["schema"], 2);
    assert_eq!(report["project_id"], "generic-rv32-fixture");
    assert_eq!(report["workflow_state"], "blocked");
    assert!(report["required_missing"].is_u64());
    assert!(report["next_actions"].is_array());
    assert!(
        report["files"].as_array().unwrap().iter().any(|file| {
            file["role"] == "project-manifest" && file["ownership"] == "entrypoint"
        })
    );
}

#[test]
fn fresh_project_files_exposes_only_the_executable_bootstrap_frontier() {
    let (directory, manifest) = init_temporary_project("files-frontier");

    let machine = run_project_command(&manifest, &["project", "files"]);
    assert!(machine.status.success());
    let document: serde_json::Value = serde_json::from_slice(&machine.stdout).unwrap();
    assert_eq!(document["schema"], 2);
    assert_eq!(document["workflow_state"], "blocked");
    assert!(document["required_missing"].as_u64().unwrap() > 0);
    let actions = document["next_actions"].as_array().unwrap();
    assert_eq!(actions.len(), 1);
    let action = actions[0].as_str().unwrap();
    assert!(action.contains("project inputs init"));
    assert!(action.ends_with("--help"));
    assert!(!action.contains("project publish"));

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

    std::fs::write(
        directory.join("registers/peripherals/device.toml"),
        r#"schema = 2

[[peripherals]]
name = "DEVICE"
baseAddress = 0x40000000

[[peripherals.registers]]

[peripherals.registers.register]
name = "CONTROL"
description = "Synthetic read-modify-write control register."
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

    let mut project = std::fs::read_to_string(&manifest).unwrap();
    project.push_str(
        r#"
[review]
output = "generated/findings/review-scopes.json"
publication-scopes = ["fixture"]

[[review.scopes]]
id = "fixture"
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
        "schema = 3\nid = \"fixture\"\ntarget-spec = \"target.toml\"\n[[analysis.ir]]\nid = \"known\"\nroots = \"all\"\noutput = \"known.json\"\n[functions]\npack = \"functions.toml\"\nprofiles = [\"missing\"]\n",
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
        "schema = 3\nid = \"fixture\"\ntarget-spec = \"target.toml\"\nchip-pack = \"chip.toml\"\n",
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
        "schema = 3\nid = \"fixture\"\ntarget-spec = \"target.toml\"\nchip-pack = \"chip.toml\"\n",
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
    assert_eq!(persistent["schema_version"], 15);
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
        r#"schema = 3
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
    assert_eq!(document["schema_version"], 15);
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
    assert_eq!(document["schema"], 4);
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
            "schema = 3\nid = \"nothing-configured\"\ntarget-spec = {:?}\n",
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
    assert_eq!(document["schema"], 4);
    assert_eq!(document["command"], "project analyze");
    assert_eq!(document["status"], "nothing-configured");
    assert_eq!(document["not-configured"], 14);
    assert_eq!(document["written"], 0);
    assert_eq!(document["verified"], 0);
    assert_eq!(document["up-to-date"], 0);
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
            "schema = 3\nid = \"input-contract\"\ntarget-spec = {:?}\n\n[[analysis.ir]]\nid = \"fixture\"\nsources = [\"fixture\"]\nroots = \"all\"\noutput = \"generated/fixture.ir\"\n",
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
            "schema = 3\nid = \"read-only-analysis\"\ntarget-spec = {:?}\n\n[analysis.symbols]\noutput = \"generated/symbols.json\"\n\n[analysis.navigation]\noutput = \"generated/navigation.json\"\n\n[[analysis.ir]]\nid = \"fixture\"\nsources = [\"fixture\"]\nroots = \"all\"\ninclude-reachable = true\nentry-contract = \"none\"\noutput = \"generated/fixture.ir\"\n",
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
            "schema = 3\nid = \"symbol-contract\"\ntarget-spec = {:?}\n\n[analysis.symbols]\noutput = \"generated/symbols.json\"\n\n[analysis.navigation]\noutput = \"generated/navigation.json\"\n\n[[analysis.ir]]\nid = \"fixture\"\nsources = [\"fixture\"]\nroots = \"all\"\ninclude-reachable = true\nentry-contract = \"none\"\noutput = \"generated/fixture.ir\"\n\n[interfaces]\nfacts = \"generated/interfaces.json\"\n",
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
        assert_eq!(document["schema"], 4);
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
        "[interfaces]\nfacts = \"generated/interfaces.json\"\npack = \"interfaces/reviewed.toml\"\n\n[functions]\npack = \"functions/reviewed.toml\"\nprofiles = [\"fixture\"]\n",
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
            b"\n[review]\noutput = \"generated/findings/review-scopes.json\"\npublication-scopes = [\"publication\"]\n\n[[review.scopes]]\nid = \"publication\"\nprofiles = [\"vendor\"]\nroots = [\"vendor:fixture_entry\"]\ninclude-reachable = true\n",
        )
        .unwrap();
    let review_output = directory.join("generated/findings/review-scopes.json");
    std::fs::create_dir_all(review_output.parent().unwrap()).unwrap();
    std::fs::write(
        review_output,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 11,
            "command": "project review scopes",
            "project": "publication-report",
            "scopes": [{
                "id": "publication",
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
