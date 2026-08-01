use super::*;

#[test]
fn wifi_osi_rand_tail_call_resolves_from_relocation() {
    let symbol = wifi_osi_tail_symbol(0x0bc);
    let trace = trace_binary_symbol(
        &symbol,
        &map(),
        &BTreeMap::new(),
        &StructuralPointerContext::default(),
        None,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert_eq!(trace.return_value, SymbolicValue::ExternalResult(0));
    assert!(matches!(
        trace.reference_events.as_slice(),
        [DraftReferenceEvent::ExternalCall {
            token: 0,
            table: external_abi::Table::Esp32s31WifiOsiV9,
            function: external_abi::Function::Rand,
            ..
        }]
    ));

    let generated = generate_reference(
        &trace,
        "libpp.a",
        ESP32S31_LIBPP_SHA256,
        Some("hal_mac.o"),
        &[],
    )
    .unwrap();
    assert!(generated.source.contains("pub trait ReferencePlatform"));
    assert!(
        generated
            .source
            .contains("let external_result0 = platform.wifi_osi_rand();")
    );
    assert!(
        generated
            .source
            .contains("assert_eq!(platform.wifi_osi_version(), 0x00000009_u32")
    );
    assert!(
        generated
            .source
            .contains("assert_eq!(platform.wifi_osi_magic(), 0xdeadbeaf_u32")
    );
    assert!(
        generated
            .source
            .contains("assert_eq!(platform.wifi_osi_table_size(), 0x00000200_u32")
    );
    assert!(
        generated
            .source
            .contains("ReferenceOutcome { exit_a0: Some(external_result0) }")
    );
    assert!(
        generated.source.contains(
            external_abi::table_spec(external_abi::Table::Esp32s31WifiOsiV9).source_sha256
        )
    );
}

#[test]
fn unknown_wifi_osi_slot_fails_closed() {
    let symbol = wifi_osi_tail_symbol(0x0c0);
    let trace = trace_binary_symbol(
        &symbol,
        &map(),
        &BTreeMap::new(),
        &StructuralPointerContext::default(),
        None,
    )
    .unwrap();

    assert!(!trace.is_reference_eligible());
    assert!(trace.reference_blockers.iter().any(|blocker| {
        blocker.contains("unregistered-external-abi-slot") && blocker.contains("+0xc0")
    }));
}

#[test]
fn wifi_osi_output_pointer_outside_private_stack_fails_closed() {
    let symbol = wifi_osi_tail_symbol(0x1a8);
    let trace = trace_binary_symbol(
        &symbol,
        &map(),
        &BTreeMap::new(),
        &StructuralPointerContext::default(),
        None,
    )
    .unwrap();

    assert!(!trace.is_reference_eligible());
    assert!(trace.reference_blockers.iter().any(|blocker| {
        blocker.contains("unsupported-external-output-pointer")
            && blocker.contains("_coex_pti_get")
            && blocker.contains("a1")
    }));
}

#[test]
fn real_libpp_hal_random_resolves_through_wifi_osi_abi() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("phy-trace remains under tools/phy-trace");
    let artifact = root.join("_oracles/libpp.a");
    if !artifact.exists() {
        eprintln!("private libpp fixture is not installed; integration test skipped");
        return;
    }

    let trace = ReferenceResolver::load(&artifact, &[])
        .unwrap()
        .trace(Some("hal_mac.o"), "hal_random", &map())
        .unwrap();
    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert_eq!(trace.return_value, SymbolicValue::ExternalResult(0));
}

#[test]
fn real_libpp_coex_output_bytes_reach_compilable_reference_codegen() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("phy-trace remains under tools/phy-trace");
    let artifact = root.join("_oracles/libpp.a");
    if !artifact.exists() {
        eprintln!("private libpp fixture is not installed; integration test skipped");
        return;
    }
    let svd = MmioRegisterMap::load_all(&[
        root.join("svd/esp32s31-radio.svd"),
        root.join("svd/esp32s31-platform-radio-deps.svd"),
    ])
    .unwrap();
    let trace = ReferenceResolver::load(&artifact, &[])
        .unwrap()
        .trace(Some("hal_coex.o"), "hal_set_ofdma_sequence_pti", &svd)
        .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert_eq!(
        trace
            .reference_events
            .iter()
            .filter(|event| matches!(event, DraftReferenceEvent::ExternalCall { .. }))
            .count(),
        12
    );
    assert_eq!(
        trace.reference_dependencies,
        [
            "hal_set_tb_pti",
            "hal_set_beamf_pti",
            "hal_set_beamf_mt_pti"
        ]
    );
    assert!(trace.reference_events.iter().any(|event| matches!(
        event,
        DraftReferenceEvent::DiagnosticCall { function, .. } if function == "wifi_log"
    )));
    let generated = generate_reference(
        &trace,
        "libpp.a",
        ESP32S31_LIBPP_SHA256,
        Some("hal_coex.o"),
        &[],
    )
    .unwrap();
    assert_eq!(
        generated.source.matches("wifi_osi_coex_pti_get(").count(),
        13
    );
    assert_generated_reference_compiles("hal_set_ofdma_sequence_pti", &generated.source);
}

#[test]
fn real_libpp_coex_runtime_leaves_generate_compilable_references() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("phy-trace remains under tools/phy-trace");
    let artifact = root.join("_oracles/libpp.a");
    if !artifact.exists() {
        eprintln!("private libpp fixture is not installed; integration test skipped");
        return;
    }
    let svd = MmioRegisterMap::load_all(&[
        root.join("svd/esp32s31-radio.svd"),
        root.join("svd/esp32s31-platform-radio-deps.svd"),
    ])
    .unwrap();
    let catalog = ReferenceResolver::load(&artifact, &[]).unwrap();

    for symbol in [
        "hal_set_rx_beacon_time",
        "hal_set_rx_beacon_pti",
        "hal_clear_rx_beacon_pti",
        "hal_set_itwt_pti",
        "hal_clr_itwt_pti",
    ] {
        let trace = catalog.trace(Some("hal_coex.o"), symbol, &svd).unwrap();
        assert!(trace.is_reference_eligible(), "{symbol}: {trace:#?}");
        let generated = generate_reference(
            &trace,
            "libpp.a",
            ESP32S31_LIBPP_SHA256,
            Some("hal_coex.o"),
            &[],
        )
        .unwrap();
        assert_generated_reference_compiles(symbol, &generated.source);
    }
}

