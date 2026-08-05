use std::path::Path;

use super::*;

fn repository_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("validator must remain under tools/vendor-code-validator")
}

#[test]
fn checked_esp32s31_model_preserves_expanded_register_identities() {
    let root = repository_root();
    let model = RegisterModel::load(
        &root.join("verification/vendor/targets/esp32s31/registers/device.toml"),
    )
    .unwrap();
    let (output, summary) = model.render_svd().unwrap();
    assert_eq!(summary.peripherals, 73);
    assert_eq!(summary.registers, 1099);
    assert_eq!(summary.fields, 2464);
    assert!(!output.contains("SOURCE["));
    assert!(!output.contains("CONFIDENCE["));
    assert!(!output.contains("openEspRadio"));

    let generated = std::env::temp_dir().join(format!(
        "vendor-validator-register-model-{}.svd",
        std::process::id()
    ));
    std::fs::write(&generated, output).unwrap();
    let original = crate::MmioRegisterMap::load(&root.join("svd/esp32s31-radio.svd")).unwrap();
    let roundtrip = crate::MmioRegisterMap::load(&generated).unwrap();
    std::fs::remove_file(generated).unwrap();
    assert_eq!(roundtrip.registers, original.registers);
}

#[test]
fn stale_review_entity_is_rejected() {
    let directory = std::env::temp_dir().join(format!(
        "vendor-validator-stale-register-review-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(directory.join("peripherals")).unwrap();
    std::fs::write(
        directory.join("device.toml"),
        r#"schema = 2
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
