use super::super::*;
use crate::harnesses::esp32s31::RISCV_HARNESS;

#[test]
fn wifi_osi_rand_tail_call_resolves_from_relocation() {
    let symbol = wifi_osi_tail_symbol(0x0bc);
    let trace = trace_binary_symbol(
        &symbol,
        &map(),
        &BTreeMap::new(),
        &reviewed_wifi_osi_context(),
        None,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert_eq!(trace.return_value, SymbolicValue::ExternalResult(0));
    assert!(matches!(
        trace.reference_events.as_slice(),
        [DraftReferenceEvent::ReviewedExternalCall {
            token: 0,
            candidates,
            ..
        }] if candidates.len() == 1
            && candidates[0].contract == "esp32s31-radio-rev0::wifi-osi-v9"
            && candidates[0].name == "rand"
            && candidates[0].execution_model.as_ref().is_some_and(|model|
                model.id == "esp32s31-wifi-osi-v9.rand")
    ));

    let generated = generate_reference(
        &trace,
        "libpp.a",
        "fixture-artifact-id",
        Some("hal_mac.o"),
        &[],
    )
    .unwrap();
    assert!(generated.source.contains("pub trait ReferencePlatform"));
    assert!(generated.source.contains(
        "let external_outcome0 = platform.external_call(\"esp32s31-radio-rev0::wifi-osi-v9\", \"esp32s31-wifi-osi-v9.rand\", &[]);"
    ));
    assert!(
        generated
            .source
            .contains("let external_result0 = external_outcome0.return_words[0];")
    );
    assert!(!generated.source.contains("external_table_version"));
    assert!(!generated.source.contains("external_table_magic"));
    assert!(!generated.source.contains("external_table_size"));
    assert!(
        generated
            .source
            .contains("ReferenceOutcome { exit_a0: Some(external_result0) }")
    );
}

#[test]
fn unknown_wifi_osi_slot_fails_closed() {
    let symbol = wifi_osi_tail_symbol(0x0c0);
    let trace = trace_binary_symbol(
        &symbol,
        &map(),
        &BTreeMap::new(),
        &reviewed_wifi_osi_context(),
        None,
    )
    .unwrap();

    assert!(!trace.is_reference_eligible());
    assert!(trace.reference_blockers.iter().any(|blocker| {
        blocker.contains("unregistered-external-abi-slot") && blocker.contains("+0xc0")
    }));
}

#[test]
fn reviewed_wifi_osi_slot_is_named_without_an_execution_model() {
    let symbol = wifi_osi_tail_symbol(0x0c0);
    let mut context = reviewed_wifi_osi_context();
    context.reviewed_external_slots.insert(
        ("esp32s31-radio-rev0::wifi-osi-v9".to_owned(), 0x0c0),
        vec![ReviewedExternalCall {
            id: "fixture::wifi-osi-v9@+0xc0".to_owned(),
            contract: "fixture::wifi-osi-v9".to_owned(),
            name: "reviewed_slot".to_owned(),
            argument_types: vec!["u32".to_owned()],
            return_type: "u32".to_owned(),
            variadic: false,
            semantic_operation: None,
            replacement_hint: None,
            execution_model: None,
            tail: true,
            evidence: ReviewedExternalCallEvidence::ObservedCallSite,
            slot_load_site: None,
        }],
    );

    let trace = trace_binary_symbol(&symbol, &map(), &BTreeMap::new(), &context, None).unwrap();

    assert!(!trace.is_reference_eligible());
    assert!(
        !trace
            .reference_blockers
            .iter()
            .any(|blocker| blocker.contains("unregistered-external-abi-slot"))
    );
    assert!(
        trace
            .reference_blockers
            .iter()
            .any(|blocker| blocker.contains("unmodeled-reviewed-external-call"))
    );
    assert!(matches!(
        trace.reference_events.as_slice(),
        [DraftReferenceEvent::ReviewedExternalCall { candidates, .. }]
            if candidates[0].name == "reviewed_slot"
    ));
}

#[test]
fn modeled_rtos_slot_still_rejects_an_unproven_output_pointer() {
    let symbol = wifi_osi_tail_symbol(0x068);
    let trace = trace_binary_symbol(
        &symbol,
        &map(),
        &BTreeMap::new(),
        &reviewed_wifi_osi_context(),
        None,
    )
    .unwrap();

    assert!(!trace.is_reference_eligible());
    assert_eq!(trace.return_value, SymbolicValue::Unknown);
    assert!(matches!(
        trace.reference_events.as_slice(),
        [DraftReferenceEvent::ReviewedExternalCall { candidates, .. }]
            if candidates.len() == 1
                && candidates[0].semantic_operation.as_deref()
                    == Some("rtos.queue.send-from-isr")
                && candidates[0].execution_model.as_ref().is_some_and(|model|
                    model.return_model == ExternalReturnModel::SymbolicU32
                        && model.outputs == vec![ExternalOutputModel::PrivateStackU8 {
                            pointer_argument: 2
                        }])
    ));
    assert!(trace.reference_blockers.iter().any(|blocker| {
        blocker.contains("unsupported-reviewed-external-output-pointer")
            && blocker.contains("queue_send_from_isr")
    }));
}

#[test]
fn wifi_osi_output_pointer_outside_private_stack_fails_closed() {
    let symbol = wifi_osi_tail_symbol(0x1a8);
    let trace = trace_binary_symbol(
        &symbol,
        &map(),
        &BTreeMap::new(),
        &reviewed_wifi_osi_context(),
        None,
    )
    .unwrap();

    assert!(!trace.is_reference_eligible());
    assert!(
        trace.reference_blockers.iter().any(|blocker| {
            blocker.contains("unsupported-reviewed-external-output-pointer")
                && blocker.contains("coex_pti_get")
                && blocker.contains("a1")
        }),
        "{trace:#?}"
    );
}

#[test]
fn real_libpp_hal_random_resolves_through_wifi_osi_abi() {
    let artifact = private_input("OPEN_ESP_RADIO_ESP32S31_LIBPP_ARCHIVE").unwrap_or_default();
    if !artifact.exists() {
        eprintln!("private libpp fixture is not installed; integration test skipped");
        return;
    }

    let trace = reviewed_reference_resolver(&artifact, &[])
        .trace(Some("hal_mac.o"), "hal_random", &map())
        .unwrap();
    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert_eq!(trace.return_value, SymbolicValue::ExternalResult(0));
}

#[test]
fn real_libpp_pp_post_matches_the_reviewed_direct_semantic_contract() {
    let artifact = private_input("OPEN_ESP_RADIO_ESP32S31_LIBPP_ARCHIVE").unwrap_or_default();
    if !artifact.exists() {
        eprintln!("private libpp fixture is not installed; integration test skipped");
        return;
    }

    let symbols =
        artifact::load_code_symbols(&artifact, "pp_post", artifact::CodeSymbolSelection::All)
            .unwrap();
    let symbol = symbols
        .iter()
        .find(|symbol| symbol.member.as_deref() == Some("pp.o") && symbol.name == "pp_post")
        .expect("reviewed libpp fixture must contain pp.o:pp_post");
    let contract = (RISCV_HARNESS.summaries.direct_semantic)(symbol)
        .expect("reviewed libpp pp_post body and relocations must match");

    assert_eq!(contract.id, "esp32s31-libpp-pp-post-v1");
    assert_eq!(contract.semantic.operation, "wifi.internal-signal.post");
    assert_eq!(contract.semantic.arguments.len(), 1);
    assert_eq!(contract.semantic.arguments[0].name, "signal");
}

#[test]
fn real_wdev_append_rx_blocks_recognizes_wifi_assert_as_a_diagnostic_boundary() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workbench facade remains under tools");
    let artifact = private_input("OPEN_ESP_RADIO_ESP32S31_LIBPP_ARCHIVE").unwrap_or_default();
    if !artifact.exists() {
        eprintln!("private libpp fixture is not installed; integration test skipped");
        return;
    }
    let svd = MmioMap::load_all(&[
        root.join("svd/esp32s31-radio.svd"),
        root.join("svd/esp32s31-platform-radio-deps.svd"),
    ])
    .unwrap();

    let trace = reviewed_reference_resolver(&artifact, &[])
        .trace(Some("wdev.o"), "wDev_AppendRxBlocks", &svd)
        .unwrap();

    assert!(!trace.is_reference_eligible(), "{trace:#?}");
    assert!(
        trace.reference_blockers.iter().all(|blocker| {
            !(blocker.contains("unresolved-call-relocation") && blocker.contains("wifi_assert"))
        }),
        "wifi_assert must be a known diagnostic boundary, not an unknown ABI: {trace:#?}"
    );
    assert!(
        trace
            .reference_blockers
            .iter()
            .any(|blocker| blocker.contains("symbolic-cfg")
                && blocker.contains("unmodeled-memory")),
        "the remaining boundary must be caller-owned descriptor/state memory: {trace:#?}"
    );
}