#[test]
fn real_libpp_tsf_runtime_leaves_generate_compilable_references() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("phy-trace remains under tools/phy-trace");
    let artifact = root.join("_oracles/libpp.a");
    if !artifact.exists() {
        eprintln!("private libpp fixture is not installed; integration test skipped");
        return;
    }
    let svd = MmioRegisterMap::load_all(&[
        root.join("svd/esp32s31-radio.svd"),
        root.join("svd/esp32s31-platform-radio-deps.svd"),
    ])
    .unwrap();
    let catalog = ReferenceResolver::load(&artifact, &[]).unwrap();

    for (member, symbol) in [
        ("hal_tsf.o", "hal_enable_nan_tsf"),
        ("hal_tsf.o", "hal_disable_nan_tsf"),
        ("hal_tsf.o", "hal_disable_softap_tsf"),
        ("hal_tsf.o", "hal_set_sta_tbtt"),
        ("hal_tsf.o", "hal_set_sta_tbtt_interval"),
        ("hal_tsf.o", "hal_set_sta_light_sleep_wake_ahead_time"),
        ("hal_tsf.o", "hal_is_sta_tsf_active"),
        ("hal_tsf.o", "hal_tsf_clear_soc_wakeup_request"),
        ("hal_mac.o", "hal_enable_sta_btwt_tsf"),
    ] {
        let trace = catalog.trace(Some(member), symbol, &svd).unwrap();
        assert!(
            trace.is_reference_eligible(),
            "{member}::{symbol}: {trace:#?}"
        );
        let generated =
            generate_reference(&trace, "libpp.a", ESP32S31_LIBPP_SHA256, Some(member), &[])
                .unwrap();
        assert_generated_reference_compiles(symbol, &generated.source);
    }
}

#[test]
fn real_libpp_remaining_mmio_leaves_generate_compilable_references() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("phy-trace remains under tools/phy-trace");
    let artifact = root.join("_oracles/libpp.a");
    if !artifact.exists() {
        eprintln!("private libpp fixture is not installed; integration test skipped");
        return;
    }
    let svd = MmioRegisterMap::load_all(&[
        root.join("svd/esp32s31-radio.svd"),
        root.join("svd/esp32s31-platform-radio-deps.svd"),
    ])
    .unwrap();
    let catalog = ReferenceResolver::load(&artifact, &[]).unwrap();

    for (member, symbol) in [
        ("hal_mac.o", "hal_beacon_ie_crc_get"),
        ("hal_mac.o", "hal_enable_sta_beacon_filter"),
        ("hal_mac.o", "hal_disable_sta_beacon_filter"),
        ("hal_mac_ctl.o", "hal_he_set_hw_qos_null_ra_to_trans"),
        ("hal_mac_ctl.o", "hal_mac_interrupt_clr_bsscolor"),
        ("hal_mac_rx.o", "hal_mac_rx_get_end_state"),
        ("hal_mac_rx.o", "hal_mac_rx_get_end_info"),
        ("hal_sniffer.o", "hal_sniffer_rx_clr_statistics"),
    ] {
        let trace = catalog.trace(Some(member), symbol, &svd).unwrap();
        assert!(
            trace.is_reference_eligible(),
            "{member}::{symbol}: {trace:#?}"
        );
        let generated =
            generate_reference(&trace, "libpp.a", ESP32S31_LIBPP_SHA256, Some(member), &[])
                .unwrap();
        assert_generated_reference_compiles(symbol, &generated.source);
    }
}

#[test]
fn real_libpp_timer_update_generates_both_symbolic_cfg_paths() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("phy-trace remains under tools/phy-trace");
    let artifact = root.join("_oracles/libpp.a");
    if !artifact.exists() {
        eprintln!("private libpp fixture is not installed; integration test skipped");
        return;
    }
    let svd = MmioRegisterMap::load_all(&[
        root.join("svd/esp32s31-radio.svd"),
        root.join("svd/esp32s31-platform-radio-deps.svd"),
    ])
    .unwrap();

    let trace = ReferenceResolver::load(&artifact, &[])
        .unwrap()
        .trace(Some("hal_tsf.o"), "hal_timer_update_by_rtc", &svd)
        .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert!(!trace.is_exact());
    assert!(trace.reference_flow.is_some());
    let generated = generate_reference(
        &trace,
        "libpp.a",
        ESP32S31_LIBPP_SHA256,
        Some("hal_tsf.o"),
        &[],
    )
    .unwrap();
    assert!(generated.exit_a0_modeled);
    assert!(generated.source.contains("if (args[0]"));
    assert!(generated.source.contains("0x2010d830_u32"));
    assert!(generated.source.contains("0x2010d878_u32"));
    assert!(generated.source.contains("0x08000000_u32"));
    assert!(generated.source.contains("0x0003ffff_u32"));
}

