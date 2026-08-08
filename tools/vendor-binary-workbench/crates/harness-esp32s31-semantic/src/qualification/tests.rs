use super::*;

fn calibration_execution_fixture() -> Option<(crate::MmioMap, execution::ExecutableImage)> {
    let linked = std::env::var_os("OPEN_ESP_RADIO_ESP32S31_LINKED_PHY_ELF")?;
    let rom = std::env::var_os("OPEN_ESP_RADIO_ESP32S31_ROM_ELF")?;
    let linked = std::path::PathBuf::from(linked);
    let rom = std::path::PathBuf::from(rom);
    if !linked.exists() || !rom.exists() {
        return None;
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(4)
        .expect("ESP32-S31 semantic harness remains under tools/vendor-binary-workbench/crates");
    let svd = crate::MmioMap::load_all(&[
        root.join("svd/esp32s31-radio.svd"),
        root.join("svd/esp32s31-platform-radio-deps.svd"),
    ])
    .unwrap();
    let mut image = execution::ExecutableImage::load(&linked).unwrap();
    image.add_companion(&rom).unwrap();
    Some((svd, image))
}

#[test]
fn inventory_symbol_identity_binds_only_the_selected_definition() {
    let definition = crate::artifact::ArtifactSymbolDefinition {
        member: Some("selected.o".to_owned()),
        name: "selected".to_owned(),
        address: 0x20,
        bytes: vec![1, 2, 3, 4],
        addresses_resolved: false,
        memory_regions: Vec::new(),
        relocations: vec![crate::artifact::SymbolRelocation {
            address: 0x22,
            kind: crate::artifact::RelocationKind::Call,
            symbol: "callee".to_owned(),
            addend: 4,
        }],
    };
    let same = definition.clone();
    let mut code_changed = definition.clone();
    code_changed.bytes[0] ^= 1;
    let mut relocation_changed = definition.clone();
    relocation_changed.relocations[0].symbol = "other_callee".to_owned();

    let identity = inventory_symbol_definition_sha256(&definition).unwrap();
    assert_eq!(identity, inventory_symbol_definition_sha256(&same).unwrap());
    assert_ne!(
        identity,
        inventory_symbol_definition_sha256(&code_changed).unwrap()
    );
    assert_ne!(
        identity,
        inventory_symbol_definition_sha256(&relocation_changed).unwrap()
    );
}

fn execution_result_with_timeline(
    timeline: Vec<execution::ExecutionTimelineEvent>,
) -> execution::ExecutionResult {
    execution::ExecutionResult {
        events: Vec::new(),
        event_producers: Vec::new(),
        timeline,
        return_value: 0,
        steps: 0,
        branches: std::collections::BTreeSet::new(),
        ordered_branches: Vec::new(),
        calls: std::collections::BTreeSet::new(),
        ordered_calls: Vec::new(),
        indirect_calls: std::collections::BTreeSet::new(),
        table_lifecycle: Vec::new(),
        table_lifecycle_complete: true,
        device_model_coverage: Vec::new(),
        memory_changes: Vec::new(),
        initial_memory: std::collections::BTreeMap::new(),
        persistent_memory: std::collections::BTreeMap::new(),
    }
}

#[test]
fn rust_channel_model_exposes_complete_action_order() {
    let events = rust_channel_events(11, 0).unwrap();
    assert_eq!(events.first(), Some(&ChannelEvent::SetAgc(false)));
    assert!(events.contains(&ChannelEvent::FrequencyReady { samples: 0 }));
    assert_eq!(
        events.last(),
        Some(&ChannelEvent::Complete {
            channel: 11,
            frequency_mhz: 2_462,
            cbw: 0,
            init_complete: false,
        })
    );
}

#[test]
fn rust_rf_init_model_preserves_typed_state_across_a_second_run() {
    let (first, state) = rust_rf_init_events(PhyColdState::new()).unwrap();
    let (second, _) = rust_rf_init_events(state).unwrap();

    assert_eq!(
        first.first(),
        Some(&rf_phase(
            RfInitPhase::ConfigureFeBbClock,
            RfInitPhaseParameters::None,
        ))
    );
    assert_eq!(first.last(), second.last());
    assert!(matches!(first.last(), Some(RfInitEvent::Complete(_))));
    assert!(first.contains(&rf_phase(
        RfInitPhase::InitializeRcCalibration,
        RfInitPhaseParameters::RcCalibrationPrestate {
            already_complete: false,
        },
    )));
    assert!(second.contains(&rf_phase(
        RfInitPhase::InitializeRcCalibration,
        RfInitPhaseParameters::RcCalibrationPrestate {
            already_complete: true,
        },
    )));
    assert!(first.contains(&rf_phase(
        RfInitPhase::ConfigureBbpllCalibration,
        RfInitPhaseParameters::Enabled(true),
    )));
    assert!(first.contains(&rf_phase(
        RfInitPhase::PostOpenI2cDelay,
        RfInitPhaseParameters::SymbolicValue(10),
    )));
    assert!(first.contains(&rf_phase(
        RfInitPhase::ConfigureI2cClockSelection,
        RfInitPhaseParameters::SymbolicValue(8),
    )));
}

#[test]
fn state_footprints_reject_unknown_offsets_and_access_directions() {
    let state_base = 0x1000;
    let unknown =
        execution_result_with_timeline(vec![execution::ExecutionTimelineEvent::RamRead {
            width: 8,
            address: state_base + 0x123,
            value: 0,
        }]);
    let error = vendor_rf_init_state_footprint(&unknown, state_base).unwrap_err();
    assert!(error.to_string().contains("reads=[0x123]"));

    let wrong_direction =
        execution_result_with_timeline(vec![execution::ExecutionTimelineEvent::RamWrite {
            width: 8,
            address: state_base + 0x007,
            value: 0,
        }]);
    let error = vendor_channel_state_footprint(&wrong_direction, state_base).unwrap_err();
    assert!(error.to_string().contains("writes=[0x007]"));
}

#[test]
fn vendor_rf_phase_rejects_mutated_direct_call_arguments() {
    let call = execution::OrderedCall {
        site: 0x1000,
        symbol: "ets_delay_us".to_owned(),
        arguments: [11, 0, 0, 0, 0, 0, 0, 0],
    };
    let error = vendor_rf_init_phase(&call).unwrap_err();
    assert!(error.to_string().contains("expected 0xa"));
}

#[test]
fn calibration_record_transfer_matches_independent_vendor_execution() {
    const RECORD_ADDRESS: u32 = 0x3fce_0000;
    const RECORD_LEN: usize =
        open_esp_radio_esp32s31_phy::phy_cold::PHY_COLD_CALIBRATION_RECORD_LEN;
    const PARAM_LEN: usize = open_esp_radio_esp32s31_phy::phy_cold::PHY_COLD_PARAMETER_LEN;
    const PAYLOAD_OFFSET: usize = RECORD_LEN - PARAM_LEN - core::mem::size_of::<u32>();

    let Some((svd, image)) = calibration_execution_fixture() else {
        eprintln!("private linked PHY fixtures are not installed; integration test skipped");
        return;
    };
    let phy_param = image
        .symbol_address("phy_param")
        .expect("linked vendor fixture must expose phy_param");
    let record_range = crate::execution_model::MemoryRange {
        start: RECORD_ADDRESS,
        length: RECORD_LEN as u32,
    };

    let parameter_image: [u8; PARAM_LEN] = core::array::from_fn(|index| {
        (index as u8)
            .wrapping_mul(73)
            .wrapping_add((index >> 3) as u8)
    });
    let mut record_bytes = [0_u8; RECORD_LEN];
    record_bytes[PAYLOAD_OFFSET..PAYLOAD_OFFSET + PARAM_LEN].copy_from_slice(&parameter_image);

    let mut recovery = execution::Scenario {
        arguments: vec![RECORD_ADDRESS],
        persistent_memory: vec![record_range],
        memory_ownership: vec![execution::MemoryOwnership {
            range: record_range,
            owner: execution::MemoryOwner::Cpu,
        }],
        max_steps: 100_000,
        ..execution::Scenario::default()
    };
    for (offset, byte) in record_bytes.iter().copied().enumerate() {
        recovery
            .memory_initial
            .insert(RECORD_ADDRESS + offset as u32, byte);
    }
    let vendor_recovery =
        execution::execute(&image, &svd, "phy_rf_cal_data_recovery_new", recovery).unwrap();
    let mut rust_recovery = open_esp_radio_esp32s31_phy::phy_cold::PhyColdState::new();
    rust_recovery.recover_from(
        &open_esp_radio_esp32s31_phy::phy_cold::PhyCalibrationRecord::from_bytes(record_bytes),
    );
    for (offset, rust_byte) in rust_recovery.parameter_image().iter().copied().enumerate() {
        assert_eq!(
            vendor_recovery
                .persistent_memory
                .get(&(phy_param + offset as u32)),
            Some(&rust_byte),
            "recovery diverged at phy_param+{offset:#05x}",
        );
    }

    let mut backup = execution::Scenario {
        arguments: vec![RECORD_ADDRESS],
        persistent_memory: vec![record_range],
        memory_ownership: vec![execution::MemoryOwnership {
            range: record_range,
            owner: execution::MemoryOwner::Cpu,
        }],
        max_steps: 100_000,
        ..execution::Scenario::default()
    };
    for (offset, byte) in parameter_image.iter().copied().enumerate() {
        backup
            .memory_initial
            .insert(phy_param + offset as u32, byte);
    }
    for offset in 0..RECORD_LEN {
        backup
            .memory_initial
            .insert(RECORD_ADDRESS + offset as u32, 0);
    }
    let vendor_backup =
        execution::execute(&image, &svd, "phy_rf_cal_data_backup_new", backup).unwrap();
    assert_eq!(vendor_backup.return_value, 0);
    let rust_state =
        open_esp_radio_esp32s31_phy::phy_cold::PhyColdState::from_parameter_image(parameter_image);
    let mut rust_record = open_esp_radio_esp32s31_phy::phy_cold::PhyCalibrationRecord::new();
    rust_state.backup_into(&mut rust_record);
    for (offset, rust_byte) in rust_record.bytes()[PAYLOAD_OFFSET..PAYLOAD_OFFSET + PARAM_LEN]
        .iter()
        .copied()
        .enumerate()
    {
        assert_eq!(
            vendor_backup
                .persistent_memory
                .get(&(RECORD_ADDRESS + PAYLOAD_OFFSET as u32 + offset as u32)),
            Some(&rust_byte),
            "backup diverged at record payload+{offset:#05x}",
        );
    }

    let rf_cal_version = 100_u32;
    let mac_sys0 = 0xa1b2_c3d4_u32;
    let mac_sys1 = 0xe5f6_0718_u32;
    let run_vendor_check = |mode: u32, bytes: &[u8; RECORD_LEN]| {
        let mut scenario = execution::Scenario {
            arguments: vec![mode, RECORD_ADDRESS, 0, rf_cal_version],
            persistent_memory: vec![record_range],
            memory_ownership: vec![execution::MemoryOwnership {
                range: record_range,
                owner: execution::MemoryOwner::Cpu,
            }],
            max_steps: 100_000,
            ..execution::Scenario::default()
        };
        scenario.mmio_initial.insert(0x2071_5050, mac_sys0);
        scenario.mmio_initial.insert(0x2071_5054, mac_sys1);
        for (offset, byte) in bytes.iter().copied().enumerate() {
            scenario
                .memory_initial
                .insert(RECORD_ADDRESS + offset as u32, byte);
        }
        execution::execute(&image, &svd, "phy_rfcal_data_check_new", scenario).unwrap()
    };

    let vendor_written = run_vendor_check(0, rust_record.bytes());
    assert_eq!(vendor_written.return_value, 0);
    let mut vendor_written_bytes = [0_u8; RECORD_LEN];
    for (offset, byte) in vendor_written_bytes.iter_mut().enumerate() {
        *byte = *vendor_written
            .persistent_memory
            .get(&(RECORD_ADDRESS + offset as u32))
            .unwrap_or(&rust_record.bytes()[offset]);
    }
    let mut rust_written = open_esp_radio_esp32s31_phy::phy_cold::PhyCalibrationRecord::from_bytes(
        rust_record.into_bytes(),
    );
    rust_written.refresh_header_and_checksum(rf_cal_version, mac_sys0, mac_sys1);
    assert_eq!(vendor_written_bytes, *rust_written.bytes());

    let vendor_valid = run_vendor_check(1, &vendor_written_bytes);
    assert_eq!(vendor_valid.return_value, 0);
    let mut rust_valid = open_esp_radio_esp32s31_phy::phy_cold::PhyCalibrationRecord::from_bytes(
        vendor_written_bytes,
    );
    assert!(rust_valid.checksum_matches(rf_cal_version, mac_sys0, mac_sys1));

    let mut corrupted = vendor_written_bytes;
    corrupted[PAYLOAD_OFFSET + 0x120] ^= 1;
    let vendor_invalid = run_vendor_check(1, &corrupted);
    assert_eq!(vendor_invalid.return_value, 1);
    let mut rust_invalid =
        open_esp_radio_esp32s31_phy::phy_cold::PhyCalibrationRecord::from_bytes(corrupted);
    assert!(!rust_invalid.checksum_matches(rf_cal_version, mac_sys0, mac_sys1));
}

#[test]
fn register_temperature_control_matches_independent_vendor_execution() {
    const PARAM_LEN: usize = open_esp_radio_esp32s31_phy::phy_cold::PHY_COLD_PARAMETER_LEN;

    let Some((svd, image)) = calibration_execution_fixture() else {
        eprintln!("private linked PHY fixtures are not installed; integration test skipped");
        return;
    };
    let phy_param = image
        .symbol_address("phy_param")
        .expect("linked vendor fixture must expose phy_param");

    for flags in [0_u32, 1 << 5, 1 << 20, (1 << 5) | (1 << 20)] {
        let mut parameter: [u8; PARAM_LEN] = core::array::from_fn(|index| {
            (index as u8)
                .wrapping_mul(29)
                .wrapping_add((index >> 2) as u8)
        });
        parameter[0] = 0xd7;
        parameter[1] = 0xff;
        parameter[0x0a4..0x0a8].copy_from_slice(&flags.to_le_bytes());

        let update_offset_130 = u32::from(flags & (1 << 5) == 0);
        let update_reference_copies = u32::from(flags & (1 << 20) == 0);
        let mut scenario = execution::Scenario {
            arguments: vec![update_offset_130, update_reference_copies],
            call_returns: std::collections::BTreeMap::from([(
                "phy_tsens_temp_read".to_owned(),
                std::collections::VecDeque::from([0]),
            )]),
            max_steps: 10_000,
            ..execution::Scenario::default()
        };
        for (offset, byte) in parameter.iter().copied().enumerate() {
            scenario
                .memory_initial
                .insert(phy_param + offset as u32, byte);
        }
        let vendor = execution::execute(&image, &svd, "phy_get_temp_init", scenario).unwrap();
        assert_eq!(vendor.ordered_calls.len(), 1);
        assert_eq!(vendor.ordered_calls[0].symbol, "phy_tsens_temp_read");

        let mut rust =
            open_esp_radio_esp32s31_phy::phy_cold::PhyColdState::from_parameter_image(parameter);
        let control = rust.register_temperature_control();
        assert_eq!(control.updates_offset_130(), update_offset_130 != 0);
        assert_eq!(
            control.updates_reference_copies(),
            update_reference_copies != 0
        );
        rust.apply_register_temperature_references(control);

        for (offset, rust_byte) in rust.parameter_image().iter().copied().enumerate() {
            let vendor_byte = vendor
                .persistent_memory
                .get(&(phy_param + offset as u32))
                .copied()
                .unwrap_or(parameter[offset]);
            assert_eq!(
                vendor_byte, rust_byte,
                "temperature flags {flags:#010x} diverged at phy_param+{offset:#05x}",
            );
        }
    }
}
