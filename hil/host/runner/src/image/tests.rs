use super::*;

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
            bluetooth_peripheral: true,
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
    assert!(
        classify_flashed_capabilities(&FeatureCapabilities {
            bluetooth_peripheral: true,
            ..features
        })
        .is_none()
    );
    assert!(!ImageClass::BluetoothGatt.requires_driver_observation());
    assert_eq!(
        ImageClass::BluetoothGatt.runtime_features(),
        "bluetooth-gatt,psram-task-stack,code-psram,profile-psram-data"
    );
}
use std::sync::atomic::{AtomicU64, Ordering};

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

fn scratch_directory(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path = env::temp_dir().join(format!(
        "open-esp-radio-hil-runner-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&path).unwrap();
    path
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
            bluetooth_watchdog_reset: true,
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
        "system-watchdog,psram-task-stack,code-psram,profile-psram-data"
    );
}

#[test]
fn image_classes_are_stable_and_do_not_use_workload_environment() {
    assert_eq!(crate::image::ImageClass::ALL.len(), 22);
    assert!(
        crate::image::ImageClass::ALL
            .into_iter()
            .all(crate::image::ImageClass::uses_psram_task_stack)
    );
    assert_eq!(crate::image::ImageClass::Performance.id(), "performance");
    assert_eq!(crate::image::ImageClass::Correctness.id(), "correctness");
    assert_eq!(
        crate::image::ImageClass::Correctness.runtime_features(),
        "open-radio-hil,driver-observation,psram-task-stack,code-psram,profile-psram-data"
    );
    assert_eq!(
        crate::image::ImageClass::DiagnosticMacIrq.runtime_features(),
        "open-radio-hil,psram-task-stack,mac-irq-telemetry,code-psram,profile-psram-data"
    );
    assert_eq!(
        crate::image::ImageClass::DiagnosticTaskResidence.runtime_features(),
        "open-radio-hil,psram-task-stack,task-residence-telemetry,code-psram,profile-psram-data"
    );
    assert_eq!(
        crate::image::ImageClass::DiagnosticTxArchitecture.runtime_features(),
        "open-radio-hil,psram-task-stack,tx-architecture-probes,code-psram,profile-psram-data"
    );
    assert_eq!(
        crate::image::ImageClass::DiagnosticTaskPoll.runtime_features(),
        "open-radio-hil,psram-task-stack,task-poll-telemetry,code-psram,profile-psram-data"
    );
    assert_eq!(
        crate::image::ImageClass::DiagnosticCore0RxCoarse.runtime_features(),
        "open-radio-hil,psram-task-stack,core0-rx-coarse-telemetry,code-psram,profile-psram-data"
    );
    assert_eq!(
        crate::image::ImageClass::DiagnosticCore0RxCycles.runtime_features(),
        "open-radio-hil,psram-task-stack,core0-rx-cycle-telemetry,code-psram,profile-psram-data"
    );
    assert_eq!(
        crate::image::ImageClass::DiagnosticRxDelivery.runtime_features(),
        "open-radio-hil,psram-task-stack,rx-delivery-telemetry,code-psram,profile-psram-data"
    );
    assert_eq!(
        crate::image::ImageClass::DiagnosticIeee802154EventStatus.runtime_features(),
        "open-radio-hil,ieee802154-event-status-probe,psram-task-stack,code-psram,profile-psram-data"
    );
    assert_eq!(
        crate::image::ImageClass::DiagnosticIeee802154EdEvent.runtime_features(),
        "open-radio-hil,ieee802154-ed-event-probe,psram-task-stack,code-psram,profile-psram-data"
    );
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
        "open-radio-hil,memory-benchmark,psram-task-stack,code-psram,profile-psram-data"
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
fn tracked_file_snapshot_restores_exact_contents() {
    let directory = scratch_directory("restore");
    let lockfile = directory.join("Cargo.lock");
    let original = b"version = 4\n\n[[package]]\nname = \"fixture\"\n";
    fs::write(&lockfile, original).unwrap();

    let mut snapshot = TrackedFileSnapshot::capture(lockfile.clone()).unwrap();
    fs::write(&lockfile, b"rewritten by cargo\n").unwrap();
    snapshot.restore().unwrap();

    assert_eq!(fs::read(&lockfile).unwrap(), original);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn tracked_file_snapshot_drop_removes_new_file() {
    let directory = scratch_directory("drop");
    let lockfile = directory.join("Cargo.lock");
    {
        let _snapshot = TrackedFileSnapshot::capture(lockfile.clone()).unwrap();
        fs::write(&lockfile, b"generated by cargo\n").unwrap();
    }

    assert!(!lockfile.exists());
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn bluetooth_image_has_no_network_recipe_and_cannot_claim_wifi_capabilities() {
    let mut features = FeatureCapabilities {
        bluetooth_dtm: true,
        bluetooth_peripheral: true,
        structured_evidence: true,
        psram_task_stack: true,
        ..FeatureCapabilities::default()
    };
    assert_eq!(
        classify_flashed_capabilities(&features),
        Some(ImageClass::BluetoothDtm)
    );
    features.phy_rx_hot_sram = true;
    assert_eq!(
        classify_flashed_capabilities(&features),
        Some(ImageClass::BluetoothDtm)
    );
    features.udp = true;
    assert_eq!(classify_flashed_capabilities(&features), None);
    assert!(
        !ImageClass::BluetoothDtm
            .runtime_features()
            .contains("open-radio-hil")
    );
    assert!(!ImageClass::BluetoothDtm.requires_driver_observation());
}

#[test]
fn rx_hot_sram_image_is_the_delivery_control_with_one_placement_change() {
    let mut features = FeatureCapabilities {
        udp: true,
        tcp: true,
        rx: true,
        tx: true,
        bidirectional: true,
        runtime_initialization: true,
        runtime_configuration: true,
        structured_evidence: true,
        udp_multi_flow: true,
        startup_artifact: true,
        station_epoch_control: true,
        station_pause: true,
        wifi_role_control: true,
        wifi_access_point: true,
        simultaneous_station_access_point: true,
        wifi_monitor_capture: true,
        station_lifecycle_events: true,
        driver_observation_evidence: true,
        rx_delivery_evidence: true,
        phy_rx_hot_sram: true,
        psram_task_stack: true,
        data_plane_placement: true,
        timebase_probe: true,
        ..FeatureCapabilities::default()
    };
    assert_eq!(
        classify_flashed_capabilities(&features),
        Some(ImageClass::DiagnosticRxDeliveryPhyHotSram)
    );
    assert!(
        ImageClass::DiagnosticRxDeliveryPhyHotSram
            .runtime_features()
            .split(',')
            .any(|feature| feature == "phy-rx-hot-sram")
    );
    features.phy_rx_hot_sram = false;
    assert_eq!(
        classify_flashed_capabilities(&features),
        Some(ImageClass::DiagnosticRxDelivery)
    );
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
fn automatic_bluetooth_image_has_a_distinct_flashed_identity() {
    let features = FeatureCapabilities {
        bluetooth_dtm: true,
        bluetooth_peripheral: true,
        bluetooth_phy_maintenance: true,
        structured_evidence: true,
        psram_task_stack: true,
        ..FeatureCapabilities::default()
    };
    assert_eq!(
        classify_flashed_capabilities(&features),
        Some(ImageClass::BluetoothPhyMaintenance)
    );
    let mut incomplete = features;
    incomplete.bluetooth_dtm = false;
    incomplete.bluetooth_peripheral = false;
    assert_eq!(classify_flashed_capabilities(&incomplete), None);
    assert!(!ImageClass::BluetoothPhyMaintenance.requires_driver_observation());
    assert!(
        ImageClass::BluetoothPhyMaintenance
            .runtime_features()
            .contains("bluetooth-phy-maintenance")
    );
}

#[test]
fn watchdog_fault_image_cannot_be_mistaken_for_automatic_maintenance() {
    let mut features = FeatureCapabilities {
        bluetooth_dtm: true,
        bluetooth_peripheral: true,
        bluetooth_watchdog_reset: true,
        structured_evidence: true,
        psram_task_stack: true,
        ..FeatureCapabilities::default()
    };
    assert_eq!(
        classify_flashed_capabilities(&features),
        Some(ImageClass::BluetoothWatchdogReset)
    );
    features.bluetooth_phy_maintenance = true;
    assert_eq!(classify_flashed_capabilities(&features), None);
    assert!(
        !ImageClass::BluetoothWatchdogReset
            .runtime_features()
            .contains("bluetooth-phy-maintenance")
    );
}
