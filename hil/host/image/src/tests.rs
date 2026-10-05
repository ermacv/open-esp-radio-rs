use super::*;
use oer_hil_image_class::NETWORK_FEATURE;

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
    assert_eq!(oer_hil_schema::image::ImageClass::ALL.len(), 28);
    assert_eq!(
        oer_hil_schema::image::ImageClass::SystemPanicReset.runtime_features(),
        "system-panic-reset"
    );
    assert_eq!(
        oer_hil_schema::image::ImageClass::Performance.id(),
        "performance"
    );
    assert_eq!(
        oer_hil_schema::image::ImageClass::Correctness.id(),
        "correctness"
    );
    assert_eq!(
        oer_hil_schema::image::ImageClass::WifiBleCoex.runtime_features(),
        "wifi-ble-coex"
    );
    assert_eq!(
        oer_hil_schema::image::ImageClass::Correctness.runtime_features(),
        "open-radio-hil,driver-observation"
    );
    assert_eq!(
        oer_hil_schema::image::ImageClass::DiagnosticMacIrq.runtime_features(),
        "open-radio-hil,mac-irq-telemetry"
    );
    assert_eq!(
        oer_hil_schema::image::ImageClass::DiagnosticTaskResidence.runtime_features(),
        "open-radio-hil,task-residence-telemetry"
    );
    assert_eq!(
        oer_hil_schema::image::ImageClass::DiagnosticTxArchitecture.runtime_features(),
        "open-radio-hil,tx-architecture-probes"
    );
    assert_eq!(
        oer_hil_schema::image::ImageClass::DiagnosticTaskPoll.runtime_features(),
        "open-radio-hil,task-poll-telemetry"
    );
    assert_eq!(
        oer_hil_schema::image::ImageClass::DiagnosticCore0RxCoarse.runtime_features(),
        "open-radio-hil,core0-rx-coarse-telemetry"
    );
    assert_eq!(
        oer_hil_schema::image::ImageClass::DiagnosticCore0RxCycles.runtime_features(),
        "open-radio-hil,core0-rx-cycle-telemetry"
    );
    assert_eq!(
        oer_hil_schema::image::ImageClass::DiagnosticRxDelivery.runtime_features(),
        "open-radio-hil,rx-delivery-telemetry"
    );
    assert_eq!(
        oer_hil_schema::image::ImageClass::DiagnosticIeee802154EventStatus.runtime_features(),
        "open-radio-hil,ieee802154-event-status-probe"
    );
    assert_eq!(
        oer_hil_schema::image::ImageClass::DiagnosticIeee802154EdEvent.runtime_features(),
        "open-radio-hil,ieee802154-ed-event-probe"
    );
    assert_eq!(
        oer_hil_schema::image::ImageClass::DiagnosticIeee802154Radio.runtime_features(),
        "open-radio-hil,ieee802154-radio"
    );
    assert_eq!(
        oer_hil_schema::image::ImageClass::DiagnosticIeee802154RadioTrace.runtime_features(),
        "open-radio-hil,ieee802154-radio,ieee802154-trace"
    );
    assert_eq!(
        oer_hil_schema::image::ImageClass::DiagnosticIeee802154Thread.runtime_features(),
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

/// This checkout's first chip whose HIL agent links the network
/// integration (`network`) or does not.
fn networked(network: bool) -> oer_chip_profile::Profile {
    oer_chip_profile::Profile::all(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.."))
        .unwrap()
        .into_iter()
        .find(|profile| oer_hil_image_class::declares(&profile.id, NETWORK_FEATURE) == network)
        .unwrap()
}

#[test]
fn the_classes_that_sample_the_program_counter_are_those_whose_features_enable_it() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let staged = networked(true);
    let manifest: toml::Table =
        toml::from_str(&std::fs::read_to_string(staged.hil_agent_manifest(&root)).unwrap())
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
fn a_seeded_build_has_artifacts_of_its_own() {
    assert_eq!(super::seed_suffix(None), "");
    let seven = super::seed_suffix(std::num::NonZeroU32::new(7));
    let eleven = super::seed_suffix(std::num::NonZeroU32::new(11));
    assert_eq!(seven, "-seed7");
    assert_ne!(seven, eleven);
}

#[test]
fn cargo_tree_lines_name_their_packages() {
    let tree = "oer-chip-a-hil-agent v0.1.0 (/repo/hil/targets/chip-a/agent)\n\
                critical-section v1.2.0\n\
                oer-ieee802154 v0.1.0 (/repo/crates/protocols/ieee802154) (*)\n";
    assert_eq!(
        tree_packages(tree),
        ["critical-section", "oer-chip-a-hil-agent", "oer-ieee802154"]
            .map(str::to_owned)
            .into()
    );
}

#[test]
fn the_application_chip_builds_the_boot_smoke_and_system_watchdog_images() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let chip = networked(false).id;
    assert!(serves(&root, &chip, ImageClass::BootSmoke).unwrap());
    assert!(serves(&root, &chip, ImageClass::SystemWatchdog).unwrap());
    assert!(!serves(&root, &chip, ImageClass::Correctness).unwrap());
}

#[test]
fn each_chip_s_spec_takes_its_agent_policy_and_network() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let placement = || PathBuf::from("/out");
    let staged_chip = networked(true);
    let application_chip = networked(false);
    let staged = spec(
        &root,
        &staged_chip.id,
        ImageClass::Correctness,
        (None, &FeatureDelta::default()),
        Overrides::default(),
        placement(),
    )
    .unwrap();
    assert_eq!(staged.application.package, staged_chip.hil_agent_package());
    assert!(
        staged
            .application
            .features
            .contains(&NETWORK_FEATURE.to_owned())
    );
    assert_eq!(staged.stack_policy, staged_chip.hil_stack_policy());
    assert!(staged.checks.is_some());
    assert!(!staged.builder_inputs.is_empty());
    let _diagnostic = spec(
        &root,
        &staged_chip.id,
        ImageClass::DiagnosticTaskPoll,
        (None, &FeatureDelta::default()),
        Overrides::default(),
        placement(),
    )
    .unwrap();
    let esp_idf = spec(
        &root,
        &application_chip.id,
        ImageClass::SystemWatchdog,
        (None, &FeatureDelta::default()),
        Overrides::default(),
        placement(),
    )
    .unwrap();
    assert_eq!(
        esp_idf.application.package,
        application_chip.hil_agent_package()
    );
    assert_eq!(
        root.join(&esp_idf.application.workspace),
        application_chip.hil_agent_workspace(&root)
    );
    assert_eq!(esp_idf.application.features, ["system-watchdog"]);
    assert_eq!(esp_idf.stack_policy, application_chip.hil_stack_policy());
    assert!(esp_idf.checks.is_some());
}
