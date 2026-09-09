//! The offline preview must remain usable without a lab or any fixture access.
#[test]
fn preview_writes_machine_json_to_stdout_without_loading_lab_configuration() {
    let directory = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_open-esp-radio-hil-runner"))
        .arg("--lab-config")
        .arg(directory.path().join("absent.toml"))
        .args(["fixture", "probe-plan"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let plan: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(plan.len(), 401);
    assert_eq!(plan[0]["offset_us"], 1_000_000);
    assert_eq!(plan[400]["offset_us"], 6_995_000);
}
