use super::*;

#[test]
fn stale_review_entity_is_rejected() {
    let directory = std::env::temp_dir().join(format!(
        "blobray-stale-register-review-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(directory.join("peripherals")).unwrap();
    std::fs::write(
        directory.join("device.toml"),
        r#"schema = 3
chip = "test-chip"
address-space = "cpu"
fragments = ["peripherals/radio.toml"]

[device]
name = "TEST"
version = "0.1"
description = "Test"
address-unit-bits = 8
width = 32
"#,
    )
    .unwrap();
    std::fs::write(
        directory.join("peripherals/radio.toml"),
        r#"schema = 2

[[peripherals]]
name = "RADIO"
baseAddress = 0x1000

[[peripherals.registers]]
[peripherals.registers.register]
name = "CONTROL"
addressOffset = 0
size = 32

[[review]]
entity = "RADIO.REMOVED"
sources = ["MANUAL_REVIEW"]
"#,
    )
    .unwrap();

    let error = RegisterModel::load(&directory.join("device.toml")).unwrap_err();
    std::fs::remove_dir_all(directory).unwrap();
    assert!(error.to_string().contains("does not exist in the model"));
}
