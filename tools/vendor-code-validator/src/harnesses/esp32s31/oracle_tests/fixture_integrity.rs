use super::super::*;

#[test]
fn artifact_digest_is_reported_without_embedding_a_trust_policy() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let digest = artifact_sha256(&manifest).unwrap();
    assert_eq!(digest.len(), 64);
    assert!(digest.bytes().all(|byte| byte.is_ascii_hexdigit()));
}

#[test]
fn composite_svd_catalog_rejects_same_address_with_different_names() {
    let registers = [
        Register {
            address: 0x2010_0010,
            name: "FIRST.REGISTER".to_owned(),
        },
        Register {
            address: 0x2010_0010,
            name: "SECOND.REGISTER".to_owned(),
        },
    ];
    let error = reject_register_collisions(&registers).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("conflicting SVD register definitions")
    );
}

#[test]
fn source_qualified_probe_names_disambiguate_vendor_sources() {
    assert!(rust_probe_suffix_matches(
        "archive",
        "set_bb_wdg",
        "archive_set_bb_wdg"
    ));
    assert!(!rust_probe_suffix_matches(
        "rom",
        "set_bb_wdg",
        "archive_set_bb_wdg"
    ));
    assert!(rust_probe_suffix_matches(
        "archive",
        "set_bb_wdg",
        "set_bb_wdg"
    ));
}

#[test]
fn structural_loader_reproduces_both_vendor_inventories() {
    let (Some(rom), Some(archive)) = (
        private_input("OPEN_ESP_RADIO_ESP32S31_ROM_ELF"),
        private_input("OPEN_ESP_RADIO_ESP32S31_LIBPHY_ARCHIVE"),
    ) else {
        eprintln!("private vendor inventory fixtures are not installed; integration test skipped");
        return;
    };
    if !rom.exists() || !archive.exists() {
        eprintln!("private vendor inventory fixtures are not installed; integration test skipped");
        return;
    }
    assert_eq!(artifact::load_symbols(&rom, "phy_").unwrap().len(), 305);
    assert_eq!(artifact::load_symbols(&archive, "").unwrap().len(), 161);
}

#[test]
fn archive_loader_retains_riscv_data_relocation_addends_and_store_kinds() {
    let Some(archive) = private_input("OPEN_ESP_RADIO_ESP32S31_LIBPP_ARCHIVE") else {
        eprintln!("private libpp fixture is not installed; integration test skipped");
        return;
    };
    if !archive.exists() {
        eprintln!("private libpp fixture is not installed; integration test skipped");
        return;
    }
    let symbols = artifact::load_symbols(&archive, "hal_tsf_get_tbttstart").unwrap();
    let symbol = symbols
        .iter()
        .find(|symbol| symbol.name == "hal_tsf_get_tbttstart")
        .unwrap();
    assert!(symbol.relocations.iter().any(|relocation| {
        relocation.kind == artifact::RelocationKind::Lo12I
            && relocation.symbol == ".LANCHOR0"
            && relocation.addend == 4
    }));

    let symbols = artifact::load_symbols(&archive, "pp_timer_register_post_cb").unwrap();
    let symbol = symbols
        .iter()
        .find(|symbol| symbol.name == "pp_timer_register_post_cb")
        .unwrap();
    assert!(
        symbol
            .relocations
            .iter()
            .any(|relocation| relocation.kind == artifact::RelocationKind::Lo12S)
    );

    let symbols = artifact::load_symbols(&archive, "hal_set_ofdma_sequence_pti").unwrap();
    let symbol = symbols
        .iter()
        .find(|symbol| symbol.name == "hal_set_ofdma_sequence_pti")
        .unwrap();
    assert!(symbol.relocations.iter().any(|relocation| {
        relocation.kind == artifact::RelocationKind::Call && relocation.symbol == "hal_set_tb_pti"
    }));
    assert!(symbol.relocations.iter().any(|relocation| {
        relocation.kind == artifact::RelocationKind::Call && relocation.symbol == "wifi_log"
    }));
}

