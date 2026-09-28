use super::*;

#[test]
fn gatt_beside_the_correctness_wifi_image_is_the_joint_image() {
    // The features the joint image advertises on hardware.
    let joint = FeatureCapabilities {
        bidirectional: true,
        bluetooth_gatt: true,
        data_plane_placement: true,
        driver_observation_evidence: true,
        psram_task_stack: true,
        runtime_configuration: true,
        runtime_initialization: true,
        rx: true,
        simultaneous_station_access_point: true,
        startup_artifact: true,
        station_epoch_control: true,
        station_lifecycle_events: true,
        structured_evidence: true,
        tcp: true,
        timebase_probe: true,
        tx: true,
        udp: true,
        udp_multi_flow: true,
        wifi_access_point: true,
        wifi_monitor_capture: true,
        wifi_role_control: true,
        ..Default::default()
    };
    assert_eq!(
        classify_flashed_capabilities(&joint),
        Some(ImageClass::WifiBleCoex)
    );
    // GATT beside any other Wi-Fi image is no class.
    let performance = FeatureCapabilities {
        driver_observation_evidence: false,
        ..joint
    };
    assert_eq!(classify_flashed_capabilities(&performance), None);
    let diagnostic = FeatureCapabilities {
        task_poll_evidence: true,
        ..joint
    };
    assert_eq!(classify_flashed_capabilities(&diagnostic), None);
}

#[test]
fn secure_gatt_never_classifies_as_plaintext_or_diagnostic_host() {
    let features = FeatureCapabilities {
        bluetooth_secure_gatt: true,
        structured_evidence: true,
        psram_task_stack: true,
        ..Default::default()
    };
    assert_eq!(
        classify_flashed_capabilities(&features),
        Some(ImageClass::BluetoothSecureGatt)
    );
    for mixed in [
        FeatureCapabilities {
            bluetooth_gatt: true,
            ..features
        },
        FeatureCapabilities {
            bluetooth_dtm: true,
            ..features
        },
    ] {
        assert!(classify_flashed_capabilities(&mixed).is_none());
    }
    assert!(!ImageClass::BluetoothSecureGatt.requires_driver_observation());
}

#[test]
fn trouble_gatt_has_one_host_and_rejects_diagnostic_capabilities() {
    let features = FeatureCapabilities {
        bluetooth_gatt: true,
        structured_evidence: true,
        psram_task_stack: true,
        ..FeatureCapabilities::default()
    };
    assert_eq!(
        classify_flashed_capabilities(&features),
        Some(ImageClass::BluetoothGatt)
    );
    assert!(
        classify_flashed_capabilities(&FeatureCapabilities {
            bluetooth_dtm: true,
            ..features
        })
        .is_none()
    );
    assert!(!ImageClass::BluetoothGatt.requires_driver_observation());
    assert_eq!(
        ImageClass::BluetoothGatt.runtime_features(),
        "bluetooth-gatt"
    );
}

#[test]
fn radio_observer_placement_remains_required_only_for_radio_compositions() {
    let symbols = [
        ("runtime::RX_PIPELINE", 0x2000),
        ("runtime::AGGREGATE_TX", 0x2100),
        ("runtime::MAC_IRQ", 0x2200),
        ("runtime::TASK_POLLS", 0x2300),
    ];
    for class in [ImageClass::Correctness, ImageClass::DiagnosticTaskPoll] {
        assert!(audit_radio_observers(class, Some(0x2000..0x2400), symbols.into_iter()).is_ok());
        assert!(audit_radio_observers(class, None, symbols.into_iter()).is_err());
        for index in 0..symbols.len() {
            let missing = symbols
                .iter()
                .enumerate()
                .filter_map(|(at, value)| (at != index).then_some(*value));
            let error = audit_radio_observers(class, Some(0x2000..0x2400), missing).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains(symbols[index].0.trim_start_matches("runtime::"))
            );
            for address in [0x1fff, 0x2400] {
                let mut misplaced = symbols;
                misplaced[index].1 = address;
                assert!(
                    audit_radio_observers(class, Some(0x2000..0x2400), misplaced.into_iter())
                        .is_err()
                );
            }
        }
    }
    for class in [
        ImageClass::Performance,
        ImageClass::DiagnosticTaskResidence,
        ImageClass::DiagnosticTxArchitecture,
        ImageClass::DiagnosticCore0RxCoarse,
        ImageClass::DiagnosticIeee802154EventStatus,
        ImageClass::DiagnosticIeee802154EdEvent,
    ] {
        let without_aggregate = symbols
            .into_iter()
            .filter(|(name, _)| !name.ends_with("AGGREGATE_TX"));
        assert!(audit_radio_observers(class, Some(0x2000..0x2400), without_aggregate).is_ok());
        let mut misplaced = symbols;
        misplaced[1].1 = 0x2400;
        assert!(audit_radio_observers(class, Some(0x2000..0x2400), misplaced.into_iter()).is_err());
        let missing_rx = symbols
            .into_iter()
            .filter(|(name, _)| !name.ends_with("RX_PIPELINE"));
        assert!(audit_radio_observers(class, Some(0x2000..0x2400), missing_rx).is_err());
    }
    for class in [ImageClass::BootSmoke, ImageClass::DiagnosticMemoryBenchmark] {
        assert!(audit_radio_observers(class, None, std::iter::empty()).is_ok());
        assert!(audit_radio_observers(class, Some(0x2000..0x2400), std::iter::empty()).is_ok());
    }
}

