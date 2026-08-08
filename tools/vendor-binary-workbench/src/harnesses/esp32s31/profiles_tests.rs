use super::*;

#[test]
fn checked_in_profile_parses() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workbench remains under tools");
    let path =
        root.join("verification/vendor/targets/esp32s31/profiles/compiled-equivalence.profile");
    let profiles = load(&path).unwrap();
    assert_eq!(profiles.len(), 41);
    assert!(profiles.iter().all(|profile| !profile.scenarios.is_empty()));
    assert_eq!(
        profiles
            .iter()
            .filter(|profile| profile.contract == ProfileContract::State)
            .count(),
        7
    );
    assert_eq!(
        profiles
            .iter()
            .find(|profile| profile.name == "rom-nrx-frequency")
            .unwrap()
            .scenarios
            .len(),
        4
    );
}

#[test]
fn libpp_tx_dma_profiles_cover_all_four_queue_selectors() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workbench remains under tools");
    let path = root.join("verification/vendor/targets/esp32s31/profiles/libpp-tx-dma.profile");
    let profiles = load(&path).unwrap();

    assert_eq!(profiles.len(), 4);
    assert!(profiles.iter().all(|profile| {
        profile.vendor_source == "libpp"
            && profile.argument_ranges
                == [ArgumentRange {
                    index: 0,
                    min: 0,
                    max: 3,
                }]
            && profile.coverage_argument_constraints()
                == (0..=3)
                    .map(|queue| {
                        let mut arguments = [None; 8];
                        arguments[0] = Some(queue);
                        arguments
                    })
                    .collect::<Vec<_>>()
            && profile.scenarios.len() == 4
            && profile
                .scenarios
                .iter()
                .enumerate()
                .all(|(queue, scenario)| scenario.scenario.arguments == [queue as u32])
    }));
}

#[test]
fn libpp_sta_tsf_wakeup_profile_closes_the_bool_domain() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workbench remains under tools");
    let path =
        root.join("verification/vendor/targets/esp32s31/profiles/libpp-sta-tsf-wakeup.profile");
    let profiles = load(&path).unwrap();

    assert_eq!(profiles.len(), 1);
    let profile = &profiles[0];
    assert_eq!(profile.vendor_symbol, "hal_set_sta_tsf_wakeup");
    assert!(!profile.compare_return);
    assert_eq!(
        profile.argument_ranges,
        [ArgumentRange {
            index: 0,
            min: 0,
            max: 1,
        }]
    );
    assert_eq!(profile.scenarios.len(), 2);
    assert_eq!(profile.scenarios[0].scenario.arguments, [0]);
    assert_eq!(profile.scenarios[1].scenario.arguments, [1]);
}

#[test]
fn rom_sta_tsf_snapshot_profile_closes_both_pointer_branches() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workbench remains under tools");
    let path =
        root.join("verification/vendor/targets/esp32s31/profiles/rom-sta-tsf-snapshot.profile");
    let profiles = load(&path).unwrap();

    assert_eq!(profiles.len(), 1);
    let profile = &profiles[0];
    assert_eq!(profile.vendor_source, "rom");
    assert_eq!(profile.vendor_symbol, "hal_get_sta_tsf");
    assert_eq!(profile.scenarios.len(), 4);
    assert_eq!(profile.scenarios[0].scenario.arguments, [0, 0]);
    assert_eq!(
        profile.scenarios[3].scenario.arguments,
        [0x3fff_0000, 0x3fff_0004]
    );
    assert!(
        profile.scenarios[3]
            .scenario
            .observed_memory
            .iter()
            .any(|range| range.start == 0x3fff_0000 && range.length == 8)
    );
}

#[test]
fn declared_argument_domain_requires_an_executed_case_for_every_value() {
    let ranges = [ArgumentRange {
        index: 0,
        min: 0,
        max: 3,
    }];
    let scenarios = (0..3)
        .map(|queue| {
            let mut scenario = NamedScenario::new(format!("queue-{queue}"));
            scenario.scenario.arguments.push(queue);
            scenario
        })
        .collect::<Vec<_>>();

    let error = validate_argument_domain("incomplete", &ranges, &scenarios)
        .unwrap_err()
        .to_string();
    assert!(error.contains("a0=0x3"), "{error}");
}