#[test]
fn real_libpp_indexed_mmio_generates_guarded_compilable_references() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("phy-trace remains under tools/phy-trace");
    let artifact = root.join("_oracles/libpp.a");
    if !artifact.exists() {
        eprintln!("private libpp fixture is not installed; integration test skipped");
        return;
    }
    let svd = MmioRegisterMap::load_all(&[
        root.join("svd/esp32s31-radio.svd"),
        root.join("svd/esp32s31-platform-radio-deps.svd"),
    ])
    .unwrap();
    let catalog = ReferenceResolver::load(&artifact, &[]).unwrap();

    for (member, symbol) in [
        ("hal_mac.o", "hal_mac_is_txq_valid"),
        ("hal_mac.o", "hal_mac_clr_bssid"),
        ("hal_mac_ctl.o", "hal_he_set_ac_muedca_param"),
    ] {
        let trace = catalog.trace(Some(member), symbol, &svd).unwrap();
        assert!(trace.is_reference_eligible(), "{symbol}: {trace:#?}");
        let generated =
            generate_reference(&trace, "libpp.a", ESP32S31_LIBPP_SHA256, Some(member), &[])
                .unwrap();
        if member == "hal_mac.o" {
            assert!(generated.source.contains("let mmio_selector0 = args[0]"));
        }
        assert!(generated.source.contains("assert!(matches!(mmio_address0"));
        assert_generated_reference_compiles(symbol, &generated.source);
    }

    for symbol in [
        "hal_disable_tsf_timer_wakeup",
        "hal_enable_tsf_timer_wakeup",
        "hal_tsf_timer_set_target",
        "hal_tsf_timer_get_target",
        "hal_disable_tsf_timer",
        "hal_enable_tsf_timer",
    ] {
        let trace = catalog.trace(Some("hal_tsf.o"), symbol, &svd).unwrap();
        assert!(trace.is_reference_eligible(), "{symbol}: {trace:#?}");
        let generated = generate_reference(
            &trace,
            "libpp.a",
            ESP32S31_LIBPP_SHA256,
            Some("hal_tsf.o"),
            &[],
        )
        .unwrap();
        assert!(generated.source.contains("assert!(matches!(mmio_address"));
        assert_generated_reference_compiles(symbol, &generated.source);
    }
}

#[test]
fn real_libpp_caller_memory_accessors_generate_compilable_references() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("phy-trace remains under tools/phy-trace");
    let artifact = root.join("_oracles/libpp.a");
    if !artifact.exists() {
        eprintln!("private libpp fixture is not installed; integration test skipped");
        return;
    }
    let svd = MmioRegisterMap::load_all(&[
        root.join("svd/esp32s31-radio.svd"),
        root.join("svd/esp32s31-platform-radio-deps.svd"),
    ])
    .unwrap();
    let catalog = ReferenceResolver::load(&artifact, &[]).unwrap();

    for (member, name) in [
        ("hal_mac.o", "hal_mac_ftm_get_t3"),
        ("hal_mac_ctl.o", "hal_mac_get_csi_filter"),
    ] {
        let trace = catalog.trace(Some(member), name, &svd).unwrap();
        assert!(
            trace.is_reference_eligible(),
            "{member}::{name}: {trace:#?}"
        );
        assert!(trace.reference_events.iter().any(|event| matches!(
            event,
            DraftReferenceEvent::Memory { region, .. }
                if region == "caller-owned ABI argument RAM"
        )));
        let generated =
            generate_reference(&trace, "libpp.a", ESP32S31_LIBPP_SHA256, Some(member), &[])
                .unwrap();
        assert_generated_reference_compiles(name, &generated.source);
    }
}

#[test]
fn real_libpp_relocated_state_accessors_generate_compilable_references() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("phy-trace remains under tools/phy-trace");
    let artifact = root.join("_oracles/libpp.a");
    if !artifact.exists() {
        eprintln!("private libpp fixture is not installed; integration test skipped");
        return;
    }
    let svd = MmioRegisterMap::load_all(&[
        root.join("svd/esp32s31-radio.svd"),
        root.join("svd/esp32s31-platform-radio-deps.svd"),
    ])
    .unwrap();
    let catalog = ReferenceResolver::load(&artifact, &[]).unwrap();

    for (member, name) in [
        ("hal_mac.o", "hal_mac_set_csi"),
        ("hal_tsf.o", "hal_tsf_get_tbttstart"),
    ] {
        let trace = catalog.trace(Some(member), name, &svd).unwrap();
        assert!(
            trace.is_reference_eligible(),
            "{member}::{name}: {trace:#?}"
        );
        let generated =
            generate_reference(&trace, "libpp.a", ESP32S31_LIBPP_SHA256, Some(member), &[])
                .unwrap();
        assert!(generated.source.contains("memory.symbol_address("));
        assert_generated_reference_compiles(name, &generated.source);
    }
}

#[test]
fn real_libpp_hal_analysis_baseline_and_codegen_remain_stable() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("phy-trace remains under tools/phy-trace");
    let artifact = root.join("_oracles/libpp.a");
    if !artifact.exists() {
        eprintln!("private libpp fixture is not installed; integration test skipped");
        return;
    }
    let svd = MmioRegisterMap::load_all(&[
        root.join("svd/esp32s31-radio.svd"),
        root.join("svd/esp32s31-platform-radio-deps.svd"),
    ])
    .unwrap();
    let catalog = ReferenceResolver::load(&artifact, &[]).unwrap();
    let symbols = catalog
        .symbols
        .iter()
        .filter(|symbol| symbol.name.starts_with("hal_"))
        .map(|symbol| (symbol.member.clone(), symbol.name.clone()))
        .collect::<Vec<_>>();
    assert_eq!(symbols.len(), 220, "the pinned libpp HAL inventory changed");

    let mut direct_exact = 0usize;
    let mut eligible = 0usize;
    let mut unmapped_mmio = BTreeSet::new();

    for (member, name) in symbols {
        let symbol = catalog
            .symbols
            .iter()
            .find(|candidate| {
                candidate.name == name && candidate.member.as_deref() == member.as_deref()
            })
            .expect("catalog identity came from the catalog");
        let direct = trace_binary_symbol(
            symbol,
            &svd,
            &BTreeMap::new(),
            &StructuralPointerContext::default(),
            None,
        )
        .unwrap();
        direct_exact += usize::from(direct.is_exact());
        unmapped_mmio.extend(
            direct
                .events
                .iter()
                .filter_map(ObservableEvent::unmapped_address),
        );

        let trace = catalog.trace(member.as_deref(), &name, &svd).unwrap();
        if trace.is_reference_eligible() {
            eligible += 1;
            let generated = generate_reference(
                &trace,
                "libpp.a",
                ESP32S31_LIBPP_SHA256,
                member.as_deref(),
                &[],
            )
            .unwrap_or_else(|error| panic!("eligible {member:?}::{name} failed: {error}"));
            if trace.reference_indexed_mmio_count() != 0 {
                assert_generated_reference_compiles(&name, &generated.source);
            }
        }
    }
    assert_eq!(direct_exact, 120, "direct trace coverage changed");
    assert_eq!(eligible, 169, "reference codegen coverage changed");
    assert_eq!(220 - eligible, 51, "reference blocker count changed");
    assert!(
        unmapped_mmio.is_empty(),
        "the pinned SVD set no longer maps {unmapped_mmio:#x?}"
    );
}

