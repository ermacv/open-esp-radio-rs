use super::*;

#[test]
fn regression_and_completion_gates_are_independent() {
    let summary = VerifySummary {
        vendor_functions: 466,
        matched: 103,
        symbolic_matches: 57,
        scenario_matches: 34,
        state_matches: 7,
        composition_matches: 5,
        missing: 363,
        ..VerifySummary::default()
    };
    assert!(VerificationGate::Regression { match_floor: 103 }.passes(summary, 0));
    assert!(!VerificationGate::Regression { match_floor: 104 }.passes(summary, 0));
    assert!(!VerificationGate::Completion.passes(summary, 0));

    let regressed = VerifySummary {
        mismatched: 1,
        ..summary
    };
    assert!(!VerificationGate::Regression { match_floor: 103 }.passes(regressed, 0));
    assert!(VerificationGate::parse("regression", None).is_err());
}

#[test]
fn checked_in_evidence_baseline_locks_symbol_and_evidence_identity() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("baselines/esp32s31.evidence");
    let expected = load_evidence_baseline(&path).unwrap();
    assert_eq!(expected.len(), 104);
    assert!(check_evidence_baseline(&expected, &expected));

    let mut downgraded = expected.clone();
    downgraded.insert(
        ("archive".to_owned(), "phy_rf_init".to_owned()),
        "scenario/profile:weaker".to_owned(),
    );
    assert!(!check_evidence_baseline(&expected, &downgraded));

    let mut missing = expected.clone();
    missing.remove(&("rom".to_owned(), "phy_enable_agc".to_owned()));
    assert!(!check_evidence_baseline(&expected, &missing));
}

#[test]
fn profile_evidence_is_bound_to_scenario_contents() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("phy-trace remains under tools/phy-trace");
    let profiles = profiles::load(&root.join("tools/phy-trace/profiles/esp32s31.profile")).unwrap();
    let mut modified = profiles[0].clone();
    let original = profile_evidence(&modified);
    modified.scenarios[0].scenario.max_steps =
        modified.scenarios[0].scenario.max_steps.saturating_add(1);
    assert_ne!(profile_evidence(&modified), original);
}

#[test]
fn semantic_evidence_is_bound_to_validator_sources() {
    let original = semantic_contract_digest_from_sources(
        "esp32s31-channel",
        &[("semantic.rs", "footprint-v1"), ("emulator.rs", "strict")],
    );
    let weakened = semantic_contract_digest_from_sources(
        "esp32s31-channel",
        &[
            ("semantic.rs", "footprint-v1"),
            ("emulator.rs", "permissive"),
        ],
    );
    let other_contract = semantic_contract_digest_from_sources(
        "esp32s31-rf-init",
        &[("semantic.rs", "footprint-v1"), ("emulator.rs", "strict")],
    );
    assert_ne!(original, weakened);
    assert_ne!(original, other_contract);
    assert!(
        semantic_contract_evidence("esp32s31-channel")
            .starts_with("composition-state-scenario/esp32s31-channel/sha256:")
    );
}

#[test]
fn effect_contract_evidence_is_bound_to_closed_policy_rules() {
    let binding = bindings::Binding::new(
        bindings::BindingVersion::V1,
        bindings::VendorRevision::Esp32s31Eco0Rom,
        BTreeSet::from([
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_owned(),
        ]),
        BTreeSet::new(),
        "open_phy_trace_leaf".to_owned(),
        None,
    )
    .unwrap();
    let exact = effect_contract::EffectPolicy::new(
        effect_contract::EffectComparison::ExactEffectsV1,
        [(
            effect_contract::EffectSelector::MmioRead {
                width: 32,
                address: 0x2010_7030,
            },
            effect_contract::EffectDisposition::Required,
        )],
    )
    .unwrap();
    let forbidden = effect_contract::EffectPolicy::new(
        effect_contract::EffectComparison::ExactEffectsV1,
        [(
            effect_contract::EffectSelector::MmioRead {
                width: 32,
                address: 0x2010_7030,
            },
            effect_contract::EffectDisposition::Forbidden,
        )],
    )
    .unwrap();
    let evidence = effect_contract_evidence(&exact, &binding, "generated-proof-v1");
    assert!(evidence.starts_with("effect-contract/exact-effects-v1/sha256:"));
    assert_ne!(
        evidence,
        effect_contract_evidence(&forbidden, &binding, "generated-proof-v1")
    );
    assert_ne!(
        evidence,
        effect_contract_evidence(&exact, &binding, "changed-generated-proof")
    );
}

#[test]
fn verification_json_report_contains_reproducible_inputs() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let path = env::temp_dir().join(format!(
        "open-esp-radio-phy-trace-report-{}.json",
        std::process::id()
    ));
    let mut evidence = EvidenceSet::new();
    record_evidence(&mut evidence, "archive", "symbol", "symbolic").unwrap();
    write_verification_json_report(
        &path,
        VerificationGate::Regression { match_floor: 1 },
        VerifySummary {
            vendor_functions: 1,
            matched: 1,
            symbolic_matches: 1,
            ..VerifySummary::default()
        },
        0,
        true,
        true,
        &evidence,
        &[("manifest", &manifest)],
        &[],
    )
    .unwrap();
    let report = fs::read_to_string(&path).unwrap();
    fs::remove_file(path).unwrap();
    assert!(report.contains("\"schema_version\": 1"));
    assert!(report.contains("\"passed\": true"));
    assert!(report.contains("\"sha256\""));
    assert!(report.contains("\"symbol\": \"symbol\""));
}