fn image_signature(
    driver_observation: bool,
    task_poll: bool,
    rx_delivery: bool,
    mac_irq: bool,
    ieee802154_event_status: bool,
    ieee802154_ed_event: bool,
) -> ImageCapabilitySignature {
    ImageCapabilitySignature {
        driver_observation,
        task_poll,
        tx_architecture_probe: false,
        core0_rx_cycles: false,
        rx_delivery,
        mac_irq,
        ieee802154_event_status,
        ieee802154_ed_event,
        psram_task_stack: true,
        memory_benchmark: false,
    }
}

#[test]
fn qualified_profile_name_is_stable() {
    assert_eq!(TARGET, "riscv32imafc-unknown-none-elf");
}

#[test]
fn system_watchdog_has_only_platform_capabilities_and_no_radio_feature() {
    let features = FeatureCapabilities {
        system_watchdog: true,
        structured_evidence: true,
        psram_task_stack: true,
        ..FeatureCapabilities::default()
    };
    assert_eq!(
        classify_flashed_capabilities(&features),
        Some(ImageClass::SystemWatchdog)
    );
    for conflicting in [
        FeatureCapabilities {
            bluetooth_dtm: true,
            ..features
        },
        FeatureCapabilities {
            phy_fault_injection: true,
            ..features
        },
        FeatureCapabilities {
            runtime_initialization: true,
            ..features
        },
    ] {
        assert_eq!(classify_flashed_capabilities(&conflicting), None);
    }
    assert_eq!(
        ImageClass::SystemWatchdog.runtime_features(),
        "system-watchdog"
    );
}

#[test]
fn image_classes_are_stable_and_do_not_use_workload_environment() {
    assert_eq!(crate::image::ImageClass::ALL.len(), 25);
    assert_eq!(crate::image::ImageClass::Performance.id(), "performance");
    assert_eq!(crate::image::ImageClass::Correctness.id(), "correctness");
    assert_eq!(
        crate::image::ImageClass::WifiBleCoex.runtime_features(),
        "wifi-ble-coex"
    );
    assert_eq!(
        crate::image::ImageClass::Correctness.runtime_features(),
        "open-radio-hil,driver-observation"
    );
    assert_eq!(
        crate::image::ImageClass::DiagnosticMacIrq.runtime_features(),
        "open-radio-hil,mac-irq-telemetry"
    );
    assert_eq!(
        crate::image::ImageClass::DiagnosticTaskResidence.runtime_features(),
        "open-radio-hil,task-residence-telemetry"
    );
    assert_eq!(
        crate::image::ImageClass::DiagnosticTxArchitecture.runtime_features(),
        "open-radio-hil,tx-architecture-probes"
    );
    assert_eq!(
        crate::image::ImageClass::DiagnosticTaskPoll.runtime_features(),
        "open-radio-hil,task-poll-telemetry"
    );
    assert_eq!(
        crate::image::ImageClass::DiagnosticCore0RxCoarse.runtime_features(),
        "open-radio-hil,core0-rx-coarse-telemetry"
    );
    assert_eq!(
        crate::image::ImageClass::DiagnosticCore0RxCycles.runtime_features(),
        "open-radio-hil,core0-rx-cycle-telemetry"
    );
    assert_eq!(
        crate::image::ImageClass::DiagnosticRxDelivery.runtime_features(),
        "open-radio-hil,rx-delivery-telemetry"
    );
    assert_eq!(
        crate::image::ImageClass::DiagnosticIeee802154EventStatus.runtime_features(),
        "open-radio-hil,ieee802154-event-status-probe"
    );
    assert_eq!(
        crate::image::ImageClass::DiagnosticIeee802154EdEvent.runtime_features(),
        "open-radio-hil,ieee802154-ed-event-probe"
    );
    assert_eq!(
        crate::image::ImageClass::DiagnosticIeee802154Radio.runtime_features(),
        "open-radio-hil,ieee802154-radio"
    );
    assert_eq!(
        crate::image::ImageClass::DiagnosticIeee802154Thread.runtime_features(),
        "open-radio-hil,ieee802154-thread"
    );
}

