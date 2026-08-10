use super::super::*;
use crate::harnesses::esp32s31::RISCV_HARNESS;

#[test]
fn structural_polling_recognizes_real_rom_backedges() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workbench facade remains under tools");
    let artifact = private_input("OPEN_ESP_RADIO_ESP32S31_ROM_ELF").unwrap_or_default();
    let companion = root.join(
        "verification/vendor/targets/esp32s31/oracle-firmware/target/riscv32imafc-unknown-none-elf/release/open-esp-radio-vendor-oracle-esp32s31-trace-elf",
    );
    if !artifact.exists() || !companion.exists() {
        eprintln!("private linked PHY fixtures are not installed; integration test skipped");
        return;
    }
    let svd = MmioMap::load_all(&[
        root.join("svd/esp32s31-radio.svd"),
        root.join("svd/esp32s31-platform-radio-deps.svd"),
    ])
    .unwrap();
    let catalog = ReferenceResolver::load_with_entry_contract(
        &artifact,
        &[companion],
        &RISCV_HARNESS,
        entry_contract::PHY_REGISTERED,
    )
    .unwrap();

    for (symbol, expected_polls) in [("phy_pbus_force_test", 1), ("phy_i2c_paral_read", 2)] {
        let trace = catalog.trace(None, symbol, &svd).unwrap();
        assert!(trace.is_reference_eligible(), "{symbol}: {trace:#?}");
        let generated =
            generate_reference(&trace, "esp32s31_rev0_rom.elf", "fixture-sha256", None, &[])
                .unwrap();
        assert_eq!(
            generated.source.matches("// Poll until").count(),
            expected_polls,
            "{symbol}: {}",
            generated.source
        );
        assert_generated_reference_compiles(symbol, &generated.source);
    }
}

#[test]
fn constant_sar_output_loop_becomes_ordered_mmio_and_memory_effects() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workbench facade remains under tools");
    let artifact = private_input("OPEN_ESP_RADIO_ESP32S31_ROM_ELF").unwrap_or_default();
    if !artifact.exists() {
        eprintln!("private ROM fixture is not installed; integration test skipped");
        return;
    }
    let svd = MmioMap::load_all(&[
        root.join("svd/esp32s31-radio.svd"),
        root.join("svd/esp32s31-platform-radio-deps.svd"),
    ])
    .unwrap();
    let catalog = ReferenceResolver::load(&artifact, &[], &RISCV_HARNESS).unwrap();
    let trace = catalog.trace(None, "phy_read_sar_dout", &svd).unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert_eq!(trace.events.len(), 4);
    assert_eq!(trace.reference_events.len(), 12);
    let generated =
        generate_reference(&trace, "esp32s31_rev0_rom.elf", "fixture-sha256", None, &[]).unwrap();
    assert_eq!(generated.source.matches("io.read(").count(), 4);
    assert_eq!(generated.source.matches("memory.write(").count(), 8);
    assert_generated_reference_compiles("phy_read_sar_dout", &generated.source);
}

