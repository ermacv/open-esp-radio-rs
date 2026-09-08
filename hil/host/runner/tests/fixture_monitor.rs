//! Opt-in acceptance test of the installed Linux/OpenWrt fixture, without a DUT.
#![cfg(target_os = "linux")]

use oer_process::CommandExt as _;
use std::{fs, io::Write, path::Path, process::Command};

#[test]
#[ignore = "requires configured OpenWrt and installed Linux helper; never accesses ESP"]
fn fixture_check_exercises_monitor_capture_and_restores_without_a_serial_device() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .unwrap();
    let mut lab: toml::Value =
        toml::from_str(&fs::read_to_string(root.join("hil/local.toml")).unwrap()).unwrap();
    lab["device"]["serial"] = toml::Value::String("/nonexistent/oer-fixture-only-device".into());
    // NamedTempFile creates private storage for the existing lab credentials.
    let mut config = tempfile::NamedTempFile::new_in(root.join("hil")).unwrap();
    config
        .write_all(toml::to_string(&lab).unwrap().as_bytes())
        .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_open-esp-radio-hil-runner"))
        .arg("--lab-config")
        .arg(config.path())
        .args([
            "fixture",
            "check",
            "udp-bidirectional-ht40-rx-delivery-split",
        ])
        .supervised_output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["report"]["device_accessed"], false);
    assert_eq!(result["report"]["prepared"], true);
    assert_eq!(result["report"]["restored"], true);
    let artifacts = Path::new(result["artifacts"].as_str().unwrap());
    assert!(artifacts.join("fixture-monitor.json").is_file());
    assert!(artifacts.join("independent-air.pcapng").is_file());
}
