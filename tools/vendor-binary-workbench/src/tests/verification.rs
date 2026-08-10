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
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workbench remains under tools");
    let path = root.join("verification/vendor/targets/esp32s31/baselines/phy.toml");
    let expected = load_evidence_baseline(&path).unwrap();
    assert_eq!(expected.len(), 107);
    for symbol in ["phy_bb_init", "phy_bt_tx_gain_init", "register_chipv7_phy"] {
        assert!(expected.contains_key(&("archive".to_owned(), symbol.to_owned())));
    }
    assert!(compare_evidence_baseline(&expected, &expected).passed);

    let mut downgraded = expected.clone();
    downgraded.insert(
        ("archive".to_owned(), "phy_rf_init".to_owned()),
        EvidenceIdentity::plain("scenario/profile:weaker"),
    );
    assert!(!compare_evidence_baseline(&expected, &downgraded).passed);

    let mut missing = expected.clone();
    missing.remove(&("rom".to_owned(), "phy_enable_agc".to_owned()));
    assert!(!compare_evidence_baseline(&expected, &missing).passed);
}

#[test]
fn profile_evidence_is_bound_to_scenario_contents() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workbench facade remains under tools");
    let profiles = profiles::load(
        &root.join("verification/vendor/targets/esp32s31/profiles/compiled-equivalence.toml"),
    )
    .unwrap();
    let mut modified = profiles[0].clone();
    let original = profile_evidence(&modified);
    modified.scenarios[0].scenario.max_steps =
        modified.scenarios[0].scenario.max_steps.saturating_add(1);
    assert_ne!(profile_evidence(&modified), original);

    let mut narrowed_domain = profiles[0].clone();
    narrowed_domain
        .argument_ranges
        .push(profiles::ArgumentRange {
            index: 0,
            min: 0,
            max: 0,
        });
    assert_ne!(profile_evidence(&narrowed_domain), original);
}

#[test]
#[cfg(feature = "esp32s31-harness")]
fn semantic_evidence_is_bound_to_workbench_sources() {
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
        semantic_contract_evidence("esp32s31-radio-v1", "esp32s31-channel")
            .label()
            .starts_with("composition-state-scenario/esp32s31-channel/sha256:")
    );
}

#[test]
fn effect_contract_evidence_is_bound_to_closed_policy_rules() {
    let binding = bindings::Binding::new(
        bindings::BindingVersion::V1,
        "open_phy_trace_leaf".to_owned(),
        false,
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
    assert!(
        evidence
            .label()
            .starts_with("effect-contract/exact-effects-v1/sha256:")
    );
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
#[cfg(feature = "esp32s31-harness")]
fn verification_json_report_contains_reproducible_inputs() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workbench remains under tools");
    let mut target =
        TargetSpec::load(&root.join("verification/vendor/targets/esp32s31/target.toml")).unwrap();
    crate::platform_pack::PlatformPack::load(
        &root.join("verification/vendor/targets/esp32s31/platform.toml"),
    )
    .unwrap()
    .apply_to_target(&mut target)
    .unwrap();
    let path = env::temp_dir().join(format!(
        "vendor-workbench-report-{}.json",
        std::process::id()
    ));
    let mut evidence = EvidenceSet::new();
    record_evidence(
        &mut evidence,
        "archive",
        "symbol",
        EvidenceIdentity::plain("symbolic"),
    )
    .unwrap();
    let verification = verification_core_report(VerificationCoreInputs {
        target: &target,
        gate: VerificationGate::Regression { match_floor: 1 },
        summary: VerifySummary {
            vendor_functions: 1,
            matched: 1,
            symbolic_matches: 1,
            ..VerifySummary::default()
        },
        orphan_probes: 0,
        evidence_baseline_passed: true,
        passed: true,
        evidence: &evidence,
        artifacts: &[("manifest", manifest.as_path())],
        qualification_gaps: &[],
    })
    .unwrap();
    let document = VerificationCommandReport {
        schema_version: VERIFICATION_REPORT_SCHEMA,
        command: "verify inventory",
        verification,
        sources: Vec::new(),
        inventory: Vec::new(),
        protocols: None,
        evidence_comparison: None,
        report: None,
    };
    write_verification_json_report(&path, &document).unwrap();
    let report = fs::read_to_string(&path).unwrap();
    let loaded_evidence = load_evidence_report(&path).unwrap();
    fs::remove_file(path).unwrap();
    assert!(report.contains("\"schema_version\": 6"));
    assert!(report.contains("\"command\": \"verify inventory\""));
    assert!(report.contains("\"calling_convention\": \"riscv-ilp32\""));
    assert!(report.contains("\"passed\": true"));
    assert!(report.contains("\"sha256\""));
    assert!(report.contains("\"symbol\": \"symbol\""));
    assert_eq!(loaded_evidence, evidence);
}

#[test]
fn evidence_candidate_is_deterministic_and_cannot_replace_the_baseline() {
    let directory = env::temp_dir();
    let suffix = std::process::id();
    let baseline = directory.join(format!("vendor-workbench-baseline-{suffix}.toml"));
    let candidate = directory.join(format!("vendor-workbench-candidate-{suffix}.toml"));
    let mut evidence = EvidenceSet::new();
    record_evidence(
        &mut evidence,
        "rom",
        "second",
        EvidenceIdentity::plain("symbolic"),
    )
    .unwrap();
    record_evidence(
        &mut evidence,
        "archive",
        "first",
        EvidenceIdentity::plain("state/profile:reviewed"),
    )
    .unwrap();
    fs::write(
        &baseline,
        "schema = 2\n\n[[evidence]]\nsource = \"rom\"\nsymbol = \"old\"\nkind = \"symbolic\"\n",
    )
    .unwrap();

    let error = write_evidence_candidate(&baseline, &[("accepted baseline", &baseline)], &evidence)
        .unwrap_err();
    assert!(error.to_string().contains("must not overwrite"));
    write_evidence_candidate(&candidate, &[("accepted baseline", &baseline)], &evidence).unwrap();
    assert_eq!(load_evidence_baseline(&candidate).unwrap(), evidence);
    assert_eq!(
        fs::read_to_string(&candidate).unwrap(),
        "schema = 2\n\n[[evidence]]\nsource = \"archive\"\nsymbol = \"first\"\nkind = \"state/profile:reviewed\"\n\n[[evidence]]\nsource = \"rom\"\nsymbol = \"second\"\nkind = \"symbolic\"\n"
    );

    fs::remove_file(baseline).unwrap();
    fs::remove_file(candidate).unwrap();
}
