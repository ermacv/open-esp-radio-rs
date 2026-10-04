use super::*;

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

#[test]
fn the_radio_less_classes_need_no_driver_observation() {
    for (class, features) in [
        (ImageClass::BluetoothGatt, "bluetooth-gatt"),
        (ImageClass::SystemWatchdog, "system-watchdog"),
        (
            ImageClass::DiagnosticMemoryBenchmark,
            "open-radio-hil,memory-benchmark",
        ),
    ] {
        assert_eq!(class.runtime_features(), features);
        assert!(!class.requires_driver_observation(), "{}", class.id());
    }
    for class in [ImageClass::BluetoothSecureGatt, ImageClass::BluetoothDtm] {
        assert!(!class.requires_driver_observation(), "{}", class.id());
        assert!(!class.runtime_features().contains("open-radio-hil"));
    }
}

#[test]
fn image_classes_are_stable_and_do_not_use_workload_environment() {
    assert_eq!(oer_hil_image_class::ImageClass::ALL.len(), 27);
    assert_eq!(
        oer_hil_image_class::ImageClass::SystemPanicReset.runtime_features(),
        "system-panic-reset"
    );
    assert_eq!(
        oer_hil_image_class::ImageClass::Performance.id(),
        "performance"
    );
    assert_eq!(
        oer_hil_image_class::ImageClass::Correctness.id(),
        "correctness"
    );
    assert_eq!(
        oer_hil_image_class::ImageClass::WifiBleCoex.runtime_features(),
        "wifi-ble-coex"
    );
    assert_eq!(
        oer_hil_image_class::ImageClass::Correctness.runtime_features(),
        "open-radio-hil,driver-observation"
    );
    assert_eq!(
        oer_hil_image_class::ImageClass::DiagnosticMacIrq.runtime_features(),
        "open-radio-hil,mac-irq-telemetry"
    );
    assert_eq!(
        oer_hil_image_class::ImageClass::DiagnosticTaskResidence.runtime_features(),
        "open-radio-hil,task-residence-telemetry"
    );
    assert_eq!(
        oer_hil_image_class::ImageClass::DiagnosticTxArchitecture.runtime_features(),
        "open-radio-hil,tx-architecture-probes"
    );
    assert_eq!(
        oer_hil_image_class::ImageClass::DiagnosticTaskPoll.runtime_features(),
        "open-radio-hil,task-poll-telemetry"
    );
    assert_eq!(
        oer_hil_image_class::ImageClass::DiagnosticCore0RxCoarse.runtime_features(),
        "open-radio-hil,core0-rx-coarse-telemetry"
    );
    assert_eq!(
        oer_hil_image_class::ImageClass::DiagnosticCore0RxCycles.runtime_features(),
        "open-radio-hil,core0-rx-cycle-telemetry"
    );
    assert_eq!(
        oer_hil_image_class::ImageClass::DiagnosticRxDelivery.runtime_features(),
        "open-radio-hil,rx-delivery-telemetry"
    );
    assert_eq!(
        oer_hil_image_class::ImageClass::DiagnosticIeee802154EventStatus.runtime_features(),
        "open-radio-hil,ieee802154-event-status-probe"
    );
    assert_eq!(
        oer_hil_image_class::ImageClass::DiagnosticIeee802154EdEvent.runtime_features(),
        "open-radio-hil,ieee802154-ed-event-probe"
    );
    assert_eq!(
        oer_hil_image_class::ImageClass::DiagnosticIeee802154Radio.runtime_features(),
        "open-radio-hil,ieee802154-radio"
    );
    assert_eq!(
        oer_hil_image_class::ImageClass::DiagnosticIeee802154RadioTrace.runtime_features(),
        "open-radio-hil,ieee802154-radio,ieee802154-trace"
    );
    assert_eq!(
        oer_hil_image_class::ImageClass::DiagnosticIeee802154Thread.runtime_features(),
        "open-radio-hil,ieee802154-thread"
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
fn the_classes_that_sample_the_program_counter_are_those_whose_features_enable_it() {
    let manifest: toml::Table =
        toml::from_str(include_str!("../../../targets/esp32s31/agent/Cargo.toml")).unwrap();
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

#[test]
fn an_unchanged_embedded_runtime_keeps_its_timestamp() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("runtime.bin");
    let target = directory.path().join("bootstrap/stage-two-runtime.bin");
    fs::write(&source, b"runtime").unwrap();
    replace_if_changed(&source, &target).unwrap();
    let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1);
    fs::File::options()
        .write(true)
        .open(&target)
        .unwrap()
        .set_modified(old)
        .unwrap();
    replace_if_changed(&source, &target).unwrap();
    assert_eq!(fs::metadata(&target).unwrap().modified().unwrap(), old);
    fs::write(&source, b"changed").unwrap();
    replace_if_changed(&source, &target).unwrap();
    assert_eq!(fs::read(&target).unwrap(), b"changed");
    assert_ne!(fs::metadata(&target).unwrap().modified().unwrap(), old);
}

#[test]
fn cargo_tree_lines_name_their_packages() {
    let tree = "oer-esp32s31-hil-agent v0.1.0 (/repo/hil/targets/esp32s31/agent)\n\
                critical-section v1.2.0\n\
                oer-ieee802154 v0.1.0 (/repo/crates/protocols/ieee802154) (*)\n";
    assert_eq!(
        tree_packages(tree),
        [
            "critical-section",
            "oer-esp32s31-hil-agent",
            "oer-ieee802154"
        ]
        .map(str::to_owned)
        .into()
    );
}
