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
