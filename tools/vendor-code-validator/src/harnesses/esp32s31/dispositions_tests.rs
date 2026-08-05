use super::*;
use open_radio_vendor_validator_semantic::Timeout;

#[test]
fn checked_in_manifest_is_strict_and_resolves_defaults() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("validator remains under tools");
    let path = root.join("validation/esp32s31/dispositions/phy.disposition");
    let manifest = Manifest::load(&path).unwrap();
    assert_eq!(manifest.entries().count(), 9);

    let disable_agc = manifest.resolve("rom", "phy_disable_agc");
    assert_eq!(disable_agc.disposition, Disposition::Direct);
    let effect_contract = disable_agc.entry.unwrap().effect_contract.as_ref().unwrap();
    assert_eq!(effect_contract.comparison, EffectComparison::ExactEffectsV1);
    assert_eq!(effect_contract.rules().count(), 2);
    let binding = disable_agc.entry.unwrap().binding.as_ref().unwrap();
    assert_eq!(binding.version, BindingVersion::V1);
    assert_eq!(binding.rust_probe, "open_phy_trace_disable_agc");
    assert_eq!(binding.driver_adapter, None);

    let iq_enable = manifest.resolve("rom", "phy_iq_est_enable");
    assert_eq!(iq_enable.disposition, Disposition::StateTransition);
    let iq_enable = iq_enable.entry.unwrap();
    assert_eq!(
        iq_enable
            .binding
            .as_ref()
            .unwrap()
            .driver_adapter
            .as_ref()
            .unwrap()
            .label(),
        "esp32s31-iq-est-enable-v1"
    );
    assert_eq!(
        iq_enable.effect_contract.as_ref().unwrap().rules().count(),
        9
    );

    let root = manifest.resolve("archive", "register_chipv7_phy");
    assert_eq!(root.disposition, Disposition::ReplacedByComposition);
    assert_eq!(root.protocol, Protocol::Shared);
    let root = root.entry.unwrap();
    assert!(root.rust_component.is_some());
    assert_eq!(
        root.qualification_blockers,
        [("archive".to_owned(), "phy_bb_init".to_owned())]
    );

    let bb_init = manifest.resolve("archive", "phy_bb_init");
    assert_eq!(
        bb_init.entry.unwrap().qualification_blockers,
        [("archive".to_owned(), "phy_bt_tx_gain_init".to_owned())]
    );

    let channel = manifest.resolve("archive", "phy_chip_set_chan");
    assert_eq!(
        channel
            .entry
            .unwrap()
            .semantic_contract
            .as_ref()
            .unwrap()
            .label(),
        "esp32s31-channel"
    );

    let rf_init = manifest.resolve("archive", "phy_rf_init");
    assert_eq!(
        rf_init
            .entry
            .unwrap()
            .semantic_contract
            .as_ref()
            .unwrap()
            .label(),
        "esp32s31-rf-init"
    );

    let bluetooth_txdc = manifest.resolve("archive", "phy_bt_txdc_cal_new");
    assert_eq!(
        bluetooth_txdc
            .entry
            .unwrap()
            .semantic_contract
            .as_ref()
            .unwrap()
            .label(),
        "esp32s31-bluetooth-txdc"
    );

    let bluetooth_tx_power = manifest.resolve("archive", "phy_bt_tx_pwctrl_init");
    assert_eq!(
        bluetooth_tx_power
            .entry
            .unwrap()
            .semantic_contract
            .as_ref()
            .unwrap()
            .label(),
        "esp32s31-bluetooth-tx-power"
    );

    let bluetooth_txdc_pwdet = manifest.resolve("archive", "phy_txdc_cal_pwdet_init");
    assert_eq!(
        bluetooth_txdc_pwdet
            .entry
            .unwrap()
            .semantic_contract
            .as_ref()
            .unwrap()
            .label(),
        "esp32s31-bluetooth-txdc-pwdet"
    );

    let bluetooth = manifest.resolve("rom", "phy_bt_filter_reg");
    assert_eq!(bluetooth.disposition, Disposition::NotYetPorted);
    assert_eq!(bluetooth.protocol, Protocol::Bluetooth);

    let unknown = manifest.resolve("rom", "phy_unclassified_example");
    assert_eq!(unknown.disposition, Disposition::NotYetPorted);
    assert_eq!(unknown.protocol, Protocol::Unknown);
}

