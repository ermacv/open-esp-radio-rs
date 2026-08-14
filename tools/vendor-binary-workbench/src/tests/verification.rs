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
    assert!(VerificationGate::Informational.passes(summary, 1));

    let regressed = VerifySummary {
        mismatched: 1,
        ..summary
    };
    assert!(!VerificationGate::Regression { match_floor: 103 }.passes(regressed, 0));
    assert!(VerificationGate::parse("regression", None).is_err());
    assert!(VerificationGate::parse("informational", Some(1)).is_err());
}

#[test]
fn effect_contract_evidence_is_bound_to_closed_policy_rules() {
    let binding = bindings::Binding::new(
        bindings::BindingVersion::V2,
        open_radio_vendor_semantics::RustBindingKind::ExactProductionEntry,
        "open_phy_trace_leaf".to_owned(),
        bindings::ComparisonPlan::parse("direct-effects-v1", 1).unwrap(),
        false,
    )
    .unwrap();
    let exact = effect_contract::EffectPolicy::new(
        effect_contract::EffectComparison::ExactEffectsV2,
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
        effect_contract::EffectComparison::ExactEffectsV2,
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
            .starts_with("effect-contract/exact-effects-v2/sha256:")
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