#[test]
fn real_libpp_mac_delay_names_both_wifi_osi_callbacks() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("phy-trace remains under tools/phy-trace");
    let artifact = root.join("_oracles/libpp.a");
    if !artifact.exists() {
        eprintln!("private libpp fixture is not installed; integration test skipped");
        return;
    }

    let trace = ReferenceResolver::load(&artifact, &[])
        .unwrap()
        .trace(Some("hal_mac_ctl.o"), "hal_he_set_mac_delay", &map())
        .unwrap();
    let functions = trace
        .reference_events
        .iter()
        .filter_map(|event| match event {
            DraftReferenceEvent::ExternalCall { function, .. } => Some(*function),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(functions.starts_with(&[
        external_abi::Function::EnvIsChip,
        external_abi::Function::Random,
    ]));
    assert!(trace.reference_blockers.iter().all(|blocker| {
        !blocker.contains("esp32s31-wifi-osi-v9+0x4")
            && !blocker.contains("esp32s31-wifi-osi-v9+0x144")
    }));
}

#[test]
fn linked_rom_catalog_discovers_wifi_osi_pointer_cell_by_symbol() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("phy-trace remains under tools/phy-trace");
    let artifact = root.join("_oracles/esp32s31_rev0_rom.elf");
    if !artifact.exists() {
        eprintln!("private ROM fixture is not installed; integration test skipped");
        return;
    }

    let catalog = ReferenceResolver::load(&artifact, &[]).unwrap();
    assert_eq!(
        catalog
            .pointer_context
            .external_pointer_cells
            .get(&0x2f07_ff44),
        Some(&external_abi::Table::Esp32s31WifiOsiV9)
    );
}

#[test]
fn registered_phy_contract_composes_pinned_i2c_polling_summaries() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("phy-trace remains under tools/phy-trace");
    let artifact = root.join("_oracles/esp32s31_rev0_rom.elf");
    let companion = root.join(
        "hil/vendor-oracle/esp32s31/target/riscv32imafc-unknown-none-elf/release/open-esp-radio-vendor-oracle-esp32s31-trace-elf",
    );
    if !artifact.exists() || !companion.exists() {
        eprintln!("private linked PHY fixtures are not installed; integration test skipped");
        return;
    }
    let svd = MmioRegisterMap::load_all(&[
        root.join("svd/esp32s31-radio.svd"),
        root.join("svd/esp32s31-platform-radio-deps.svd"),
    ])
    .unwrap();
    let catalog = ReferenceResolver::load_with_entry_contract(
        &artifact,
        &[companion],
        entry_contract::EntryContract::Esp32s31PhyRegistered,
    )
    .unwrap();

    for symbol in ["phy_chip_i2c_readReg", "phy_chip_i2c_writeReg"] {
        let trace = catalog.trace(None, symbol, &svd).unwrap();
        assert!(trace.is_reference_eligible(), "{symbol}: {trace:#?}");
        let generated =
            generate_reference(&trace, "esp32s31_rev0_rom.elf", "fixture-sha256", None, &[])
                .unwrap();
        assert!(generated.source.contains("// Poll until"));
        assert_generated_reference_compiles(symbol, &generated.source);
    }
}

#[test]
fn registered_phy_contract_composes_exact_rom_wide_division() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("phy-trace remains under tools/phy-trace");
    let artifact = root.join("_oracles/esp32s31_rev0_rom.elf");
    let companion = root.join(
        "hil/vendor-oracle/esp32s31/target/riscv32imafc-unknown-none-elf/release/open-esp-radio-vendor-oracle-esp32s31-trace-elf",
    );
    if !artifact.exists() || !companion.exists() {
        eprintln!("private linked PHY fixtures are not installed; integration test skipped");
        return;
    }
    let svd = MmioRegisterMap::load_all(&[
        root.join("svd/esp32s31-radio.svd"),
        root.join("svd/esp32s31-platform-radio-deps.svd"),
    ])
    .unwrap();
    let mut catalog = ReferenceResolver::load_with_entry_contract(
        &artifact,
        &[companion],
        entry_contract::EntryContract::Esp32s31PhyRegistered,
    )
    .unwrap();

    let trace = catalog.trace(None, "phy_rfpll_set_freq", &svd).unwrap();
    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert_eq!(
        trace
            .reference_dependencies
            .iter()
            .filter(|dependency| dependency.as_str() == "__divdi3")
            .count(),
        8,
        "the four structured paths each contain the exact two-call division chain"
    );
    let generated =
        generate_reference(&trace, "esp32s31_rev0_rom.elf", "fixture-sha256", None, &[]).unwrap();
    assert!(generated.source.contains("riscv_div_i64_words"));
    assert!(generated.source.contains("call_result0_high"));
    assert_generated_reference_compiles("phy_rfpll_set_freq", &generated.source);

    let divdi3 = catalog
        .symbols_by_address
        .get_mut(&ESP32S31_ROM_DIVDI3_ADDRESS)
        .expect("pinned ROM must contain __divdi3");
    divdi3.bytes[0] ^= 1;
    let changed = catalog.trace(None, "phy_rfpll_set_freq", &svd).unwrap();
    assert!(!changed.is_reference_eligible());
    assert!(
        changed
            .reference_failure_reasons()
            .iter()
            .any(|reason| reason.contains("__divdi3")),
        "{changed:#?}"
    );
}

