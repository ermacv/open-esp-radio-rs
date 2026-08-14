use std::{path::Path, process::Command};

fn repository_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(5)
        .expect("ESP32-S31 host remains under verification/vendor/targets")
}

fn project() -> std::path::PathBuf {
    repository_root().join("verification/vendor/targets/esp32s31/vendor-project.toml")
}

fn workbench() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_vendor-binary-workbench"));
    command
        .current_dir(repository_root())
        .env_remove("RUST_LOG");
    command
}

#[test]
fn removed_provider_contracts_fail_before_reading_artifacts() {
    let output = workbench()
        .args(["advanced", "verify", "contract", "channel", "--project"])
        .arg(project())
        .args([
            "--vendor-artifact",
            "/missing/vendor-contract.elf",
            "--vendor-companion",
            "/missing/vendor-rom.elf",
            "--format",
            "json",
            "--color",
            "never",
        ])
        .output()
        .expect("run provider contract");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("workbench::addon::provider"));
    assert!(stderr.contains("add-on provider \"esp32s31-radio-verification-v1\" failed"));
    assert!(stderr.contains("has no self-verdict semantic contract \"channel\""));
    assert!(!stderr.contains("No such file or directory"));
    assert!(!stderr.contains("Usage:"));
}

#[test]
fn checked_register_publications_are_typed_reports() {
    for (arguments, expected_status) in [
        (vec!["registers", "validate"], "valid"),
        (vec!["registers", "export-svd", "--check"], "verified"),
        (vec!["registers", "generate-pac-raw", "--check"], "verified"),
        (
            vec!["registers", "generate-bindings", "--check"],
            "verified",
        ),
    ] {
        let output = workbench()
            .args(arguments)
            .arg("--project")
            .arg(project())
            .args(["--format", "json", "--color", "never"])
            .output()
            .expect("run register publication check");
        assert!(
            output.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let document: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(document["schema"], 1);
        assert_eq!(document["status"], expected_status);
    }
}

#[test]
fn checked_in_project_and_target_owned_review_packs_pass_doctor() {
    let output = workbench()
        .args(["project", "doctor", "--project"])
        .arg(project())
        .args([
            "--format",
            "json",
            "--color",
            "never",
            "--progress",
            "never",
        ])
        .output()
        .expect("validate the checked-in ESP32-S31 project");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["schema"], 2);
    assert_eq!(document["command"], "project doctor");
    assert!(matches!(
        document["status"].as_str(),
        Some("valid" | "valid-with-warnings")
    ));
    assert_eq!(document["project"]["id"], "esp32s31-radio-rev0");
    assert_eq!(document["target"]["id"], "esp32s31-rev0");
    assert!(
        document["capabilities"]
            .as_array()
            .is_some_and(|capabilities| capabilities.iter().all(|capability| {
                matches!(
                    capability["status"].as_str(),
                    Some("available" | "ready" | "valid")
                ) || (capability["name"] == "verification-report"
                    && capability["status"] == "incomplete")
            }))
    );
}
