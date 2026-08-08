use std::{
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

fn run(arguments: &[&str]) -> Output {
    workbench()
        .current_dir(repository_root())
        .args(arguments)
        .output()
        .expect("run vendor-binary-workbench")
}

fn write_rv32_symbol_fixture(path: &Path) {
    let bytes = include_str!("fixtures/symbols-rv32.hex")
        .split_ascii_whitespace()
        .map(|octet| u8::from_str_radix(octet, 16).unwrap())
        .collect::<Vec<_>>();
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn project_status_json_is_pipe_safe_and_dependency_warnings_are_suppressed() {
    let output = run(&[
        "project",
        "status",
        "--project",
        "verification/vendor/targets/esp32s31/vendor-project.toml",
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
    assert_eq!(document["records"][0]["kind"], "project-status");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("Missing description for register"),
        "dependency warnings leaked to stderr: {stderr}"
    );
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
            "verification/vendor/targets/esp32s31/vendor-project.toml",
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
fn project_analyze_is_the_only_project_analysis_entry_point() {
    let help = run(&["project", "analyze", "--help"]);
    assert!(help.status.success());
    let help = String::from_utf8_lossy(&help.stdout);
    assert!(help.contains("--check"));
    assert!(help.contains("--deny-unreviewed"));

    for removed in ["build", "check"] {
        let output = run(&["project", removed]);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("unrecognized subcommand"));
    }
}

#[test]
fn project_analysis_emits_a_typed_summary_when_inputs_are_blocked() {
    let output = run(&[
        "project",
        "analyze",
        "--check",
        "--project",
        "verification/vendor/targets/esp32s31/vendor-project.toml",
        "--format",
        "json",
        "--color",
        "never",
    ]);
    assert_eq!(output.status.code(), Some(2));
    let document: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("analysis stdout must be valid JSON");
    let report = document["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|record| record["kind"] == "project-analysis")
        .expect("project-analysis record");
    assert_eq!(report["data"]["schema"], 1);
    assert_eq!(report["data"]["command"], "project analyze");
    assert_eq!(report["data"]["mode"], "check");
    assert_eq!(report["data"]["status"], "failed");
    assert!(report["data"]["blocked"].as_u64().unwrap() > 0);
    assert!(report["data"]["stages"].is_array());
    assert!(
        report["data"]["stages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|stage| { stage["name"] == "symbol-inventory" && stage["status"] == "blocked" })
    );
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
    let target = repository_root().join("verification/vendor/targets/esp32s31/target.spec");
    let manifest = directory.join("vendor-project.toml");
    std::fs::write(
        &manifest,
        format!(
            "schema = 1\nid = \"symbol-contract\"\ntarget-spec = {:?}\n\n[analysis.symbols]\noutput = \"generated/symbols.json\"\n",
            target.display().to_string()
        ),
    )
    .unwrap();
    let artifact = directory.join("vendor.o");
    write_rv32_symbol_fixture(&artifact);
    let run_spec = directory.join("local.run");
    std::fs::write(
        &run_spec,
        format!("schema 1\ninput artifact {}\n", artifact.display()),
    )
    .unwrap();

    for (check, expected_stage_status) in [(false, "written"), (true, "verified")] {
        let mut command = workbench();
        command
            .current_dir(repository_root())
            .args(["project", "analyze", "--project"])
            .arg(&manifest)
            .arg("--run-spec")
            .arg(&run_spec)
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
        let analysis = document["records"]
            .as_array()
            .unwrap()
            .iter()
            .find(|record| record["kind"] == "project-analysis")
            .expect("project-analysis record");
        assert_eq!(analysis["data"]["status"], "ok");
        let symbol_stage = analysis["data"]["stages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|stage| stage["name"] == "symbol-inventory")
            .expect("symbol-inventory stage");
        assert_eq!(symbol_stage["status"], expected_stage_status);
    }

    let report = directory.join("generated/symbols.json");
    let document: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report).unwrap()).unwrap();
    assert_eq!(document["schema_version"], 2);
    assert_eq!(document["command"], "symbols inventory");
    assert!(document["summary"]["symbol_facts"].as_u64().unwrap() > 0);

    let status = workbench()
        .current_dir(repository_root())
        .args(["project", "status", "--project"])
        .arg(&manifest)
        .args(["--format", "json", "--color", "never"])
        .output()
        .expect("run project status");
    assert!(status.status.success());
    let status: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    let status = status["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|record| record["kind"] == "project-status")
        .expect("project-status record");
    let symbol_component = status["data"]["phases"]["analysis"]["components"]
        .as_array()
        .unwrap()
        .iter()
        .find(|component| component["name"] == "symbol_inventory")
        .expect("symbol_inventory component");
    assert_eq!(symbol_component["status"], "ready");
    assert_eq!(symbol_component["exported_definitions"], 1);
    assert_eq!(symbol_component["undefined"], 1);
    std::fs::remove_dir_all(directory).unwrap();
}