#[test]
fn registered_phy_contract_composes_the_bounded_rfpll_poll() {
    const PHY_WAIT_RFPLL_CAL_END_ADDRESS: u32 = 0x2f82_5874;

    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("phy-trace remains under tools/phy-trace");
    let artifact = root.join("_oracles/esp32s31_rev0_rom.elf");
    let companion = root.join(
        "hil/vendor-oracle/esp32s31/target/riscv32imafc-unknown-none-elf/release/open-esp-radio-vendor-oracle-esp32s31-trace-elf",
    );
    if !artifact.exists() || !companion.exists() {
        eprintln!("private linked PHY fixtures are not installed; integration test skipped");
        return;
    }
    let svd = MmioRegisterMap::load_all(&[
        root.join("svd/esp32s31-radio.svd"),
        root.join("svd/esp32s31-platform-radio-deps.svd"),
    ])
    .unwrap();
    let mut catalog = ReferenceResolver::load_with_entry_contract(
        &artifact,
        &[companion],
        entry_contract::EntryContract::Esp32s31PhyRegistered,
    )
    .unwrap();

    let trace = catalog.trace(None, "phy_wait_rfpll_cal_end", &svd).unwrap();
    assert!(trace.is_reference_eligible(), "{trace:#?}");
    let generated =
        generate_reference(&trace, "esp32s31_rev0_rom.elf", "fixture-sha256", None, &[]).unwrap();
    assert!(
        generated
            .source
            .contains("for bounded_poll_attempt0 in 0..100_u16")
    );
    assert!(
        generated
            .source
            .contains("platform.ets_printf(0x2f84d9cc_u32)")
    );
    assert_generated_reference_compiles("phy_wait_rfpll_cal_end", &generated.source);

    let symbol = catalog
        .symbols
        .iter_mut()
        .find(|symbol| symbol.address == u64::from(PHY_WAIT_RFPLL_CAL_END_ADDRESS))
        .expect("pinned ROM must contain phy_wait_rfpll_cal_end");
    symbol.bytes[0] ^= 1;
    let changed = catalog.trace(None, "phy_wait_rfpll_cal_end", &svd).unwrap();
    assert!(!changed.is_reference_eligible());
    assert!(
        changed
            .reference_failure_reasons()
            .iter()
            .any(|reason| reason.contains("branch exploration did not cover both outcomes")),
        "{changed:#?}"
    );
}

#[test]
fn registered_phy_contract_composes_the_rfpll_cap_search() {
    const PHY_RFPLL_CAP_INIT_CAL_ADDRESS: u32 = 0x2f82_5ada;

    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("phy-trace remains under tools/phy-trace");
    let artifact = root.join("_oracles/esp32s31_rev0_rom.elf");
    let companion = root.join(
        "hil/vendor-oracle/esp32s31/target/riscv32imafc-unknown-none-elf/release/open-esp-radio-vendor-oracle-esp32s31-trace-elf",
    );
    if !artifact.exists() || !companion.exists() {
        eprintln!("private linked PHY fixtures are not installed; integration test skipped");
        return;
    }
    let svd = MmioRegisterMap::load_all(&[
        root.join("svd/esp32s31-radio.svd"),
        root.join("svd/esp32s31-platform-radio-deps.svd"),
    ])
    .unwrap();
    let mut catalog = ReferenceResolver::load_with_entry_contract(
        &artifact,
        &[companion],
        entry_contract::EntryContract::Esp32s31PhyRegistered,
    )
    .unwrap();
    let calibration_tests = r#"
struct CalibrationIo {
    initial: u16,
    statuses: Vec<u8>,
    status_index: usize,
    read_register: u8,
    read_phase: u8,
}

impl CalibrationIo {
    fn new(initial: u16, statuses: Vec<u8>) -> Self {
        Self { initial, statuses, status_index: 0, read_register: 0, read_phase: 0 }
    }

    fn read_data(&mut self) -> u8 {
        match self.read_register {
            0x05 => self.initial as u8,
            0x07 => (((self.initial >> 8) & 1) as u8) << 2,
            0x0c => {
                let status = self.statuses.get(self.status_index).copied().unwrap_or(1);
                self.status_index += 1;
                status << 2
            }
            _ => 0,
        }
    }
}

impl ReferenceIo for CalibrationIo {
    fn read(&mut self, _width: u8, address: u32) -> u32 {
        if address == 0x2010f820 { return 0; }
        if !matches!(address, 0x2010f800 | 0x2010f804) { return 0; }
        match self.read_phase {
            2 => { self.read_phase = 1; 0 }
            1 => { self.read_phase = 0; u32::from(self.read_data()) << 16 }
            _ => 0,
        }
    }

    fn write(&mut self, _width: u8, address: u32, value: u32) {
        if matches!(address, 0x2010f800 | 0x2010f804)
            && value & 0x07000000 == 0x04000000
        {
            self.read_register = ((value >> 8) & 0xff) as u8;
            self.read_phase = 2;
        }
    }

    fn delay_micros(&mut self, _micros: u32) {}
    fn fence(&mut self, _fm: u8, _predecessor: u8, _successor: u8) {}
}

struct CalibrationMemory;
impl ReferenceMemory for CalibrationMemory {
    fn symbol_address(&mut self, _member: Option<&str>, _symbol: &str) -> u32 { 0 }
    fn read(&mut self, _width: u8, _address: u32) -> u32 { 0 }
    fn write(&mut self, _width: u8, _address: u32, _value: u32) {}
}

struct CalibrationPlatform;
impl ReferencePlatform for CalibrationPlatform {
    fn wifi_osi_version(&mut self) -> u32 { 9 }
    fn wifi_osi_magic(&mut self) -> u32 { 0xdeadbeaf }
    fn wifi_osi_table_size(&mut self) -> u32 { 512 }
    fn wifi_osi_env_is_chip(&mut self) -> bool { false }
    fn wifi_osi_rand(&mut self) -> u32 { 0 }
    fn wifi_osi_random(&mut self) -> u32 { 0 }
    fn wifi_osi_slowclk_cal_get(&mut self) -> u32 { 0 }
    fn wifi_osi_coex_pti_get(&mut self, _event: u32) -> u8 { 0 }
    fn wifi_log(&mut self, _arguments: [u32; 6]) {}
    fn ets_printf(&mut self, _format_address: u32) {}
}

fn run_calibration(initial: u16, statuses: Vec<u8>) -> (u32, usize) {
    let mut io = CalibrationIo::new(initial, statuses);
    let mut memory = CalibrationMemory;
    let mut platform = CalibrationPlatform;
    let outcome = open_phy_reference_phy_rfpll_cap_init_cal(
        &mut io,
        &mut memory,
        &mut platform,
        Rv32ReferenceArguments { registers: [0; 8], stack: [0; 8] },
    );
    (outcome.exit_a0.unwrap(), io.status_index)
}

#[test]
fn all_candidates_are_averaged_across_both_directions() {
    assert_eq!(run_calibration(16, vec![0; 20]), (0x00100010, 20));
}

#[test]
fn no_accepted_candidate_preserves_the_initial_cap() {
    assert_eq!(run_calibration(16, vec![1; 20]), (0x00100010, 20));
}

#[test]
fn accepted_window_stops_after_its_first_rejection() {
    assert_eq!(run_calibration(16, vec![1, 0, 0, 1, 1]), (0x0010000e, 5));
}
"#;

    for symbol in ["phy_rfpll_cap_init_cal", "phy_set_rfpll_freq"] {
        let trace = catalog.trace(None, symbol, &svd).unwrap();
        assert!(trace.is_reference_eligible(), "{symbol}: {trace:#?}");
        let generated =
            generate_reference(&trace, "esp32s31_rev0_rom.elf", "fixture-sha256", None, &[])
                .unwrap();
        assert!(
            generated
                .source
                .contains("for calibration_direction0 in 0..2_u8")
        );
        assert!(
            generated
                .source
                .contains("for calibration_step0 in 0..10_u16")
        );
        assert!(generated.source.contains("calibration_sum0.wrapping_add"));
        assert_generated_reference_compiles(symbol, &generated.source);
        if symbol == "phy_rfpll_cap_init_cal" {
            assert_generated_reference_tests_run(symbol, &generated.source, calibration_tests);
        }
    }

    let symbol = catalog
        .symbols
        .iter_mut()
        .find(|symbol| symbol.address == u64::from(PHY_RFPLL_CAP_INIT_CAL_ADDRESS))
        .expect("pinned ROM must contain phy_rfpll_cap_init_cal");
    symbol.bytes[0] ^= 1;
    if let Ok(changed) = catalog.trace(None, "phy_rfpll_cap_init_cal", &svd) {
        assert!(!changed.is_reference_eligible(), "{changed:#?}");
    }
}