#[test]
fn malformed_profile_retains_its_physical_source_line() {
    let input = "profile fixture\nvendor-source fixture\nunknown value\n";
    let path = std::env::temp_dir().join(format!(
        "vendor-workbench-profile-diagnostic-{}.profile",
        std::process::id()
    ));
    std::fs::write(&path, input).unwrap();
    let error = load(&path).unwrap_err();
    std::fs::remove_file(&path).unwrap();

    assert!(matches!(
        error,
        crate::error::WorkbenchError::ManifestSource {
            path: reported,
            span,
            ..
        } if reported == path && span.offset() == input.find("unknown").unwrap()
    ));
}

#[test]
fn profile_models_runtime_tables_as_layout_instances() {
    let profiles = parse(
        "profile callback-table\n\
         vendor-source rom\n\
         vendor-symbol vendor_entry\n\
         rust-symbol rust_entry\n\
         case installed\n\
         vendor-table reviewed-services-v1 0x4000 0x20 0x3000\n\
         vendor-table-slot reviewed-services-v1 0x4 vendor_callback\n\
         rust-table reviewed-services-v1 0x5000 0x20\n\
         rust-table-slot reviewed-services-v1 0x4 rust_callback\n",
    )
    .unwrap();

    let scenario = &profiles[0].scenarios[0];
    assert_eq!(scenario.vendor_table_instances.len(), 1);
    assert_eq!(
        scenario.vendor_table_instances[0].layout_id,
        "reviewed-services-v1"
    );
    assert_eq!(scenario.vendor_table_instances[0].pointer_cells, [0x3000]);
    assert_eq!(scenario.vendor_table_instances[0].slots[0].offset, 4);
    assert!(matches!(
        &scenario.vendor_table_instances[0].slots[0].target,
        execution::TableSlotTarget::Symbol(symbol) if symbol == "vendor_callback"
    ));
    assert_eq!(scenario.rust_table_instances[0].base_address, 0x5000);

    let error = parse(
        "profile callback-table\n\
         vendor-source rom\n\
         vendor-symbol vendor_entry\n\
         rust-symbol rust_entry\n\
         case missing-layout\n\
         vendor-table-slot missing 0x4 callback\n",
    )
    .unwrap_err();
    assert!(error.to_string().contains("undeclared table missing"));
}

#[test]
fn profile_keeps_runtime_memory_identity_separate_from_logical_types() {
    let profiles = parse(
        "profile memory-alias\n\
         vendor-source rom\n\
         vendor-symbol vendor_entry\n\
         rust-symbol rust_entry\n\
         case shared-state\n\
         arg 0x3fff0000\n\
         vendor-memory-instance state-0 0x3fff0000 0x40\n\
         vendor-memory-binding state-0 argument 0\n\
         vendor-memory-binding state-0 dereferenced-global g_state 0x4\n\
         rust-memory-instance state-0 0x3fff0000 0x40\n\
         rust-memory-binding state-0 argument 0\n\
         rust-memory-binding state-0 absolute dram 0x3fff0000\n",
    )
    .unwrap();

    let scenario = &profiles[0].scenarios[0];
    let vendor = &scenario.vendor_memory_instances[0];
    assert_eq!(vendor.id, "state-0");
    assert_eq!(vendor.base_address, 0x3fff_0000);
    assert_eq!(vendor.length, 0x40);
    assert!(matches!(
        &vendor.bindings[1],
        crate::RuntimeMemoryObjectBinding::DereferencedGlobal {
            symbol,
            pointer_offset: 4,
        } if symbol == "g_state"
    ));
    assert!(matches!(
        &scenario.rust_memory_instances[0].bindings[1],
        crate::RuntimeMemoryObjectBinding::Absolute {
            address_space,
            address: 0x3fff_0000,
        } if address_space == "dram"
    ));
}

#[test]
fn profile_models_reviewed_register_behavior_as_a_device_factory() {
    let profiles = parse(
        "profile device\n\
         vendor-source rom\n\
         vendor-symbol vendor_entry\n\
         rust-symbol rust_entry\n\
         case irq\n\
         device-model w1c irq-status 0x60008020 32 0xf 0x3 0xc\n",
    )
    .unwrap();

    let models = &profiles[0].scenarios[0].scenario.device_models;
    assert_eq!(models.len(), 1);
    let descriptor = models[0].descriptor();
    assert_eq!(descriptor.id, "irq-status");
    assert_eq!(descriptor.kind, "w1c");
    assert_eq!(descriptor.range.start, 0x6000_8020);
    assert_eq!(descriptor.range.length, 4);
    assert_eq!(descriptor.configuration["clear-mask"], "0x00000003");
}