#[test]
fn the_ieee802154_radio_image_is_the_performance_image_with_its_services() {
    use crate::image::ImageClass;
    use oer_hil_protocol::FeatureCapabilities;

    let performance = FeatureCapabilities {
        psram_task_stack: true,
        ..FeatureCapabilities::default()
    };
    assert_eq!(
        classify_flashed_capabilities(&performance),
        Some(ImageClass::Performance)
    );
    let air_check = FeatureCapabilities {
        ieee802154_air_check: true,
        ieee802154_session: true,
        ..performance
    };
    assert_eq!(
        classify_flashed_capabilities(&air_check),
        Some(ImageClass::DiagnosticIeee802154Radio)
    );
    let partial = FeatureCapabilities {
        ieee802154_session: false,
        ..air_check
    };
    assert_eq!(classify_flashed_capabilities(&partial), None);
    let thread = FeatureCapabilities {
        ieee802154_thread: true,
        ..air_check
    };
    assert_eq!(
        classify_flashed_capabilities(&thread),
        Some(ImageClass::DiagnosticIeee802154Thread)
    );
    let route = FeatureCapabilities {
        ieee802154_route_probe: true,
        ..performance
    };
    assert_eq!(
        classify_flashed_capabilities(&route),
        Some(ImageClass::DiagnosticIeee802154Route)
    );
    // The route probe image carries no other radio service.
    assert_eq!(
        classify_flashed_capabilities(&FeatureCapabilities {
            ieee802154_route_probe: true,
            ..air_check
        }),
        None
    );
    // Thread runs over the radio image's client, never without it.
    let thread_alone = FeatureCapabilities {
        ieee802154_thread: true,
        ..performance
    };
    assert_eq!(classify_flashed_capabilities(&thread_alone), None);
    let mixed = FeatureCapabilities {
        ieee802154_ed_event_probe: true,
        ..air_check
    };
    assert_eq!(classify_flashed_capabilities(&mixed), None);
}

#[test]
fn image_capability_classifier_preserves_every_exclusive_class() {
    use crate::image::ImageClass;

    for (signals, expected) in [
        (
            image_signature(false, false, false, false, false, false),
            ImageClass::Performance,
        ),
        (
            image_signature(true, false, false, false, false, false),
            ImageClass::Correctness,
        ),
        (
            image_signature(true, false, false, true, false, false),
            ImageClass::DiagnosticMacIrq,
        ),
        (
            image_signature(true, true, false, true, false, false),
            ImageClass::DiagnosticTxWait,
        ),
        (
            image_signature(false, true, false, false, false, false),
            ImageClass::DiagnosticTaskResidence,
        ),
        (
            image_signature(true, true, false, false, false, false),
            ImageClass::DiagnosticTaskPoll,
        ),
        (
            image_signature(true, false, true, false, false, false),
            ImageClass::DiagnosticRxDelivery,
        ),
        (
            image_signature(false, false, false, false, true, false),
            ImageClass::DiagnosticIeee802154EventStatus,
        ),
        (
            image_signature(false, false, false, false, false, true),
            ImageClass::DiagnosticIeee802154EdEvent,
        ),
    ] {
        assert_eq!(classify_image_signature(signals), Some(expected));
    }
    let mut tx_architecture = image_signature(false, true, false, false, false, false);
    tx_architecture.tx_architecture_probe = true;
    assert_eq!(
        classify_image_signature(tx_architecture),
        Some(ImageClass::DiagnosticTxArchitecture),
    );
    let mut core0_rx_cycles = image_signature(true, true, false, false, false, false);
    core0_rx_cycles.core0_rx_cycles = true;
    assert_eq!(
        classify_image_signature(core0_rx_cycles),
        Some(ImageClass::DiagnosticCore0RxCycles),
    );
    let mut core0_rx_coarse = image_signature(false, true, false, false, false, false);
    core0_rx_coarse.core0_rx_cycles = true;
    assert_eq!(
        classify_image_signature(core0_rx_coarse),
        Some(ImageClass::DiagnosticCore0RxCoarse),
    );
}