#[test]
fn registered_phy_contract_scopes_the_rf_frequency_scratch() {
    const PHY_SET_RF_FREQ_OFFSET_ADDRESS: u32 = 0x2f82_5c10;
    const SCRATCH_TESTS: &str = r#"
struct BackingMemory;
impl ReferenceMemory for BackingMemory {
    fn symbol_address(&mut self, _member: Option<&str>, _symbol: &str) -> u32 { 0 }
    fn read(&mut self, _width: u8, _address: u32) -> u32 { 0xaabbccdd }
    fn write(&mut self, _width: u8, _address: u32, _value: u32) {}
}

#[test]
fn scratch_round_trips_little_endian_bytes_and_delegates_disjoint_reads() {
    let mut backing = BackingMemory;
    let mut scratch = ReferenceScratchMemory::new(&mut backing, 0xfffe0000, 5);
    scratch.write(32, 0xfffe0000, 0x44332211);
    scratch.write(8, 0xfffe0004, 0x55);
    assert_eq!(scratch.read(32, 0xfffe0000), 0x44332211);
    assert_eq!(scratch.read(8, 0xfffe0004), 0x55);
    assert_eq!(scratch.read(32, 0x10000000), 0xaabbccdd);
}

#[test]
#[should_panic(expected = "read from uninitialized reference scratch")]
fn scratch_rejects_uninitialized_reads() {
    let mut backing = BackingMemory;
    let mut scratch = ReferenceScratchMemory::new(&mut backing, 0xfffe0000, 5);
    let _ = scratch.read(8, 0xfffe0000);
}

#[test]
#[should_panic(expected = "partially overlaps private scratch")]
fn scratch_rejects_partial_overlap() {
    let mut backing = BackingMemory;
    let mut scratch = ReferenceScratchMemory::new(&mut backing, 0xfffe0000, 5);
    let _ = scratch.read(16, 0xfffdffff);
}
"#;

    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("phy-trace remains under tools/phy-trace");
    let artifact = root.join("_oracles/esp32s31_rev0_rom.elf");
    let companion = root.join(
        "hil/vendor-oracle/esp32s31/target/riscv32imafc-unknown-none-elf/release/open-esp-radio-vendor-oracle-esp32s31-trace-elf",
    );
    if !artifact.exists() || !companion.exists() {
        eprintln!("private linked PHY fixtures are not installed; integration test skipped");
        return;
    }
    let svd = MmioRegisterMap::load_all(&[
        root.join("svd/esp32s31-radio.svd"),
        root.join("svd/esp32s31-platform-radio-deps.svd"),
    ])
    .unwrap();
    let mut catalog = ReferenceResolver::load_with_entry_contract(
        &artifact,
        &[companion],
        entry_contract::EntryContract::Esp32s31PhyRegistered,
    )
    .unwrap();

    for symbol in [
        "phy_set_rf_freq_offset",
        "phy_set_channel_rfpll_freq",
        "phy_set_freq",
        "phy_chip_set_chan_ana",
        "phy_dcode_cal_init",
    ] {
        let trace = catalog.trace(None, symbol, &svd).unwrap();
        assert!(trace.is_reference_eligible(), "{symbol}: {trace:#?}");
        if matches!(symbol, "phy_set_rf_freq_offset" | "phy_chip_set_chan_ana") {
            let generated =
                generate_reference(&trace, "esp32s31_rev0_rom.elf", "fixture-sha256", None, &[])
                    .unwrap();
            assert!(generated.source.contains("ReferenceScratchMemory::new"));
            assert_generated_reference_compiles(symbol, &generated.source);
            if symbol == "phy_set_rf_freq_offset" {
                assert_generated_reference_tests_run(symbol, &generated.source, SCRATCH_TESTS);
            }
        }
    }

    let symbol = catalog
        .symbols
        .iter_mut()
        .find(|symbol| symbol.address == u64::from(PHY_SET_RF_FREQ_OFFSET_ADDRESS))
        .expect("pinned ROM must contain phy_set_rf_freq_offset");
    symbol.bytes[0] ^= 1;
    if let Ok(changed) = catalog.trace(None, "phy_set_rf_freq_offset", &svd) {
        assert!(!changed.is_reference_eligible(), "{changed:#?}");
    }
}