#[test]
fn real_libpp_coex_output_bytes_reach_compilable_reference_codegen() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workbench facade remains under tools");
    let artifact = private_input("OPEN_ESP_RADIO_ESP32S31_LIBPP_ARCHIVE").unwrap_or_default();
    if !artifact.exists() {
        eprintln!("private libpp fixture is not installed; integration test skipped");
        return;
    }
    let svd = MmioMap::load_all(&[
        root.join("svd/esp32s31-radio.svd"),
        root.join("svd/esp32s31-platform-radio-deps.svd"),
    ])
    .unwrap();
    let trace = reviewed_reference_resolver(&artifact, &[])
        .trace(Some("hal_coex.o"), "hal_set_ofdma_sequence_pti", &svd)
        .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert_eq!(
        trace
            .reference_events
            .iter()
            .filter(|event| matches!(event, DraftReferenceEvent::ReviewedExternalCall { .. }))
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
        "fixture-artifact-id",
        Some("hal_coex.o"),
        &[],
    )
    .unwrap();
    assert_eq!(
        generated
            .source
            .matches("\"esp32s31-wifi-osi-v9.coex-pti-get\"")
            .count(),
        13
    );
    assert_generated_reference_compiles("hal_set_ofdma_sequence_pti", &generated.source);
}