#[test]
fn libpp_interrupt_bindings_require_exact_return_values() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("validator remains under tools");
    let manifest =
        Manifest::load(&root.join("validation/esp32s31/dispositions/libpp-interrupt.disposition"))
            .unwrap();

    for symbol in ["hal_mac_interrupt_get_event", "hal_mac_interrupt_clr_event"] {
        let binding = manifest
            .resolve("libpp", symbol)
            .entry
            .unwrap()
            .binding
            .as_ref()
            .unwrap();
        assert!(binding.compare_return, "{symbol}");
    }
    assert!(
        !manifest
            .resolve("libpp", "wDev_ProcessFiq")
            .entry
            .unwrap()
            .binding
            .as_ref()
            .unwrap()
            .compare_return
    );
}

#[test]
fn libpp_power_interrupt_manifest_keeps_status_and_clear_disjoint() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("validator remains under tools");
    let manifest = Manifest::load(
        &root.join("validation/esp32s31/dispositions/libpp-power-interrupt.disposition"),
    )
    .unwrap();

    assert_eq!(manifest.entries().count(), 2);
    let status = manifest.resolve("libpp", "hal_pwr_interrupt_get_event");
    let status = status.entry.unwrap();
    assert!(status.binding.as_ref().unwrap().compare_return);
    assert!(status.effect_contract.as_ref().unwrap().rules().any(
        |(selector, disposition)| matches!(
            (selector, disposition),
            (
                EffectSelector::MmioRead {
                    width: 32,
                    address: 0x2010_d8bc,
                },
                EffectDisposition::Required,
            )
        )
    ));

    let clear = manifest.resolve("libpp", "hal_pwr_interrupt_clr_event");
    let clear = clear.entry.unwrap();
    assert!(!clear.binding.as_ref().unwrap().compare_return);
    assert!(clear.effect_contract.as_ref().unwrap().rules().any(
        |(selector, disposition)| matches!(
            (selector, disposition),
            (
                EffectSelector::MmioWrite {
                    width: 32,
                    address: 0x2010_d8c0,
                },
                EffectDisposition::Required,
            )
        )
    ));
}

#[test]
fn libpp_tx_dma_manifest_covers_every_ordinary_queue_register() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("validator remains under tools");
    let manifest =
        Manifest::load(&root.join("validation/esp32s31/dispositions/libpp-tx-dma.disposition"))
            .unwrap();

    assert_eq!(manifest.entries().count(), 7);
    for symbol in [
        "hal_mac_is_txq_enabled",
        "hal_mac_is_txq_valid",
        "hal_mac_set_txq_invalid",
        "hal_mac_txq_disable",
        "hal_mac_txq_enable",
    ] {
        let policy = manifest
            .resolve("libpp", symbol)
            .entry
            .unwrap()
            .effect_contract
            .as_ref()
            .unwrap();
        for address in [0x2010_4d40, 0x2010_4d50, 0x2010_4d60, 0x2010_4d70] {
            assert!(
                policy.rules().any(|(selector, _)| matches!(
                    selector,
                    EffectSelector::MmioRead { address: actual, .. }
                        | EffectSelector::MmioWrite { address: actual, .. }
                        if *actual == address
                )),
                "{symbol} omits {address:#010x}"
            );
        }
    }
    assert_eq!(
        manifest
            .resolve("libpp", "hal_mac_txq_enable")
            .entry
            .unwrap()
            .binding
            .as_ref()
            .unwrap()
            .driver_adapter
            .as_ref()
            .unwrap()
            .label(),
        "esp32s31-hal-mac-txq-enable-register-slice-v1"
    );
}

