use super::*;

#[test]
fn functional_review_cannot_move_numeric_measurements_to_a_changed_application() {
    let fixture = setup();
    let index = fixture.load().unwrap();
    let mut catalog = ScenarioCatalog::default();
    catalog.checks.insert(
        "exchange".into(),
        checks::contracts(&json!({
            "workload":{"kind":"udp","direction":"rx"}, "criteria":{"minimum_rx_bps":10}
        }))
        .unwrap(),
    );
    save(&fixture, &record(&fixture, &index, &catalog));
    assert_eq!(
        evaluated(&fixture, &index, &catalog).1[0].status,
        "identical-image-required"
    );
    run(&fixture, "new", "passed", 200, b"old image");
    let index = fixture.load().unwrap();
    let mut review = record(&fixture, &index, &catalog);
    review.kind = Kind::IdenticalImage;
    save(&fixture, &review);
    assert_eq!(
        evaluated(&fixture, &index, &catalog).0,
        EvidenceStatus::Satisfied
    );
}

#[test]
fn explicit_inapplicability_retains_failure_and_requires_a_reason() {
    let fixture = setup();
    run(&fixture, "other-fixture", "failed", 150, b"old image");
    let index = fixture.load().unwrap();
    let catalog = ScenarioCatalog::default();
    let mut review = record(&fixture, &index, &catalog);
    review.failures.push(FailureResolution {
        observation: reference(&index, "other-fixture").id,
        disposition: Disposition::NotApplicable,
        reason: "Reviewed fixture mismatch excludes this experiment from the selected conditions"
            .into(),
        resolving_observation: None,
    });
    save(&fixture, &review);
    let (reviewed, _) = apply(
        &fixture.0,
        &declaration(),
        &BTreeMap::new(),
        &index,
        &catalog,
    )
    .unwrap();
    let decision = reviewed.decision_for(&requirement(), &catalog);
    assert_eq!(decision.status, EvidenceStatus::Satisfied);
    assert_eq!(decision.observation_counts().failed, 1);
    let value = serde_json::to_value(decision).unwrap();
    let failure = value["observations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["run_id"] == "other-fixture")
        .unwrap();
    assert_eq!(failure["outcome"], "failed");
    assert_eq!(failure["resolution"]["disposition"], "not-applicable");
    review.failures[0].reason.clear();
    save(&fixture, &review);
    assert!(validate(&fixture.0, &declaration(), &catalog).is_err());
}

#[test]
fn snapshot_archive_claim_is_checked_independently_of_run_integrity() {
    let fixture = setup();
    let run = fixture.0.join("runs/new");
    let path = run.join("source/snapshot/snapshot.json");
    let mut snapshot: Value = read_json(&path).unwrap();
    snapshot["archive_sha256"] = json!("11".repeat(32));
    write(&path, &snapshot);
    crate::hil::tests::seal(&run);
    let index = fixture.load().unwrap();
    let catalog = ScenarioCatalog::default();
    save(&fixture, &record(&fixture, &index, &catalog));
    assert_eq!(
        evaluated(&fixture, &index, &catalog).1[0].status,
        "destination-source-binding-not-established"
    );
}

#[test]
fn replay_and_changed_build_selection_cannot_be_admitted() {
    for replay in [true, false] {
        let fixture = setup();
        let run = fixture.0.join("runs/new");
        if replay {
            let path = run.join("manifest.json");
            let mut manifest: Value = read_json(&path).unwrap();
            manifest["firmware"][0]["replayed_from"] = json!("another-run");
            write(&path, &manifest);
        } else {
            let path = run.join("build-provenance.json");
            let mut build: Value = read_json(&path).unwrap();
            build["parameters"]["runtime_features"] = json!("different-runtime");
            write(&path, &build);
        }
        crate::hil::tests::seal(&run);
        let index = fixture.load().unwrap();
        let catalog = ScenarioCatalog::default();
        save(&fixture, &record(&fixture, &index, &catalog));
        let (result, decisions) = evaluated(&fixture, &index, &catalog);
        assert_eq!(result, EvidenceStatus::Missing);
        assert_eq!(
            decisions[0].status,
            if replay {
                "destination-source-binding-not-established"
            } else {
                "external-composition-changed-or-unavailable"
            }
        );
    }
}

#[test]
fn property_identity_tracks_dependency_contracts_and_scenario_criteria() {
    let fixture = setup();
    let mut d = declaration();
    d.depends_on.push("phy".into());
    let mut dependency = declaration();
    dependency.id = "phy".into();
    let mut declarations = BTreeMap::from([("phy".into(), dependency)]);
    let mut catalog = ScenarioCatalog::default();
    let initial = property(&d, &declarations, &requirement(), &catalog, &fixture.0).unwrap();
    declarations.get_mut("phy").unwrap().source_contracts[0].limits = "No shutdown".into();
    let changed = property(&d, &declarations, &requirement(), &catalog, &fixture.0).unwrap();
    assert_ne!(initial.sha256, changed.sha256);
    catalog.definitions.insert(
        "exchange".into(),
        json!({"criteria":{"minimum_rx_bps":100}}),
    );
    let new_criteria = property(&d, &declarations, &requirement(), &catalog, &fixture.0).unwrap();
    assert_ne!(changed.sha256, new_criteria.sha256);
    declarations
        .get_mut("phy")
        .unwrap()
        .source_contracts
        .clear();
    let unmapped = property(&d, &declarations, &requirement(), &catalog, &fixture.0).unwrap();
    assert_eq!(unmapped.unmapped_capabilities, vec!["phy"]);
}

#[test]
fn whole_gatt_memory_obligation_requires_identical_application() {
    let fixture = setup();
    let index = fixture.load().unwrap();
    let mut catalog = ScenarioCatalog::default();
    catalog.definitions.insert(
        "exchange".into(),
        json!({"workload":{"kind":"bluetooth-secure-gatt"}}),
    );
    save(&fixture, &record(&fixture, &index, &catalog));
    assert_eq!(
        evaluated(&fixture, &index, &catalog).1[0].status,
        "identical-image-required"
    );
}