#[test]
fn real_libpp_coex_runtime_leaves_generate_compilable_references() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workbench facade remains under tools");
    let artifact = private_input("OPEN_ESP_RADIO_ESP32S31_LIBPP_ARCHIVE").unwrap_or_default();
    if !artifact.exists() {
        eprintln!("private libpp fixture is not installed; integration test skipped");
        return;
    }
    let svd = MmioMap::load_all(&[
        root.join("svd/esp32s31-radio.svd"),
        root.join("svd/esp32s31-platform-radio-deps.svd"),
    ])
    .unwrap();
    let catalog = reviewed_reference_resolver(&artifact, &[]);

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
            "fixture-artifact-id",
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
        .expect("workbench facade remains under tools");
    let artifact = private_input("OPEN_ESP_RADIO_ESP32S31_LIBPP_ARCHIVE").unwrap_or_default();
    if !artifact.exists() {
        eprintln!("private libpp fixture is not installed; integration test skipped");
        return;
    }
    let svd = MmioMap::load_all(&[
        root.join("svd/esp32s31-radio.svd"),
        root.join("svd/esp32s31-platform-radio-deps.svd"),
    ])
    .unwrap();
    let catalog = reviewed_reference_resolver(&artifact, &[]);

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
            generate_reference(&trace, "libpp.a", "fixture-artifact-id", Some(member), &[])
                .unwrap();
        assert_generated_reference_compiles(symbol, &generated.source);
    }
}

#[test]
fn real_libpp_remaining_mmio_leaves_generate_compilable_references() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workbench facade remains under tools");
    let artifact = private_input("OPEN_ESP_RADIO_ESP32S31_LIBPP_ARCHIVE").unwrap_or_default();
    if !artifact.exists() {
        eprintln!("private libpp fixture is not installed; integration test skipped");
        return;
    }
    let svd = MmioMap::load_all(&[
        root.join("svd/esp32s31-radio.svd"),
        root.join("svd/esp32s31-platform-radio-deps.svd"),
    ])
    .unwrap();
    let catalog = reviewed_reference_resolver(&artifact, &[]);

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
            generate_reference(&trace, "libpp.a", "fixture-artifact-id", Some(member), &[])
                .unwrap();
        assert_generated_reference_compiles(symbol, &generated.source);
    }
}