#[test]
fn concrete_rom_execution_keeps_target_specific_call_and_mmio_semantics() {
    let Some(rom) = private_input("OPEN_ESP_RADIO_ESP32S31_ROM_ELF") else {
        eprintln!("private ROM fixture is not installed; integration test skipped");
        return;
    };
    if !rom.exists() {
        eprintln!("private ROM fixture is not installed; integration test skipped");
        return;
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("validator facade remains under tools");
    let image = execution::ExecutableImage::load(&rom).unwrap();
    let svd = MmioRegisterMap::load(&root.join("svd/esp32s31-radio.svd")).unwrap();

    let mut frequency = execution::Scenario {
        arguments: vec![1],
        ..execution::Scenario::default()
    };
    frequency.mmio_initial.insert(0x2010_7030, u32::MAX);
    frequency.mmio_initial.insert(0x2010_7ce4, 0);
    let result = execution::execute(&image, &svd, "phy_freq_band_reg_set", frequency).unwrap();
    assert_eq!(result.events.len(), 4);
    assert_eq!(
        result.events[1],
        execution::ExecutionEvent::Write {
            width: 32,
            address: 0x2010_7030,
            register: "PHY_AGC_ORACLE.AGC_ANTENNA_CONTROL".to_owned(),
            value: !(1 << 5),
        }
    );
    assert_eq!(
        result.events[3],
        execution::ExecutionEvent::Write {
            width: 32,
            address: 0x2010_7ce4,
            register: "PHY_FREQUENCY_CHANNEL_ORACLE.CHANNEL_CBW_CONTROL_1".to_owned(),
            value: 1 << 5,
        }
    );
    assert!(result.calls.contains("phy_vht_support"));

    let mut delayed = execution::Scenario::default();
    delayed.mmio_initial.insert(0x2010_001c, 0);
    let result = execution::execute(&image, &svd, "phy_dis_hw_set_freq", delayed).unwrap();
    assert!(matches!(
        result.events.last(),
        Some(execution::ExecutionEvent::DelayMicros(2))
    ));

    assert!(
        !image
            .coverage_inventory("phy_bb_bss_cbw40")
            .unwrap()
            .branch_sites
            .is_empty()
    );
    let wrapper = image.coverage_inventory("phy_pbus_debugmode").unwrap();
    assert_eq!(wrapper.branch_outcomes.len(), 1);
    assert!(wrapper.branch_outcomes.iter().all(|(_, taken)| !taken));
    let child = image.coverage_inventory("phy_pbus_force_mode").unwrap();
    assert!(child.branch_outcomes.iter().any(|(_, taken)| *taken));
    assert!(child.branch_outcomes.iter().any(|(_, taken)| !*taken));
}

#[test]
fn generated_reference_survives_compile_and_reextract_for_selected_target() {
    let Some(artifact) = private_input("OPEN_ESP_RADIO_ESP32S31_ROM_ELF") else {
        eprintln!("private ROM fixture was not supplied; integration test skipped");
        return;
    };
    if !artifact.exists() {
        eprintln!("private ROM fixture is not installed; integration test skipped");
        return;
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("validator facade remains under tools");
    let svd = MmioRegisterMap::load_all(&[
        root.join("svd/esp32s31-radio.svd"),
        root.join("svd/esp32s31-platform-radio-deps.svd"),
    ])
    .unwrap();
    let input = ArtifactSymbolSelector {
        artifact,
        member: None,
        symbol: "phy_disable_agc".to_owned(),
    };
    let vendor = extract(&input, &svd).unwrap();
    let proof = generated_reference::generate_compile_and_prove_exact_mmio_leaf(
        &svd,
        "esp32s31-phy-v1",
        "riscv32imafc-unknown-none-elf",
        &input,
        &[],
        &vendor,
    )
    .unwrap();
    assert!(traces_equal(&vendor, &proof.trace));
    assert!(
        proof
            .canonical()
            .contains("generated-reference exact-mmio-leaf-v1")
    );
}
