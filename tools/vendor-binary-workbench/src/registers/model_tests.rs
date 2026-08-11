use std::path::Path;

use super::*;

fn repository_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workbench must remain under tools/vendor-binary-workbench")
}

#[test]
fn checked_esp32s31_model_preserves_expanded_register_identities() {
    let root = repository_root();
    let model = RegisterModel::load(
        &root.join("verification/vendor/targets/esp32s31/registers/device.toml"),
    )
    .unwrap();
    let (output, summary) = model.render_svd().unwrap();
    assert_eq!(summary.peripherals, 74);
    assert_eq!(summary.registers, 1136);
    assert_eq!(summary.fields, 2529);
    assert!(!output.contains("SOURCE["));
    assert!(!output.contains("CONFIDENCE["));
    assert!(!output.contains("openEspRadio"));
    let bindings = open_esp_radio_register_model::generate_pac_binding_index(
        &output,
        "open_esp_radio_esp32s31_pac_raw",
    )
    .unwrap();
    assert_eq!(
        bindings,
        std::fs::read_to_string(root.join("svd/esp32s31-radio.bindings.toml")).unwrap()
    );
    let api =
        PacApiPack::load(&root.join("verification/vendor/targets/esp32s31/registers/api.toml"))
            .unwrap();
    assert_eq!(api.operation_count(), 92);
    assert_eq!(api.domain_count(), 1);
    assert_eq!(api.source_ids().len(), 47);
    api.validate_against_svd(&output).unwrap();
    let helpers = api.render_rust(&output).unwrap();
    for module in [
        "interrupt_snapshot",
        "peripheral_ownership",
        "full_register_write",
        "fixed_register_write",
        "fixed_register_image",
        "register_image_write",
        "zero_based_field_write",
        "zero_register_write",
        "masked_register_modify",
        "device_access",
    ] {
        assert!(helpers.contains(&format!("pub mod {module}")));
    }
    let facade = api.render_facade_rust().unwrap();
    assert!(facade.contains("pub struct MacInterruptMask(u32);"));
    assert!(!facade.contains("from_bits"));
    let evidence_root = root.join("verification/vendor/targets/esp32s31/registers/evidence");
    let evidence = RegisterEvidenceSet::load_all(
        &[
            "policy.toml",
            "platform.toml",
            "vendor-rom.toml",
            "vendor-radio-libraries.toml",
            "vendor-libpp.toml",
            "vendor-net80211.toml",
            "hil-open.toml",
            "hil-vendor.toml",
            "migration.toml",
        ]
        .map(|name| evidence_root.join(name)),
    )
    .unwrap();
    assert_eq!(evidence.sources.len(), 216);
    assert_eq!(evidence.ranges.len(), 14);
    assert_eq!(evidence.confidence_levels.len(), 6);
    evidence
        .validate_references(
            "register model review",
            model
                .review()
                .iter()
                .flat_map(|annotation| annotation.sources.iter().map(String::as_str)),
        )
        .unwrap();
    evidence
        .validate_confidence_levels(
            "register model review",
            model
                .review()
                .iter()
                .filter_map(|annotation| annotation.confidence.as_deref()),
        )
        .unwrap();
    evidence
        .validate_references("PAC API pack", api.source_ids())
        .unwrap();

    let generated = std::env::temp_dir().join(format!(
        "vendor-workbench-register-model-{}.svd",
        std::process::id()
    ));
    std::fs::write(&generated, output).unwrap();
    let original = crate::MmioMap::load(&root.join("svd/esp32s31-radio.svd")).unwrap();
    let roundtrip = crate::MmioMap::load(&generated).unwrap();
    std::fs::remove_file(generated).unwrap();
    assert_eq!(roundtrip.registers, original.registers);
}

#[test]
fn stale_review_entity_is_rejected() {
    let directory = std::env::temp_dir().join(format!(
        "vendor-workbench-stale-register-review-{}",
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
