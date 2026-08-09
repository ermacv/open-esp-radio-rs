use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn workbench() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_vendor-binary-workbench"));
    command.env_remove("RUST_LOG");
    command
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root")
}

const GENERIC_PROJECT: &str =
    "tools/vendor-binary-workbench/tests/fixtures/generic-project/vendor-project.toml";

fn run(arguments: &[&str]) -> Output {
    workbench()
        .current_dir(repository_root())
        .args(arguments)
        .output()
        .expect("run vendor-binary-workbench")
}

fn init_temporary_project(label: &str) -> (PathBuf, PathBuf) {
    let directory = std::env::temp_dir().join(format!(
        "vendor-workbench-cli-contract-{label}-{}",
        std::process::id()
    ));
    if directory.exists() {
        std::fs::remove_dir_all(&directory).unwrap();
    }
    let output = workbench()
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

fn write_rv32_symbol_fixture(path: &Path) {
    let bytes = include_str!("fixtures/symbols-rv32.hex")
        .split_ascii_whitespace()
        .map(|octet| u8::from_str_radix(octet, 16).unwrap())
        .collect::<Vec<_>>();
    std::fs::write(path, bytes).unwrap();
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
    assert_eq!(document["schema"], 1);
    assert_eq!(document["command"], "project status");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("Missing description for register"),
        "dependency warnings leaked to stderr: {stderr}"
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
    assert!(jsonl.status.success());
    assert_eq!(String::from_utf8_lossy(&jsonl.stdout).lines().count(), 1);
    let report: serde_json::Value = serde_json::from_slice(&jsonl.stdout).unwrap();
    assert_eq!(report["command"], "project doctor");
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
fn runtime_errors_do_not_emit_usage_or_an_empty_json_result() {
    let output = run(&[
        "project",
        "init",
        "--directory",
        "/tmp/vendor-workbench-invalid-cli-contract",
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
        "vendor-workbench-project-diagnostic-contract-{}",
        std::process::id()
    ));
    if directory.exists() {
        std::fs::remove_dir_all(&directory).unwrap();
    }
    std::fs::create_dir_all(&directory).unwrap();
    let manifest = directory.join("vendor-project.toml");
    std::fs::write(
        &manifest,
        "schema = 1\nid = \"fixture\"\ntarget-spec = \"target.toml\"\n[[analysis.ir]]\nid = \"known\"\nroots = \"all\"\noutput = \"known.json\"\n[functions]\npack = \"functions.toml\"\nprofiles = [\"missing\"]\n",
    )
    .unwrap();

    let output = workbench()
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
        "vendor-workbench-composed-diagnostic-contract-{}",
        std::process::id()
    ));
    if directory.exists() {
        std::fs::remove_dir_all(&directory).unwrap();
    }
    std::fs::create_dir_all(&directory).unwrap();
    let project = directory.join("vendor-project.toml");
    std::fs::write(
        directory.join("target.toml"),
        "schema = 1\nid = \"fixture\"\narchitecture = \"riscv32\"\ncalling-convention = \"riscv-ilp32\"\nendianness = \"little\"\npointer-width = 32\nrust-target = \"riscv32imac-unknown-none-elf\"\n",
    )
    .unwrap();
    std::fs::write(
        directory.join("platform.toml"),
        "schema = 1\nid = \"fixture\"\narchitecture = \"riscv32\"\ncalling-convention = \"riscv-ilp32\"\nharness = [\"bad\"]\n",
    )
    .unwrap();
    std::fs::write(
        &project,
        "schema = 1\nid = \"fixture\"\ntarget-spec = \"target.toml\"\nplatform-pack = \"platform.toml\"\n",
    )
    .unwrap();

    let platform_output = workbench()
        .current_dir(repository_root())
        .args(["project", "status", "--project"])
        .arg(&project)
        .args(["--format", "json", "--color", "never"])
        .output()
        .expect("diagnose invalid platform pack");
    assert!(!platform_output.status.success());
    assert!(platform_output.stdout.is_empty());
    let platform_stderr = String::from_utf8_lossy(&platform_output.stderr);
    assert!(platform_stderr.contains("harness = [\"bad\"]"));
    assert!(platform_stderr.contains("platform.toml"));
    assert!(!platform_stderr.contains("Usage:"));

    std::fs::write(
        directory.join("memory.toml"),
        "schema = 1\ndefault-address-space = \"cpu\"\n[[address-spaces]]\nid = \"cpu\"\naddress-width = 32\nendianness = \"little\"\n[[regions]]\nname = \"radio\"\naddress-space = \"cpu\"\nkind = \"peripheral\"\nstart = 0x1000\nend-exclusive = 0x2000\n",
    )
    .unwrap();
    std::fs::write(
        &project,
        "schema = 1\nid = \"fixture\"\ntarget-spec = \"target.toml\"\nmemory-map = \"memory.toml\"\n",
    )
    .unwrap();
    let memory_output = workbench()
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
fn semantic_contract_commands_keep_failed_qualifications_off_stdout() {
    for contract in ["channel", "rf-init"] {
        let output = run(&[
            "verify",
            "contract",
            contract,
            "--project",
            "verification/vendor/targets/esp32s31/vendor-project.toml",
            "--vendor-artifact",
            "/missing/vendor-contract.elf",
            "--vendor-companion",
            "/missing/vendor-rom.elf",
            "--format",
            "json",
            "--color",
            "never",
        ]);
        assert!(
            !output.status.success(),
            "{contract} unexpectedly succeeded"
        );
        assert!(
            output.stdout.is_empty(),
            "{contract} leaked qualification output: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr.contains("ORACLE\t"),
            "domain output leaked: {stderr}"
        );
        assert!(
            !stderr.contains("Usage:"),
            "runtime error printed usage: {stderr}"
        );
    }
}

#[test]
fn malformed_composite_values_are_rejected_by_the_leaf_clap_grammar() {
    let output = run(&[
        "project",
        "init",
        "--directory",
        "/tmp/vendor-workbench-invalid-value-contract",
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
    let output = workbench()
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
        .expect("run quiet vendor-binary-workbench");
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
        "vendor-workbench-verify-source-contract-{}.o",
        std::process::id()
    ));
    write_rv32_symbol_fixture(&artifact);
    let output = workbench()
        .current_dir(repository_root())
        .args([
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
    let artifact = std::env::temp_dir().join(format!(
        "vendor-workbench-verify-inventory-contract-{suffix}.o"
    ));
    let report_path = std::env::temp_dir().join(format!(
        "vendor-workbench-verify-inventory-contract-{suffix}.json"
    ));
    write_rv32_symbol_fixture(&artifact);
    let output = workbench()
        .current_dir(repository_root())
        .args([
            "verify",
            "inventory",
            "--project",
            GENERIC_PROJECT,
            "--source-artifact",
        ])
        .arg(format!("fixture={}", artifact.display()))
        .args(["--source-prefix", "fixture=fixture_", "--rust-artifact"])
        .arg(&artifact)
        .args([
            "--rust-prefix",
            "fixture_",
            "--no-profiles",
            "--no-dispositions",
            "--no-evidence-baseline",
            "--json-report",
        ])
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
    assert_eq!(persistent["schema_version"], 4);
    assert_eq!(persistent["command"], "verify inventory");
    assert_eq!(persistent["sources"][0]["functions"][0]["status"], "match");
    std::fs::remove_file(artifact).unwrap();
    std::fs::remove_file(report_path).unwrap();
}

#[test]
fn direct_target_audit_json_is_one_typed_report() {
    let artifact = std::env::temp_dir().join(format!(
        "vendor-workbench-direct-target-audit-contract-{}.o",
        std::process::id()
    ));
    write_rv32_symbol_fixture(&artifact);
    let mut image = std::fs::read(&artifact).unwrap();
    image[16..18].copy_from_slice(&2_u16.to_le_bytes());
    std::fs::write(&artifact, image).unwrap();
    let output = workbench()
        .current_dir(repository_root())
        .args([
            "image",
            "audit-targets",
            "--project",
            "verification/vendor/targets/esp32s31/vendor-project.toml",
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
fn project_analyze_is_the_only_project_analysis_entry_point() {
    let help = run(&["project", "analyze", "--help"]);
    assert!(help.status.success());
    let help = String::from_utf8_lossy(&help.stdout);
    assert!(help.contains("--check"));
    assert!(help.contains("--deny-unreviewed"));
    assert!(help.contains("--progress"));

    for removed in ["build", "check"] {
        let output = run(&["project", removed]);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("unrecognized subcommand"));
    }
}

#[test]
fn project_analysis_emits_a_typed_summary_when_inputs_are_blocked() {
    let (directory, manifest) = init_temporary_project("blocked-analysis");
    let output = workbench()
        .current_dir(repository_root())
        .args(["project", "analyze", "--check", "--project"])
        .arg(&manifest)
        .args(["--format", "json", "--color", "never"])
        .output()
        .expect("run blocked project analysis");
    assert_eq!(output.status.code(), Some(2));
    let document: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("analysis stdout must be valid JSON");
    assert_eq!(document["schema"], 1);
    assert_eq!(document["command"], "project analyze");
    assert_eq!(document["mode"], "check");
    assert_eq!(document["status"], "failed");
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
fn project_inputs_validate_elf_and_archive_roles_before_writing() {
    let directory = std::env::temp_dir().join(format!(
        "vendor-workbench-project-inputs-contract-{}",
        std::process::id()
    ));
    if directory.exists() {
        std::fs::remove_dir_all(&directory).unwrap();
    }
    std::fs::create_dir_all(&directory).unwrap();
    let target = repository_root().join("verification/vendor/targets/esp32s31/target.toml");
    let manifest = directory.join("vendor-project.toml");
    std::fs::write(
        &manifest,
        format!(
            "schema = 1\nid = \"input-contract\"\ntarget-spec = {:?}\n\n[[analysis.ir]]\nid = \"fixture\"\nsources = [\"fixture\"]\nroots = \"all\"\noutput = \"generated/fixture.ir.json\"\n",
            target.display().to_string()
        ),
    )
    .unwrap();
    let artifact = directory.join("vendor.o");
    let archive = directory.join("vendor.a");
    write_rv32_symbol_fixture(&artifact);
    write_rv32_archive_fixture(&archive);

    let output = workbench()
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

    let invalid = workbench()
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
fn project_symbol_inventory_writes_and_checks_its_manifest_owned_report() {
    let directory = std::env::temp_dir().join(format!(
        "vendor-workbench-symbol-project-contract-{}",
        std::process::id()
    ));
    if directory.exists() {
        std::fs::remove_dir_all(&directory).unwrap();
    }
    std::fs::create_dir_all(&directory).unwrap();
    let target = repository_root().join("verification/vendor/targets/esp32s31/target.toml");
    let manifest = directory.join("vendor-project.toml");
    std::fs::write(
        &manifest,
        format!(
            "schema = 1\nid = \"symbol-contract\"\ntarget-spec = {:?}\n\n[analysis.symbols]\noutput = \"generated/symbols.json\"\n\n[analysis.navigation]\noutput = \"generated/navigation.json\"\n\n[[analysis.ir]]\nid = \"fixture\"\nsources = [\"fixture\"]\nroots = \"all\"\ninclude-reachable = true\nentry-contract = \"none\"\noutput = \"generated/fixture.ir.json\"\n\n[interfaces]\nfacts = \"generated/interfaces.json\"\n",
            target.display().to_string()
        ),
    )
    .unwrap();
    let artifact = directory.join("vendor.o");
    write_rv32_symbol_fixture(&artifact);
    let run_spec = directory.join("local.toml");
    let inputs = workbench()
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

    let checked_inputs = workbench()
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

    for (check, expected_stage_status) in [(false, "written"), (true, "verified")] {
        let mut command = workbench();
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
        assert_eq!(document["command"], "project analyze");
        assert_eq!(document["status"], "ok");
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
        let output = workbench()
            .current_dir(repository_root())
            .args(["code", command, "--project"])
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

    let checked = workbench()
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

    let ir_build = workbench()
        .current_dir(repository_root())
        .args(["ir", "build", "--check", "--project"])
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
        let output = workbench()
            .current_dir(repository_root())
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
        let output = workbench()
            .current_dir(repository_root())
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

    let denied = workbench()
        .current_dir(repository_root())
        .args(["functions", "validate", "--deny-unreviewed", "--project"])
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
    assert_eq!(document["schema_version"], 3);
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
    assert_eq!(navigation["schema_version"], 1);
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

    let status = workbench()
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
    assert_eq!(symbol_component["exported_definitions"], 1);
    assert_eq!(symbol_component["undefined"], 1);
    let navigation_component = status["phases"]["analysis"]["components"]
        .as_array()
        .unwrap()
        .iter()
        .find(|component| component["name"] == "navigation_index")
        .expect("navigation_index component");
    assert_eq!(navigation_component["status"], "ready");
    assert_eq!(navigation_component["linked_ir_functions"], 1);

    std::fs::OpenOptions::new()
        .append(true)
        .open(directory.join("generated/fixture.ir.json"))
        .unwrap()
        .write_all(b" ")
        .unwrap();
    let stale = workbench()
        .current_dir(repository_root())
        .args(["project", "status", "--project"])
        .arg(&manifest)
        .args(["--format", "json", "--color", "never"])
        .output()
        .expect("run project status with stale navigation");
    assert!(!stale.status.success());
    let stale: serde_json::Value = serde_json::from_slice(&stale.stdout).unwrap();
    let navigation_component = stale["phases"]["analysis"]["components"]
        .as_array()
        .unwrap()
        .iter()
        .find(|component| component["name"] == "navigation_index")
        .expect("stale navigation_index component");
    assert_eq!(navigation_component["status"], "invalid");
    assert!(
        navigation_component["diagnostic"]
            .as_str()
            .unwrap()
            .contains("changed since indexing")
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn project_publication_json_is_one_typed_report() {
    let (directory, manifest) = init_temporary_project("publication-report");
    let written = workbench()
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
    let output = workbench()
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
    assert_eq!(document["command"], "project publish");
    assert_eq!(document["mode"], "check");
    assert_eq!(document["status"], "ok");
    assert_eq!(document["stages"].as_array().unwrap().len(), 4);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn register_lifecycle_commands_emit_one_typed_report() {
    let project = "verification/vendor/targets/esp32s31/vendor-project.toml";
    for (arguments, kind, status) in [
        (
            vec!["registers", "validate", "--project", project],
            "register-workspace",
            "valid",
        ),
        (
            vec!["registers", "export-svd", "--check", "--project", project],
            "svd-publication",
            "verified",
        ),
        (
            vec!["registers", "generate-pac", "--check", "--project", project],
            "pac-publication",
            "verified",
        ),
        (
            vec![
                "registers",
                "generate-bindings",
                "--check",
                "--project",
                project,
            ],
            "binding-publication",
            "verified",
        ),
    ] {
        let mut command = workbench();
        command
            .current_dir(repository_root())
            .args(arguments)
            .args(["--format", "json", "--color", "never"]);
        let output = command.output().expect("run register lifecycle command");
        assert!(
            output.status.success(),
            "{kind} stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let document: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(document["schema"], 1);
        assert_eq!(document["status"], status);
    }
}

#[test]
fn human_tables_are_presentation_and_removed_tsv_is_rejected() {
    let project = "verification/vendor/targets/esp32s31/vendor-project.toml";
    let human = run(&[
        "registers",
        "validate",
        "--project",
        project,
        "--format",
        "human",
        "--color",
        "never",
    ]);
    assert!(
        human.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&human.stderr)
    );
    let human = String::from_utf8(human.stdout).unwrap();
    assert!(human.contains("Coverage:\n╭"));
    assert!(human.contains("Checks:\n╭"));
    assert!(!human.contains("REGISTER-WORKSPACE\t"));

    let removed = run(&[
        "registers",
        "validate",
        "--project",
        project,
        "--format",
        "tsv",
        "--color",
        "never",
    ]);
    assert!(!removed.status.success());
    assert!(removed.stdout.is_empty());
    assert!(String::from_utf8_lossy(&removed.stderr).contains("invalid value 'tsv'"));
}

#[test]
fn tooling_assets_come_from_the_canonical_cli_without_a_project() {
    let root = std::env::temp_dir().join(format!(
        "vendor-workbench-tooling-assets-{}",
        std::process::id()
    ));
    if root.exists() {
        std::fs::remove_dir_all(&root).unwrap();
    }
    std::fs::create_dir_all(&root).unwrap();
    let completion = root.join("vendor-binary-workbench.bash");
    let manpage = root.join("vendor-binary-workbench.1");

    for (arguments, expected_kind, output_path) in [
        (
            vec!["tooling", "completions", "bash", "--output"],
            "shell-completion",
            &completion,
        ),
        (vec!["tooling", "manpage", "--output"], "manpage", &manpage),
    ] {
        let mut command = workbench();
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
    assert!(completion_text.contains("vendor-binary-workbench"));
    let manpage_text = std::fs::read_to_string(&manpage).unwrap();
    assert!(manpage_text.contains(".TH vendor-binary-workbench"));
    std::fs::remove_dir_all(root).unwrap();
}
