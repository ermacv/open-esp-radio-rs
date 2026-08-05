use super::*;

fn facts() -> RegisterFacts {
    RegisterFacts {
        ranges: vec![FactRange {
            name: "radio".to_owned(),
            start: 0x1000,
            end: 0x2000,
        }],
        registers: vec![RegisterFact {
            address: 0x1010,
            width: 32,
            catalog_name: "UNMAPPED".to_owned(),
            reads: 1,
            writes: 1,
            candidate_masks: vec![3],
        }],
    }
}

#[test]
fn write_pattern_field_must_be_covered_by_observed_bits() {
    let path = std::env::temp_dir().join(format!(
        "vendor-validator-register-overlay-{}.toml",
        std::process::id()
    ));
    std::fs::write(
        &path,
        r#"
schema = 1
device-name = "RADIO_DEVICE"
[[registers]]
address = 0x1010
width = 32
name = "CONTROL"
[[registers.fields]]
name = "ENABLE"
lsb = 0
width = 2
origin = "write-pattern"
"#,
    )
    .unwrap();
    let overlay = RegisterOverlayFile::load(&path, &facts()).unwrap();
    std::fs::remove_file(path).unwrap();
    assert_eq!(overlay.registers[0].fields[0].name, "ENABLE");
}

#[test]
fn stale_observed_register_is_rejected() {
    let path = std::env::temp_dir().join(format!(
        "vendor-validator-stale-overlay-{}.toml",
        std::process::id()
    ));
    std::fs::write(
        &path,
        "schema=1\ndevice-name=\"RADIO\"\n[[registers]]\naddress=0x1020\nwidth=32\nname=\"STALE\"\n",
    )
    .unwrap();
    let error = RegisterOverlayFile::load(&path, &facts()).unwrap_err();
    std::fs::remove_file(path).unwrap();
    assert!(error.to_string().contains("stale"));
}