#[test]
fn registered_phy_contract_models_the_live_iq_estimator_poll() {
    const PHY_IQ_EST_ENABLE_ADDRESS: u32 = 0x2f82_89d4;
    const IQ_ESTIMATOR_TESTS: &str = r#"
struct EstimatorIo {
    done: Vec<u32>,
    statuses: Vec<u32>,
    done_index: usize,
    status_index: usize,
    writes: Vec<(u32, u32)>,
    delays: Vec<u32>,
}

impl EstimatorIo {
    fn new(done: Vec<u32>, statuses: Vec<u32>) -> Self {
        Self {
            done,
            statuses,
            done_index: 0,
            status_index: 0,
            writes: Vec::new(),
            delays: Vec::new(),
        }
    }
}

impl ReferenceIo for EstimatorIo {
    fn read(&mut self, width: u8, address: u32) -> u32 {
        assert_eq!(width, 32);
        match address {
            0x2010044c | 0x20100450 => 0,
            0x2010047c => {
                let value = self.done.get(self.done_index).copied().unwrap_or(0x00010000);
                self.done_index += 1;
                value
            }
            0x201008d0 => {
                let value = self.statuses.get(self.status_index).copied().unwrap_or(0);
                self.status_index += 1;
                value
            }
            _ => panic!("unexpected MMIO read at {address:#010x}"),
        }
    }

    fn write(&mut self, width: u8, address: u32, value: u32) {
        assert_eq!(width, 32);
        self.writes.push((address, value));
    }

    fn delay_micros(&mut self, micros: u32) { self.delays.push(micros); }
    fn fence(&mut self, _fm: u8, _predecessor: u8, _successor: u8) {}
}

struct EstimatorMemory {
    base: u32,
    counter: u16,
}

impl ReferenceMemory for EstimatorMemory {
    fn symbol_address(&mut self, member: Option<&str>, symbol: &str) -> u32 {
        assert_eq!(member, None);
        assert_eq!(symbol, "phy_param");
        self.base
    }

    fn read(&mut self, width: u8, address: u32) -> u32 {
        assert_eq!((width, address), (16, self.base + 0x1ac));
        u32::from(self.counter)
    }

    fn write(&mut self, width: u8, address: u32, value: u32) {
        assert_eq!((width, address), (16, self.base + 0x1ac));
        self.counter = value as u16;
    }
}

struct EstimatorPlatform;
impl ReferencePlatform for EstimatorPlatform {
    fn wifi_osi_version(&mut self) -> u32 { 9 }
    fn wifi_osi_magic(&mut self) -> u32 { 0xdeadbeaf }
    fn wifi_osi_table_size(&mut self) -> u32 { 512 }
    fn wifi_osi_env_is_chip(&mut self) -> bool { false }
    fn wifi_osi_rand(&mut self) -> u32 { 0 }
    fn wifi_osi_random(&mut self) -> u32 { 0 }
    fn wifi_osi_slowclk_cal_get(&mut self) -> u32 { 0 }
    fn wifi_osi_coex_pti_get(&mut self, _event: u32) -> u8 { 0 }
    fn wifi_log(&mut self, _arguments: [u32; 6]) {}
    fn ets_printf(&mut self, _format_address: u32) {}
}

fn run_estimator(done: Vec<u32>, statuses: Vec<u32>) -> (EstimatorIo, EstimatorMemory) {
    let mut io = EstimatorIo::new(done, statuses);
    let mut memory = EstimatorMemory { base: 0x3fcd0000, counter: 0xffff };
    let mut platform = EstimatorPlatform;
    let mut registers = [0; 8];
    registers[1] = 0x12345;
    let outcome = open_phy_reference_phy_iq_est_enable(
        &mut io,
        &mut memory,
        &mut platform,
        Rv32ReferenceArguments { registers, stack: [0; 8] },
    );
    assert_eq!(outcome.exit_a0, None);
    (io, memory)
}

#[test]
fn immediate_done_does_not_sample_activity_status() {
    let (io, memory) = run_estimator(vec![0x00010000], vec![]);
    assert_eq!(memory.counter, 0);
    assert_eq!((io.done_index, io.status_index), (1, 0));
    assert_eq!(io.delays, [1]);
    assert_eq!(
        io.writes,
        [
            (0x2010044c, 0x04000000),
            (0x20100450, 0x00100000),
            (0x20100450, 0x00008d14),
            (0x20100450, 0x00000001),
            (0x20100450, 0x00000002),
        ]
    );
}

#[test]
fn live_reads_increment_only_on_active_not_done_iterations() {
    let (io, memory) = run_estimator(
        vec![0, 0, 0x00010000],
        vec![0, 0x00100000],
    );
    assert_eq!(memory.counter, 1);
    assert_eq!((io.done_index, io.status_index), (3, 2));
}
"#;

    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("phy-trace remains under tools/phy-trace");
    let artifact = root.join("_oracles/esp32s31_rev0_rom.elf");
    let companion = root.join(
        "hil/vendor-oracle/esp32s31/target/riscv32imafc-unknown-none-elf/release/open-esp-radio-vendor-oracle-esp32s31-trace-elf",
    );
    if !artifact.exists() || !companion.exists() {
        eprintln!("private linked PHY fixtures are not installed; integration test skipped");
        return;
    }
    let svd = MmioRegisterMap::load_all(&[
        root.join("svd/esp32s31-radio.svd"),
        root.join("svd/esp32s31-platform-radio-deps.svd"),
    ])
    .unwrap();
    let mut catalog = ReferenceResolver::load_with_entry_contract(
        &artifact,
        &[companion],
        entry_contract::EntryContract::Esp32s31PhyRegistered,
    )
    .unwrap();

    let trace = catalog.trace(None, "phy_iq_est_enable", &svd).unwrap();
    assert!(trace.is_reference_eligible(), "{trace:#?}");
    let generated =
        generate_reference(&trace, "esp32s31_rev0_rom.elf", "fixture-sha256", None, &[]).unwrap();
    assert!(generated.source.contains("Poll a complete composed flow"));
    assert!(
        generated
            .source
            .contains("memory.symbol_address(None, \"phy_param\")")
    );
    assert!(generated.source.contains("io.read(32, 0x2010047c_u32)"));
    assert!(generated.source.contains("io.read(32, 0x201008d0_u32)"));
    assert_generated_reference_compiles("phy_iq_est_enable", &generated.source);
    assert_generated_reference_tests_run(
        "phy_iq_est_enable",
        &generated.source,
        IQ_ESTIMATOR_TESTS,
    );

    let symbol = catalog
        .symbols
        .iter_mut()
        .find(|symbol| symbol.address == u64::from(PHY_IQ_EST_ENABLE_ADDRESS))
        .expect("pinned ROM must contain phy_iq_est_enable");
    symbol.bytes[0] ^= 1;
    if let Ok(changed) = catalog.trace(None, "phy_iq_est_enable", &svd) {
        assert!(!changed.is_reference_eligible(), "{changed:#?}");
    }
}