#[test]
fn memory_benchmark_image_is_exclusive_and_has_a_reproducible_recipe() {
    let mut signature = image_signature(false, false, false, false, false, false);
    signature.memory_benchmark = true;
    assert_eq!(
        classify_image_signature(signature),
        Some(ImageClass::DiagnosticMemoryBenchmark)
    );
    signature.task_poll = true;
    assert_eq!(classify_image_signature(signature), None);
    assert_eq!(
        ImageClass::DiagnosticMemoryBenchmark.runtime_features(),
        "open-radio-hil,memory-benchmark"
    );
    assert!(!ImageClass::DiagnosticMemoryBenchmark.requires_driver_observation());
}

#[test]
fn image_capability_classifier_rejects_mixed_or_non_psram_images() {
    assert_eq!(
        classify_image_signature(image_signature(true, false, false, false, true, false)),
        None
    );
    assert_eq!(
        classify_image_signature(image_signature(false, true, false, false, true, false)),
        None
    );
    assert_eq!(
        classify_image_signature(image_signature(false, false, false, false, true, true)),
        None
    );

    let mut performance = image_signature(false, false, false, false, false, false);
    performance.psram_task_stack = false;
    assert_eq!(classify_image_signature(performance), None);
}

#[test]
fn bluetooth_image_has_no_network_recipe_and_cannot_claim_wifi_capabilities() {
    let mut features = FeatureCapabilities {
        bluetooth_dtm: true,
        structured_evidence: true,
        psram_task_stack: true,
        ..FeatureCapabilities::default()
    };
    assert_eq!(
        classify_flashed_capabilities(&features),
        Some(ImageClass::BluetoothHci)
    );
    features.phy_rx_hot_sram = true;
    assert_eq!(
        classify_flashed_capabilities(&features),
        Some(ImageClass::BluetoothHci)
    );
    features.phy_fault_injection = true;
    assert_eq!(classify_flashed_capabilities(&features), None);
    features.phy_fault_injection = false;
    features.udp = true;
    assert_eq!(classify_flashed_capabilities(&features), None);
    assert!(
        !ImageClass::BluetoothHci
            .runtime_features()
            .contains("open-radio-hil")
    );
    assert!(!ImageClass::BluetoothHci.requires_driver_observation());
}

#[test]
fn the_diagnostic_hooks_tell_the_diagnostic_hci_image_apart() {
    let plain = ImageClass::BluetoothHci.console_capabilities().unwrap();
    let diagnostic = ImageClass::BluetoothHciDiagnostics
        .console_capabilities()
        .unwrap();
    assert_eq!(
        diagnostic,
        FeatureCapabilities {
            bluetooth_hci_lifecycle: true,
            bluetooth_mic_fault: true,
            ..plain
        }
    );
    for partial in [
        FeatureCapabilities {
            bluetooth_hci_lifecycle: true,
            ..plain
        },
        FeatureCapabilities {
            bluetooth_mic_fault: true,
            ..plain
        },
    ] {
        assert_eq!(classify_flashed_capabilities(&partial), None);
    }
    assert_eq!(
        classify_flashed_capabilities(&plain),
        Some(ImageClass::BluetoothHci)
    );
    assert_eq!(
        classify_flashed_capabilities(&diagnostic),
        Some(ImageClass::BluetoothHciDiagnostics)
    );
    assert!(
        ImageClass::BluetoothHciDiagnostics
            .runtime_features()
            .contains("bluetooth-hci-lifecycle,bluetooth-mic-fault")
    );
    assert!(!ImageClass::BluetoothHciDiagnostics.requires_driver_observation());
}