#[test]
fn libpp_rx_dma_manifest_covers_the_finite_ring_register_boundary() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("validator remains under tools");
    let manifest =
        Manifest::load(&root.join("validation/esp32s31/dispositions/libpp-rx-dma.disposition"))
            .unwrap();

    assert_eq!(manifest.entries().count(), 9);
    for symbol in [
        "hal_mac_rx_disable",
        "hal_mac_rx_enable",
        "hal_mac_rx_read_rxdscrlast",
        "hal_mac_rx_read_rxdscrnext",
        "hal_mac_rx_set_base",
        "hal_mac_rx_get_last_dscr",
        "hal_mac_rx_is_dscr_reload",
        "hal_mac_rx_set_dscr_reload",
    ] {
        let resolved = manifest.resolve("libpp", symbol);
        assert_eq!(resolved.disposition, Disposition::Direct, "{symbol}");
        let entry = resolved.entry.unwrap();
        assert_eq!(entry.protocol, Some(Protocol::Wifi), "{symbol}");
        assert!(entry.effect_contract.is_some(), "{symbol}");
        assert_eq!(entry.binding.as_ref().unwrap().version, BindingVersion::V1);
    }

    for symbol in [
        "hal_mac_rx_read_rxdscrlast",
        "hal_mac_rx_read_rxdscrnext",
        "hal_mac_rx_get_last_dscr",
        "hal_mac_rx_is_dscr_reload",
    ] {
        assert!(
            manifest
                .resolve("libpp", symbol)
                .entry
                .unwrap()
                .binding
                .as_ref()
                .unwrap()
                .compare_return,
            "{symbol}",
        );
    }

    let append = manifest.resolve("libpp", "wDev_AppendRxBlocks");
    assert_eq!(append.disposition, Disposition::ReplacedByComposition);
    let append = append.entry.unwrap();
    assert_eq!(append.protocol, Some(Protocol::Wifi));
    assert_eq!(
        append
            .binding
            .as_ref()
            .unwrap()
            .driver_adapter
            .as_ref()
            .unwrap()
            .label(),
        "esp32s31-wdev-append-rx-blocks-v1"
    );
    let policy = append.effect_contract.as_ref().unwrap();
    assert!(policy.rules().any(|(selector, disposition)| {
        matches!(
            (selector, disposition),
            (
                EffectSelector::MmioRead {
                    width: 32,
                    address: 0x2010_4080,
                },
                EffectDisposition::ReplacedByAsync {
                    timeout: Timeout::Attempts(0x186a1),
                    ..
                }
            )
        )
    }));
}

#[test]
fn libpp_modem_wakeup_manifest_closes_each_selected_register_leaf() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("validator remains under tools");
    let manifest = Manifest::load(
        &root.join("validation/esp32s31/dispositions/libpp-modem-wakeup.disposition"),
    )
    .unwrap();

    let leaves = [
        ("pwr_hal_set_mac_modem_beacon_miss_timeout", 0x2010_d854),
        ("pwr_hal_set_mac_modem_beacon_miss_limit", 0x2010_d838),
        (
            "pwr_hal_set_mac_modem_beacon_miss_limit_exceeded_wakeup_enable",
            0x2010_d838,
        ),
        ("pwr_hal_set_mac_modem_state_sleep_limit", 0x2010_d838),
        (
            "pwr_hal_set_mac_modem_state_sleep_limit_exceeded_wakeup_enable",
            0x2010_d838,
        ),
        (
            "pwr_hal_set_mac_modem_state_wakeup_protect_enable",
            0x2010_d858,
        ),
        (
            "pwr_hal_set_mac_modem_state_wakeup_protect_early_time",
            0x2010_d83c,
        ),
        ("pwr_hal_set_mac_modem_tbtt_auto_period_enable", 0x2010_d83c),
        (
            "pwr_hal_set_mac_modem_tbtt_auto_period_disable",
            0x2010_d83c,
        ),
        (
            "pwr_hal_set_mac_modem_tbtt_auto_period_interval",
            0x2010_d83c,
        ),
    ];
    assert_eq!(manifest.entries().count(), leaves.len());

    for (symbol, address) in leaves {
        let resolved = manifest.resolve("libpp", symbol);
        assert_eq!(resolved.disposition, Disposition::Direct, "{symbol}");
        let entry = resolved.entry.unwrap();
        assert_eq!(entry.protocol, Some(Protocol::Wifi), "{symbol}");
        let policy = entry.effect_contract.as_ref().unwrap();
        assert_eq!(policy.comparison, EffectComparison::ExactEffectsV1);
        assert_eq!(policy.rules().count(), 2, "{symbol}");
        for write in [false, true] {
            assert!(
                policy.rules().any(|(selector, disposition)| {
                    *disposition == EffectDisposition::Required
                        && matches!(
                            (selector, write),
                            (
                                EffectSelector::MmioRead {
                                    width: 32,
                                    address: actual,
                                },
                                false,
                            ) | (
                                EffectSelector::MmioWrite {
                                    width: 32,
                                    address: actual,
                                },
                                true,
                            ) if *actual == address
                        )
                }),
                "{symbol} omits {} at {address:#010x}",
                if write { "write" } else { "read" },
            );
        }
        assert!(!entry.binding.as_ref().unwrap().compare_return, "{symbol}");
    }
}