#[test]
fn structural_polling_recognizes_real_rom_backedges() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("phy-trace remains under tools/phy-trace");
    let artifact = root.join("_oracles/esp32s31_rev0_rom.elf");
    let companion = root.join(
        "hil/vendor-oracle/esp32s31/target/riscv32imafc-unknown-none-elf/release/open-esp-radio-vendor-oracle-esp32s31-trace-elf",
    );
    if !artifact.exists() || !companion.exists() {
        eprintln!("private linked PHY fixtures are not installed; integration test skipped");
        return;
    }
    let svd = MmioRegisterMap::load_all(&[
        root.join("svd/esp32s31-radio.svd"),
        root.join("svd/esp32s31-platform-radio-deps.svd"),
    ])
    .unwrap();
    let catalog = ReferenceResolver::load_with_entry_contract(
        &artifact,
        &[companion],
        entry_contract::EntryContract::Esp32s31PhyRegistered,
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
        .expect("phy-trace remains under tools/phy-trace");
    let artifact = root.join("_oracles/esp32s31_rev0_rom.elf");
    if !artifact.exists() {
        eprintln!("private ROM fixture is not installed; integration test skipped");
        return;
    }
    let svd = MmioRegisterMap::load_all(&[
        root.join("svd/esp32s31-radio.svd"),
        root.join("svd/esp32s31-platform-radio-deps.svd"),
    ])
    .unwrap();
    let catalog = ReferenceResolver::load(&artifact, &[]).unwrap();
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
        memory_regions: Vec::new(),
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
        &StructuralPointerContext::default(),
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
            function: external_abi::Function::Rand,
            ..
        }]
    ));
}

#[test]
fn composite_svd_catalog_resolves_platform_owned_radio_dependencies() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("phy-trace remains under tools/phy-trace");
    let map = MmioRegisterMap::load_all(&[
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
            map.register_name(address),
            expected,
            "address {address:#010x}"
        );
    }

    assert_eq!(map.register_name(0x2010_9c18), "MODEM_SYSCON.WIFI_BB_CFG");
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
            map.register_name(address),
            format!("MODEM_LPCON.{register}"),
            "MODEM_LPCON address {address:#010x}"
        );
    }
    assert_eq!(map.register_name(0x2010_f800), "I2C_ANA_MST.I2C0_CTRL");
    assert_eq!(map.register_name(0x2010_f824), "I2C_ANA_MST.I2C0_CTRL1");
    assert_eq!(map.register_name(0x2010_f828), "I2C_ANA_MST.I2C1_CTRL1");
    assert_eq!(map.register_name(0x2010_f82c), "I2C_ANA_MST.HW_I2C_CTRL");
    assert_eq!(
        map.register_name(0x2070_1068),
        "LP_AON_CLKRST.RTC_SAR2_PWDET_CCT"
    );
    assert_eq!(map.register_name(0x2070_401c), "PMU.HP_ACTIVE_HP_CK_POWER");
    assert_eq!(map.register_name(0x2070_40f0), "PMU.IMM_HP_CK_POWER_0");
    assert_eq!(map.register_name(0x2070_4184), "PMU.RF_PWC");
    assert_eq!(map.register_name(0x2070_4208), "PMU.ANA_PERI_PWR_CTRL");
    assert_eq!(map.register_name(0x2071_0030), "LP_PERICLKRST.TSENS_CTRL");
    assert_eq!(map.register_name(0x2081_8000), "LP_TSENS.CTRL");
    assert_eq!(map.register_name(0x2081_8018), "LP_TSENS.CLK_CONF");
}

#[test]
fn vendor_provenance_requires_the_complete_artifact_digest() {
    assert!(is_pinned_vendor_digest(ESP32S31_LINKED_LIBPHY_SHA256));
    assert!(is_pinned_vendor_digest(ESP32S31_LIBPHY_SHA256));
    assert!(!is_pinned_vendor_digest(
        "0000000000000000000000000000000000000000000000000000000000000000"
    ));
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
