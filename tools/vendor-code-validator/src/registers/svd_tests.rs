use super::*;

#[test]
fn exported_svd_keeps_reviewed_fields_and_unreviewed_placeholders() {
    let workspace = RegisterWorkspace {
        facts: RegisterFacts {
            ranges: vec![FactRange {
                name: "radio".to_owned(),
                start: 0x1000,
                end: 0x2000,
            }],
            registers: vec![
                RegisterFact {
                    address: 0x1010,
                    width: 32,
                    catalog_name: "UNMAPPED".to_owned(),
                    reads: 1,
                    writes: 1,
                    read_functions: Default::default(),
                    write_functions: Default::default(),
                    write_patterns: vec![],
                    candidate_masks: vec![1],
                },
                RegisterFact {
                    address: 0x1020,
                    width: 32,
                    catalog_name: "UNMAPPED".to_owned(),
                    reads: 1,
                    writes: 0,
                    read_functions: Default::default(),
                    write_functions: Default::default(),
                    write_patterns: vec![],
                    candidate_masks: vec![],
                },
            ],
        },
        overlay: RegisterOverlayFile {
            device: DeviceOverlay {
                name: "RADIO_DEVICE".to_owned(),
                vendor: Some("Open & Radio".to_owned()),
                version: "0.1".to_owned(),
                description: "Reviewed <map>".to_owned(),
                address_unit_bits: 8,
                width: 32,
            },
            peripherals: vec![PeripheralOverlay {
                range: "radio".to_owned(),
                name: "RADIO".to_owned(),
                description: None,
            }],
            registers: vec![RegisterOverlay {
                address: 0x1010,
                width: 32,
                status: RegisterStatus::Reviewed,
                name: Some("CONTROL".to_owned()),
                description: None,
                access: Some("read-write".to_owned()),
                reset_value: None,
                reset_mask: None,
                fields: vec![FieldOverlay {
                    name: "ENABLE".to_owned(),
                    lsb: 0,
                    width: 1,
                    description: None,
                    access: None,
                    modified_write_values: None,
                    read_action: None,
                    origin: FieldOrigin::WritePattern,
                }],
            }],
        },
    };
    let path = std::env::temp_dir().join(format!(
        "vendor-validator-exported-registers-{}.svd",
        std::process::id()
    ));
    let (output, summary) = workspace.render_svd(SvdExportProfile::Audit).unwrap();
    std::fs::write(&path, &output).unwrap();
    let parsed = crate::MmioRegisterMap::load(&path).unwrap();
    std::fs::remove_file(path).unwrap();
    assert_eq!(summary.registers, 2);
    assert_eq!(parsed.registers.len(), 2);
    assert!(output.contains("schemaVersion=\"1.3\""));
    assert!(output.contains("<name>CONTROL</name>"));
    assert!(output.contains("<name>ENABLE</name>"));
    assert!(output.contains("<name>REG_00000020_W32</name>"));
    assert!(output.contains("Open &amp; Radio"));
}

#[test]
fn release_svd_contains_only_reviewed_hardware_metadata() {
    let workspace = RegisterWorkspace {
        facts: RegisterFacts {
            ranges: vec![FactRange {
                name: "radio".to_owned(),
                start: 0x1000,
                end: 0x2000,
            }],
            registers: vec![
                RegisterFact {
                    address: 0x1010,
                    width: 32,
                    catalog_name: "UNMAPPED".to_owned(),
                    reads: 7,
                    writes: 2,
                    read_functions: Default::default(),
                    write_functions: Default::default(),
                    write_patterns: vec![],
                    candidate_masks: vec![1],
                },
                RegisterFact {
                    address: 0x1020,
                    width: 32,
                    catalog_name: "UNMAPPED".to_owned(),
                    reads: 1,
                    writes: 0,
                    read_functions: Default::default(),
                    write_functions: Default::default(),
                    write_patterns: vec![],
                    candidate_masks: vec![],
                },
            ],
        },
        overlay: RegisterOverlayFile {
            device: DeviceOverlay {
                name: "RADIO_DEVICE".to_owned(),
                vendor: None,
                version: "0.1".to_owned(),
                description: "Radio register map".to_owned(),
                address_unit_bits: 8,
                width: 32,
            },
            peripherals: vec![PeripheralOverlay {
                range: "radio".to_owned(),
                name: "RADIO".to_owned(),
                description: None,
            }],
            registers: vec![RegisterOverlay {
                address: 0x1010,
                width: 32,
                status: RegisterStatus::Reviewed,
                name: Some("CONTROL".to_owned()),
                description: None,
                access: Some("read-write".to_owned()),
                reset_value: None,
                reset_mask: None,
                fields: vec![],
            }],
        },
    };

    let (output, summary) = workspace.render_svd(SvdExportProfile::Release).unwrap();
    assert_eq!(summary.registers, 1);
    assert!(output.contains("<name>CONTROL</name>"));
    assert!(!output.contains("REG_00000020_W32"));
    assert!(!output.contains("Unreviewed MMIO observation"));
    assert!(!output.contains("MMIO discovery range"));
    assert!(!output.contains("<description></description>"));
}