#[test]
fn real_libpp_timer_update_generates_both_symbolic_cfg_paths() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workbench facade remains under tools");
    let artifact = private_input("OPEN_ESP_RADIO_ESP32S31_LIBPP_ARCHIVE").unwrap_or_default();
    if !artifact.exists() {
        eprintln!("private libpp fixture is not installed; integration test skipped");
        return;
    }
    let svd = MmioMap::load_all(&[
        root.join("svd/esp32s31-radio.svd"),
        root.join("svd/esp32s31-platform-radio-deps.svd"),
    ])
    .unwrap();

    let trace = reviewed_reference_resolver(&artifact, &[])
        .trace(Some("hal_tsf.o"), "hal_timer_update_by_rtc", &svd)
        .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert!(!trace.is_exact());
    assert!(trace.reference_flow.is_some());
    let generated = generate_reference(
        &trace,
        "libpp.a",
        "fixture-artifact-id",
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
        .expect("workbench facade remains under tools");
    let artifact = private_input("OPEN_ESP_RADIO_ESP32S31_LIBPP_ARCHIVE").unwrap_or_default();
    if !artifact.exists() {
        eprintln!("private libpp fixture is not installed; integration test skipped");
        return;
    }
    let svd = MmioMap::load_all(&[
        root.join("svd/esp32s31-radio.svd"),
        root.join("svd/esp32s31-platform-radio-deps.svd"),
    ])
    .unwrap();
    let catalog = reviewed_reference_resolver(&artifact, &[]);

    for (member, symbol) in [
        ("hal_mac.o", "hal_mac_is_txq_valid"),
        ("hal_mac.o", "hal_mac_clr_bssid"),
        ("hal_mac_ctl.o", "hal_he_set_ac_muedca_param"),
    ] {
        let trace = catalog.trace(Some(member), symbol, &svd).unwrap();
        assert!(trace.is_reference_eligible(), "{symbol}: {trace:#?}");
        let generated =
            generate_reference(&trace, "libpp.a", "fixture-artifact-id", Some(member), &[])
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
            "fixture-artifact-id",
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
        .expect("workbench facade remains under tools");
    let artifact = private_input("OPEN_ESP_RADIO_ESP32S31_LIBPP_ARCHIVE").unwrap_or_default();
    if !artifact.exists() {
        eprintln!("private libpp fixture is not installed; integration test skipped");
        return;
    }
    let svd = MmioMap::load_all(&[
        root.join("svd/esp32s31-radio.svd"),
        root.join("svd/esp32s31-platform-radio-deps.svd"),
    ])
    .unwrap();
    let catalog = reviewed_reference_resolver(&artifact, &[]);

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
            generate_reference(&trace, "libpp.a", "fixture-artifact-id", Some(member), &[])
                .unwrap();
        assert_generated_reference_compiles(name, &generated.source);
    }
}

#[test]
fn real_libpp_relocated_state_accessors_generate_compilable_references() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workbench facade remains under tools");
    let artifact = private_input("OPEN_ESP_RADIO_ESP32S31_LIBPP_ARCHIVE").unwrap_or_default();
    if !artifact.exists() {
        eprintln!("private libpp fixture is not installed; integration test skipped");
        return;
    }
    let svd = MmioMap::load_all(&[
        root.join("svd/esp32s31-radio.svd"),
        root.join("svd/esp32s31-platform-radio-deps.svd"),
    ])
    .unwrap();
    let catalog = reviewed_reference_resolver(&artifact, &[]);

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
            generate_reference(&trace, "libpp.a", "fixture-artifact-id", Some(member), &[])
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
        .expect("workbench facade remains under tools");
    let artifact = private_input("OPEN_ESP_RADIO_ESP32S31_LIBPP_ARCHIVE").unwrap_or_default();
    if !artifact.exists() {
        eprintln!("private libpp fixture is not installed; integration test skipped");
        return;
    }
    let svd = MmioMap::load_all(&[
        root.join("svd/esp32s31-radio.svd"),
        root.join("svd/esp32s31-platform-radio-deps.svd"),
    ])
    .unwrap();
    let catalog = reviewed_reference_resolver(&artifact, &[]);
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
            &reviewed_wifi_osi_context(),
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
                "fixture-artifact-id",
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
    let artifact = private_input("OPEN_ESP_RADIO_ESP32S31_LIBPP_ARCHIVE").unwrap_or_default();
    if !artifact.exists() {
        eprintln!("private libpp fixture is not installed; integration test skipped");
        return;
    }

    let trace = reviewed_reference_resolver(&artifact, &[])
        .trace(Some("hal_mac_ctl.o"), "hal_he_set_mac_delay", &map())
        .unwrap();
    let functions = trace
        .reference_events
        .iter()
        .flat_map(|event| match event {
            DraftReferenceEvent::ReviewedExternalCall { candidates, .. } => candidates
                .iter()
                .map(|candidate| candidate.name.as_str())
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        })
        .collect::<Vec<_>>();
    assert!(functions.starts_with(&["env_is_chip", "random"]));
    assert!(trace.reference_blockers.iter().all(|blocker| {
        !blocker.contains("esp32s31-wifi-osi-v9+0x4")
            && !blocker.contains("esp32s31-wifi-osi-v9+0x144")
    }));
}

#[test]
fn linked_rom_catalog_discovers_wifi_osi_pointer_cell_by_symbol() {
    let artifact = private_input("OPEN_ESP_RADIO_ESP32S31_ROM_ELF").unwrap_or_default();
    if !artifact.exists() {
        eprintln!("private ROM fixture is not installed; integration test skipped");
        return;
    }

    let catalog = reviewed_reference_resolver(&artifact, &[]);
    assert_eq!(
        catalog
            .pointer_context
            .reviewed_external_pointer_cells
            .get(&0x2f07_ff44),
        Some(&"esp32s31-radio-rev0::wifi-osi-v9".to_owned())
    );
}