#[test]
fn removed_rx_phy_images_are_rejected_by_both_decoders() {
    for id in [
        "diagnostic-rx-delivery-phy-steps",
        "diagnostic-rx-delivery-phy-settle",
    ] {
        assert!(serde_json::from_str::<ImageClass>(&format!("\"{id}\"")).is_err());
        assert!(id.parse::<ImageClass>().is_err());
    }
}

#[test]
fn a_diagnostic_feature_is_an_explicit_overlay_on_the_performance_image() {
    use oer_hil_protocol::{DiagnosticFeature, DiagnosticFeatures};
    for (feature, class, cargo_feature) in [
        (
            DiagnosticFeature::RxOwnership,
            ImageClass::DiagnosticRxOwnership,
            "rx-ownership-telemetry",
        ),
        (
            DiagnosticFeature::StationExit,
            ImageClass::DiagnosticStationExit,
            "station-exit-evidence",
        ),
    ] {
        let mut features = FeatureCapabilities {
            psram_task_stack: true,
            diagnostic_features: DiagnosticFeatures::empty().with(feature, true),
            ..FeatureCapabilities::default()
        };
        assert_eq!(classify_flashed_capabilities(&features), Some(class));
        assert!(!class.requires_driver_observation());
        assert!(
            class
                .runtime_features()
                .split(',')
                .any(|f| f == cargo_feature)
        );
        features.driver_observation_evidence = true;
        assert_eq!(classify_flashed_capabilities(&features), None);
        features.driver_observation_evidence = false;
        features.diagnostic_features = DiagnosticFeatures::empty();
        assert_eq!(
            classify_flashed_capabilities(&features),
            Some(ImageClass::Performance)
        );
    }
    // Two overlays at once are no known image.
    let both = FeatureCapabilities {
        psram_task_stack: true,
        diagnostic_features: DiagnosticFeature::ALL.into_iter().collect(),
        ..FeatureCapabilities::default()
    };
    assert_eq!(classify_flashed_capabilities(&both), None);
}

#[test]
fn firmware_builds_drop_inherited_cargo_overrides_that_change_the_image() {
    let names = [
        "CARGO_PROFILE_RELEASE_OPT_LEVEL",
        "CARGO_BUILD_RUSTFLAGS",
        "CARGO_TARGET_RISCV32IMAFC_UNKNOWN_NONE_ELF_RUSTFLAGS",
        "CARGO_BUILD_JOBS",
        "CARGO_TARGET_DIR",
        "RUSTFLAGS",
        "PATH",
    ]
    .map(std::ffi::OsString::from);
    assert_eq!(
        super::inherited_build_overrides(names.into_iter()),
        [
            "CARGO_PROFILE_RELEASE_OPT_LEVEL",
            "CARGO_BUILD_RUSTFLAGS",
            "CARGO_TARGET_RISCV32IMAFC_UNKNOWN_NONE_ELF_RUSTFLAGS",
        ]
        .map(std::ffi::OsString::from)
    );
}

#[test]
fn a_console_image_classifies_as_the_class_that_declares_it() {
    for class in ImageClass::ALL {
        if let Some(features) = class.console_capabilities() {
            assert_eq!(
                classify_flashed_capabilities(&features),
                Some(class),
                "{}",
                class.id()
            );
        }
    }
    let dtm = ImageClass::BluetoothHci.console_capabilities().unwrap();
    assert!(dtm.bluetooth_dtm && dtm.bluetooth_hci);
    assert_eq!(ImageClass::Correctness.console_capabilities(), None);
}

#[test]
fn the_bootstrap_takes_only_the_esp_hal_override() {
    // A patch the bootstrap does not use would change its lock file, which
    // its --locked build refuses; a local Xarxa or Embassy must not reach it.
    let mut command = Command::new("cargo");
    add_bootstrap_patches(&mut command, Some(Path::new("/esp-hal")));
    let arguments = command
        .get_args()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert!(
        arguments
            .iter()
            .any(|argument| argument.contains("esp-hal"))
    );
    assert!(
        !arguments
            .iter()
            .any(|argument| argument.contains("xarxa") || argument.contains("embassy"))
    );
}