#[test]
fn wifi_osi_result_survives_direct_call_composition() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "rand_wrapper".to_owned(),
        address: 0x1000,
        bytes: vec![
            0xef, 0x10, 0x00, 0x00, // jal ra, 0x2000
            0x13, 0x75, 0xf5, 0x0f, // andi a0, a0, 255
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let mut child = wifi_osi_tail_symbol(0x0bc);
    child.address = 0x2000;
    child.relocations[0].address = 0x2004;
    let symbols = BTreeMap::from([(0x2000, child)]);
    let mut visiting = BTreeSet::from([0x1000]);

    let trace = resolve_reference_trace(
        &parent,
        &symbols,
        &BTreeMap::new(),
        &StructuralPointerContext::from_harness(&RISCV_HARNESS),
        None,
        &map(),
        &mut visiting,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert_eq!(trace.reference_dependencies, ["wifi_osi_tail"]);
    assert_eq!(
        trace.return_value,
        SymbolicValue::ExternalResult(0).and(0xff)
    );
    assert!(matches!(
        trace.reference_events.as_slice(),
        [DraftReferenceEvent::ExternalCall {
            token: 0,
            function,
            ..
        }] if *function == external_abi::RAND
    ));
}

#[test]
fn composite_svd_catalog_resolves_platform_owned_radio_dependencies() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workbench facade remains under tools");
    let map = MmioMap::load_all(&[
        root.join("svd/esp32s31-radio.svd"),
        root.join("svd/esp32s31-platform-radio-deps.svd"),
    ])
    .unwrap();

    let recovered_phy_gaps = [
        (
            0x2010_0020,
            "PHY_FECOEX_RECOVERED.RF_FREQUENCY_CONTROL_OPAQUE",
        ),
        (
            0x2010_0040,
            "PHY_FECOEX_RECOVERED.RF_FREQUENCY_RESULT_OPAQUE",
        ),
        (0x2010_0430, "PHY_FEDATA_RECOVERED.RX_FILTER_MODE_OPAQUE"),
        (0x2010_0440, "PHY_FEDATA_RECOVERED.TX_RX_RESET_OPAQUE"),
        (
            0x2010_0834,
            "PHY_FECTRL_RECOVERED.ANTENNA_CONFIG_WORD_0_OPAQUE",
        ),
        (
            0x2010_0838,
            "PHY_FECTRL_RECOVERED.ANTENNA_CONFIG_WORD_1_OPAQUE",
        ),
        (
            0x2010_083c,
            "PHY_FECTRL_RECOVERED.ANTENNA_CONFIG_WORD_2_OPAQUE",
        ),
        (
            0x2010_0840,
            "PHY_FECTRL_RECOVERED.ANTENNA_CONFIG_WORD_3_OPAQUE",
        ),
        (
            0x2010_0c18,
            "PHY_FEDATA_WIFI_RECOVERED.FREQUENCY_CORRECTION_WORD_0_OPAQUE",
        ),
        (
            0x2010_0c1c,
            "PHY_FEDATA_WIFI_RECOVERED.FREQUENCY_CORRECTION_WORD_1_OPAQUE",
        ),
        (0x2010_2840, "PHY_BTAGC_RECOVERED.RX_GAIN_FORCE_OPAQUE"),
        (0x2010_2848, "PHY_BTAGC_RECOVERED.GAIN_OFFSET_WORD_0_OPAQUE"),
        (0x2010_2868, "PHY_BTAGC_RECOVERED.GAIN_OFFSET_WORD_1_OPAQUE"),
        (0x2010_7010, "PHY_AGC_RECOVERED_GAPS.RX_SENSE_WORD_0_OPAQUE"),
        (0x2010_7014, "PHY_AGC_RECOVERED_GAPS.RX_SENSE_WORD_1_OPAQUE"),
        (0x2010_701c, "PHY_AGC_RECOVERED_GAPS.CCA_CONTROL_OPAQUE"),
        (
            0x2010_7050,
            "PHY_AGC_RECOVERED_GAPS.NOISE_FLOOR_STATUS_OPAQUE",
        ),
        (0x2010_706c, "PHY_AGC_RECOVERED_GAPS.RSSI_STATUS_OPAQUE"),
        (
            0x2010_7074,
            "PHY_AGC_RECOVERED_GAPS.CHANNEL_FILTER_CONTROL_OPAQUE",
        ),
        (0x2010_70cc, "PHY_AGC_RECOVERED_GAPS.BACKUP_WORD_CC_OPAQUE"),
        (
            0x2010_70f4,
            "PHY_AGC_RECOVERED_GAPS.RIFS_MODE_CONTROL_OPAQUE",
        ),
        (
            0x2010_7108,
            "PHY_AGC_RECOVERED_GAPS.RX_SENSE_BACKUP_WORD_OPAQUE",
        ),
        (
            0x2010_7800,
            "PHY_NRX_RECOVERED_GAPS.FFT_SCALE_CONTROL_OPAQUE",
        ),
        (0x2010_780c, "PHY_NRX_RECOVERED_GAPS.BACKUP_WORD_0C_OPAQUE"),
        (
            0x2010_7898,
            "PHY_NRX_RECOVERED_GAPS.FREQUENCY_CORRECTION_CONTROL_OPAQUE",
        ),
        (
            0x2010_7904,
            "PHY_NRX_RECOVERED_GAPS.CHANNEL_FILTER_CONTROL_OPAQUE",
        ),
        (
            0x2010_7c58,
            "PHY_BB_RECOVERED_GAPS.CCA_COUNTER_CONTROL_OPAQUE",
        ),
        (
            0x2010_7c5c,
            "PHY_BB_RECOVERED_GAPS.CCA_COUNTER_STATUS_0_OPAQUE",
        ),
        (
            0x2010_7c60,
            "PHY_BB_RECOVERED_GAPS.CCA_COUNTER_STATUS_1_OPAQUE",
        ),
        (
            0x2010_8050,
            "PHY_BRX_RECOVERED_GAPS.FREQUENCY_CORRECTION_CONTROL_OPAQUE",
        ),
        (0x2010_9c14, "MODEM_SYSCON.CLK_CONF1"),
        (0x2071_5050, "EFUSE.RD_MAC_SYS0"),
        (0x2071_5054, "EFUSE.RD_MAC_SYS1"),
    ];
    for (address, expected) in recovered_phy_gaps {
        assert_eq!(
            map.display_register_name(address),
            expected,
            "address {address:#010x}"
        );
    }

    assert_eq!(
        map.display_register_name(0x2010_9c18),
        "MODEM_SYSCON.WIFI_BB_CFG"
    );
    let modem_lpcon_registers = [
        "TEST_CONF",
        "LP_TIMER_CONF",
        "COEX_LP_CLK_CONF",
        "WIFI_LP_CLK_CONF",
        "MODEM_SRC_CLK_CONF",
        "MODEM_32K_CLK_CONF",
        "CLK_CONF",
        "CLK_CONF_FORCE_ON",
        "CLK_CONF_POWER_ST",
        "RST_CONF",
        "TICK_CONF",
        "MEM_CONF",
        "MEM_RF1_AUX_CTRL",
        "MEM_RF2_AUX_CTRL",
        "APB_MEM_SEL",
        "DCMEM_VALID_0",
        "DCMEM_VALID_1",
        "DCMEM_VALID_2",
        "DCMEM_VALID_3",
        "MODEM_INTR_WAKEUP_CONF",
        "MODEM_INTR_STATUS",
        "MEM_CONF2",
        "DATE",
    ];
    for (index, register) in modem_lpcon_registers.into_iter().enumerate() {
        let address = 0x2010_f000 + u32::try_from(index).unwrap() * 4;
        assert_eq!(
            map.display_register_name(address),
            format!("MODEM_LPCON.{register}"),
            "MODEM_LPCON address {address:#010x}"
        );
    }
    assert_eq!(
        map.display_register_name(0x2010_f800),
        "I2C_ANA_MST.I2C0_CTRL"
    );
    assert_eq!(
        map.display_register_name(0x2010_f824),
        "I2C_ANA_MST.I2C0_CTRL1"
    );
    assert_eq!(
        map.display_register_name(0x2010_f828),
        "I2C_ANA_MST.I2C1_CTRL1"
    );
    assert_eq!(
        map.display_register_name(0x2010_f82c),
        "I2C_ANA_MST.HW_I2C_CTRL"
    );
    assert_eq!(
        map.display_register_name(0x2070_1068),
        "LP_AON_CLKRST.RTC_SAR2_PWDET_CCT"
    );
    assert_eq!(
        map.display_register_name(0x2070_401c),
        "PMU.HP_ACTIVE_HP_CK_POWER"
    );
    assert_eq!(
        map.display_register_name(0x2070_40f0),
        "PMU.IMM_HP_CK_POWER_0"
    );
    assert_eq!(map.display_register_name(0x2070_4184), "PMU.RF_PWC");
    assert_eq!(
        map.display_register_name(0x2070_4208),
        "PMU.ANA_PERI_PWR_CTRL"
    );
    assert_eq!(
        map.display_register_name(0x2071_0030),
        "LP_PERICLKRST.TSENS_CTRL"
    );
    assert_eq!(map.display_register_name(0x2081_8000), "LP_TSENS.CTRL");
    assert_eq!(map.display_register_name(0x2081_8018), "LP_TSENS.CLK_CONF");
}
