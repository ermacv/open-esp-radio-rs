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