#[test]
fn libpp_sta_tsf_wakeup_manifest_requires_both_register_rmws() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("validator remains under tools");
    let manifest = Manifest::load(
        &root.join("validation/esp32s31/dispositions/libpp-sta-tsf-wakeup.disposition"),
    )
    .unwrap();

    assert_eq!(manifest.entries().count(), 1);
    let entry = manifest
        .resolve("libpp", "hal_set_sta_tsf_wakeup")
        .entry
        .unwrap();
    let policy = entry.effect_contract.as_ref().unwrap();
    assert_eq!(policy.comparison, EffectComparison::ExactEffectsV1);
    assert_eq!(policy.rules().count(), 4);
    for address in [0x2010_d858, 0x2010_d830] {
        for write in [false, true] {
            assert!(policy.rules().any(|(selector, disposition)| {
                *disposition == EffectDisposition::Required
                    && matches!(
                        (selector, write),
                        (
                            EffectSelector::MmioRead {
                                width: 32,
                                address: actual,
                            },
                            false,
                        ) | (
                            EffectSelector::MmioWrite {
                                width: 32,
                                address: actual,
                            },
                            true,
                        ) if *actual == address
                    )
            }));
        }
    }
}

#[test]
fn rom_sta_tsf_snapshot_manifest_names_the_complete_coherent_transaction() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("validator remains under tools");
    let manifest = Manifest::load(
        &root.join("validation/esp32s31/dispositions/rom-sta-tsf-snapshot.disposition"),
    )
    .unwrap();

    assert_eq!(manifest.entries().count(), 1);
    let entry = manifest.resolve("rom", "hal_get_sta_tsf").entry.unwrap();
    assert_eq!(
        entry.rust_component.as_deref(),
        Some("open_esp_radio_esp32s31_registers::RadioRegisters::station_tsf")
    );
    assert_eq!(
        entry.binding.as_ref().unwrap().rust_probe,
        "open_rom_power_tsf_trace_hal_get_sta_tsf"
    );
    let policy = entry.effect_contract.as_ref().unwrap();
    assert_eq!(policy.comparison, EffectComparison::ExactEffectsV1);
    for address in [0x2010_d814, 0x2010_d820, 0x2010_d824] {
        assert!(policy.rules().any(|(selector, disposition)| {
            *disposition == EffectDisposition::Required
                && matches!(
                    selector,
                    EffectSelector::MmioRead {
                        width: 32,
                        address: actual,
                    } if *actual == address
                )
        }));
    }
}

#[test]
fn libnet80211_sta_join_manifest_is_an_explicit_architectural_replacement() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("validator remains under tools");
    let manifest = Manifest::load(
        &root.join("validation/esp32s31/dispositions/libnet80211-sta-join.disposition"),
    )
    .unwrap();

    assert_eq!(manifest.entries().count(), 1);
    let state = manifest.resolve("libnet80211", "ieee80211_sta_new_state");
    assert_eq!(state.disposition, Disposition::ReplacedByComposition);
    let state = state.entry.unwrap();
    assert_eq!(state.protocol, Some(Protocol::Wifi));
    assert_eq!(state.effect_contract.as_ref().unwrap().rules().count(), 13);
    assert_eq!(
        state
            .binding
            .as_ref()
            .unwrap()
            .driver_adapter
            .as_ref()
            .unwrap()
            .label(),
        "esp32s31-sta-join-state-v1"
    );
}

#[test]
fn manifest_accepts_arbitrary_stable_source_ids() {
    for source in ["rom", "libphy", "libpp", "libnet80211", "libwpa"] {
        assert_eq!(validate_source_id(source, 1).unwrap(), source);
    }
    for invalid in ["", "LibPP", "1libpp", "lib/pp", "libpp.a"] {
        assert!(validate_source_id(invalid, 1).is_err());
    }
}