#[test]
fn the_program_counter_sampler_rides_only_in_the_classes_that_compile_it() {
    let performance = FeatureCapabilities {
        bidirectional: true,
        data_plane_placement: true,
        psram_task_stack: true,
        runtime_configuration: true,
        runtime_initialization: true,
        rx: true,
        simultaneous_station_access_point: true,
        startup_artifact: true,
        station_epoch_control: true,
        station_lifecycle_events: true,
        structured_evidence: true,
        tcp: true,
        timebase_probe: true,
        tx: true,
        udp: true,
        udp_multi_flow: true,
        wifi_access_point: true,
        wifi_monitor_capture: true,
        wifi_role_control: true,
        ..Default::default()
    };
    let residence = FeatureCapabilities {
        task_poll_evidence: true,
        ..performance
    };
    assert_eq!(
        classify_flashed_capabilities(&residence),
        Some(ImageClass::DiagnosticTaskResidence)
    );
    let sampling = |features: FeatureCapabilities| FeatureCapabilities {
        diagnostic_features: DiagnosticFeatures::empty().with(DiagnosticFeature::PcProfile, true),
        ..features
    };
    assert_eq!(
        classify_flashed_capabilities(&sampling(residence)),
        Some(ImageClass::DiagnosticTaskResidence)
    );
    assert!(ImageClass::DiagnosticTaskResidence.samples_program_counter());
    // A class that does not compile the sampler never advertises it.
    assert_eq!(classify_flashed_capabilities(&sampling(performance)), None);
}

#[test]
fn the_classes_that_sample_the_program_counter_are_those_whose_features_enable_it() {
    let manifest: toml::Table = toml::from_str(include_str!(
        "../../../../targets/esp32s31/runtime/Cargo.toml"
    ))
    .unwrap();
    let features = manifest["features"].as_table().unwrap();
    let enables = |class: ImageClass| {
        let mut pending = class
            .runtime_features()
            .split(',')
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let mut seen = std::collections::BTreeSet::new();
        while let Some(feature) = pending.pop() {
            if !seen.insert(feature.clone()) {
                continue;
            }
            if let Some(implied) = features.get(&feature).and_then(toml::Value::as_array) {
                pending.extend(
                    implied
                        .iter()
                        .filter_map(toml::Value::as_str)
                        .filter(|name| !name.starts_with("dep:") && !name.contains('/'))
                        .map(str::to_owned),
                );
            }
        }
        seen.contains("pc-profile")
    };
    for class in ImageClass::ALL {
        assert_eq!(
            class.samples_program_counter(),
            enables(class),
            "{}",
            class.id()
        );
    }
}

#[test]
fn an_inherited_layout_seed_never_reaches_a_build() {
    let command = super::cargo_command();
    assert!(
        command
            .get_envs()
            .any(|(name, value)| name == super::LAYOUT_SEED_ENV && value.is_none()),
        "cargo_command must remove {}",
        super::LAYOUT_SEED_ENV
    );
}

#[test]
fn a_seeded_build_has_artifacts_of_its_own() {
    assert_eq!(super::seed_suffix(None), "");
    let seven = super::seed_suffix(std::num::NonZeroU32::new(7));
    let eleven = super::seed_suffix(std::num::NonZeroU32::new(11));
    assert_eq!(seven, "-seed7");
    assert_ne!(seven, eleven);
}

#[test]
fn the_shared_compile_caches_move_only_when_overridden() {
    let root = Path::new("/checkout");
    assert_eq!(
        compile_cache_base(root, None),
        Path::new("/checkout/target/hil/esp32s31/build-cache")
    );
    assert_eq!(
        compile_cache_base(root, Some("".into())),
        Path::new("/checkout/target/hil/esp32s31/build-cache")
    );
    assert_eq!(
        compile_cache_base(root, Some("/main/target/hil/esp32s31/build-cache".into())),
        Path::new("/main/target/hil/esp32s31/build-cache")
    );
}
