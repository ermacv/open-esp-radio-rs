use super::*;
use serde_json::json;

fn artifact(path: &str, test: bool) -> serde_json::Value {
    json!({
        "reason": "compiler-artifact", "package_id": "path+file:///phy#0.1.0",
        "manifest_path": "/phy/Cargo.toml",
        "target": {"kind": ["lib"], "crate_types": ["lib"], "name": "open_esp_radio_esp32s31_phy",
            "src_path": "/phy/src/lib.rs", "edition": "2024", "doc": true, "doctest": true, "test": true},
        "profile": {"opt_level": "3", "debuginfo": 0, "debug_assertions": false, "overflow_checks": false, "test": test},
        "features": [], "filenames": [path], "executable": null, "fresh": true
    })
}

#[test]
fn selects_actual_library_output_and_ignores_test_artifacts() {
    let messages = format!(
        "{}\n{}\n",
        artifact("/arbitrary target/PHY.rlib", false),
        artifact("/test/PHY.rlib", true)
    );
    assert_eq!(
        phy_artifact(messages.as_bytes()).unwrap(),
        PathBuf::from("/arbitrary target/PHY.rlib")
    );
}

#[test]
fn missing_or_ambiguous_library_output_fails() {
    assert!(phy_artifact(b"{\"reason\":\"build-finished\",\"success\":true}\n").is_err());
    let messages = format!(
        "{}\n{}\n",
        artifact("/one.rlib", false),
        artifact("/two.rlib", false)
    );
    assert!(phy_artifact(messages.as_bytes()).is_err());
}

#[test]
fn image_audit_uses_the_reported_existing_performance_elf() {
    let temporary = tempfile::tempdir().unwrap();
    let elf = temporary.path().join("actual image.elf");
    fs::write(&elf, b"fixture").unwrap();
    let mut report = json!({"image_class": "performance", "runtime_elf": elf});
    assert_eq!(
        runtime_artifact(&serde_json::to_vec(&report).unwrap()).unwrap(),
        elf
    );
    report["image_class"] = json!("correctness");
    assert!(runtime_artifact(&serde_json::to_vec(&report).unwrap()).is_err());
    report["image_class"] = json!("performance");
    fs::remove_file(&elf).unwrap();
    assert!(runtime_artifact(&serde_json::to_vec(&report).unwrap()).is_err());
}
