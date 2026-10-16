use super::*;

#[test]
fn image_classes_are_stable_and_do_not_use_workload_environment() {
    assert_eq!(oer_hil_schema::image::ImageClass::ALL.len(), 8);
    for (class, features) in [
        (ImageClass::BluetoothGatt, "bluetooth-gatt"),
        (
            ImageClass::DiagnosticBluetoothGattTrace,
            "bluetooth-gatt,bluetooth-trace",
        ),
        (ImageClass::SystemWatchdog, "system-watchdog"),
        (ImageClass::SystemPanicReset, "system-panic-reset"),
    ] {
        assert_eq!(class.runtime_features(), features);
    }
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

/// This checkout's first chip whose HIL agent builds the Bluetooth images
/// (`bluetooth`), the staged agent, or does not, the application agent.
fn agent(bluetooth: bool) -> oer_chip_profile::Profile {
    oer_chip_profile::Profile::all(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.."))
        .unwrap()
        .into_iter()
        .find(|profile| oer_hil_image_class::declares(&profile.id, "bluetooth-radio") == bluetooth)
        .unwrap()
}

#[test]
fn the_classes_that_sample_the_program_counter_are_those_whose_features_enable_it() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let staged = agent(true);
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
    let chip = agent(false).id;
    assert!(serves(&root, &chip, ImageClass::BootSmoke).unwrap());
    assert!(serves(&root, &chip, ImageClass::SystemWatchdog).unwrap());
    assert!(!serves(&root, &chip, ImageClass::BluetoothGatt).unwrap());
}

#[test]
fn each_chip_s_spec_takes_its_agent_policy() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let placement = || PathBuf::from("/out");
    let staged_chip = agent(true);
    let application_chip = agent(false);
    let staged = spec(
        &root,
        &staged_chip.id,
        ImageClass::BluetoothGatt,
        (None, &FeatureDelta::default()),
        Overrides::default(),
        placement(),
    )
    .unwrap();
    assert_eq!(staged.application.package, staged_chip.hil_agent_package());
    assert_eq!(staged.application.features, ["bluetooth-gatt"]);
    assert_eq!(staged.stack_policy, staged_chip.hil_stack_policy());
    assert!(staged.checks.is_some());
    assert!(!staged.builder_inputs.is_empty());
    let _diagnostic = spec(
        &root,
        &staged_chip.id,
        ImageClass::DiagnosticBluetoothGattTrace,
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
