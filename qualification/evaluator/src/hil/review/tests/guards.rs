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
    let mut catalog = ScenarioCatalog::default();
    catalog.definitions.insert(
        "exchange".into(),
        json!({"id":"exchange","image":"correctness","workload":{"kind":"bluetooth-secure-gatt"}}),
    );
    let run = fixture.0.join("runs/old");
    write(
        &run.join("scenarios/exchange/scenario.json"),
        &catalog.definitions["exchange"],
    );
    crate::hil::tests::seal(&run);
    let index = fixture.load().unwrap();
    save(&fixture, &record(&fixture, &index, &catalog));
    assert_eq!(
        evaluated(&fixture, &index, &catalog).1[0].status,
        "identical-image-required"
    );
}

#[test]
fn property_identity_ignores_only_display_metadata() {
    let fixture = setup();
    let mut catalog = ScenarioCatalog::default();
    let definition = json!({"description":"Original", "tags":["wifi"],
        "workload":{"kind":"station-reconnect"}, "repetitions":2});
    catalog.definitions.insert("exchange".into(), definition);
    let original = property(
        &declaration(),
        &BTreeMap::new(),
        &requirement(),
        &catalog,
        &fixture.0,
    )
    .unwrap();
    let scenario = catalog.definitions.get_mut("exchange").unwrap();
    scenario["description"] = json!("Edited explanation");
    scenario["tags"] = json!(["smoke"]);
    let annotated = property(
        &declaration(),
        &BTreeMap::new(),
        &requirement(),
        &catalog,
        &fixture.0,
    )
    .unwrap();
    assert_eq!(original.sha256, annotated.sha256);
    catalog.definitions.get_mut("exchange").unwrap()["repetitions"] = json!(3);
    let changed = property(
        &declaration(),
        &BTreeMap::new(),
        &requirement(),
        &catalog,
        &fixture.0,
    )
    .unwrap();
    assert_ne!(original.sha256, changed.sha256);
}

#[test]
fn changed_or_unrecorded_composition_cannot_transfer_to_another_application() {
    for change in [
        "network",
        "missing-network",
        "lock",
        "lock-bytes",
        "flags",
        "compiler",
        "missing-environment",
    ] {
        let fixture = setup();
        let run = fixture.0.join("runs/new");
        let path = run.join("build-provenance.json");
        let mut build: Value = read_json(&path).unwrap();
        match change {
            "network" => build["parameters"]["network"] = json!("upstream-smoltcp"),
            "missing-network" => {
                build["parameters"]
                    .as_object_mut()
                    .unwrap()
                    .remove("network");
            }
            "flags" => build["environment"]["inherited_rustflags"] = json!("--cfg changed"),
            "compiler" => build["environment"]["tools"][0]["version"] = json!("different compiler"),
            "missing-environment" => {
                build.as_object_mut().unwrap().remove("environment");
            }
            "lock" | "lock-bytes" => {
                fs::write(run.join("embedded-lock.lock"), b"different dependencies").unwrap();
                if change == "lock" {
                    build["files"][1]["sha256"] = json!(digest(b"different dependencies"));
                    build["files"][1]["size_bytes"] = json!(22);
                }
            }
            _ => unreachable!(),
        }
        write(&path, &build);
        crate::hil::tests::seal(&run);
        let index = fixture.load().unwrap();
        let catalog = ScenarioCatalog::default();
        save(&fixture, &record(&fixture, &index, &catalog));
        assert_eq!(
            evaluated(&fixture, &index, &catalog).1[0].status,
            "external-composition-changed-or-unavailable",
            "{change}"
        );
    }
}

#[test]
fn wifi_review_survives_ble_only_build_and_requires_reassessment_after_phy_change() {
    let fixture = Fixture::new();
    fs::write(fixture.0.join("driver.rs"), "wifi owner").unwrap();
    fs::write(fixture.0.join("phy.rs"), "shared PHY owner").unwrap();
    fs::write(fixture.0.join("ble.rs"), "BLE version A").unwrap();
    run(&fixture, "old", "passed", 100, b"application A");
    fs::write(fixture.0.join("ble.rs"), "BLE version B").unwrap();
    let destination = run(&fixture, "new", "passed", 200, b"application B");
    // Only BLE runs on B. No new observation of the Wi-Fi experiment exists.
    let mut suite: Value = read_json(&destination.join("suite.json")).unwrap();
    suite["scenarios"][0]["scenario"] = json!("bluetooth-check");
    write(&destination.join("suite.json"), &suite);
    let procedure = destination.join("scenarios/exchange/scenario.json");
    let mut definition: Value = read_json(&procedure).unwrap();
    definition["id"] = json!("bluetooth-check");
    write(&procedure, &definition);
    fs::rename(
        destination.join("scenarios/exchange"),
        destination.join("scenarios/bluetooth-check"),
    )
    .unwrap();
    crate::hil::tests::seal(&destination);

    let mut wifi = declaration();
    wifi.id = "wifi".into();
    wifi.depends_on.push("phy".into());
    let mut phy = declaration();
    phy.id = "phy".into();
    phy.source_contracts[0].source_paths = vec!["phy.rs".into()];
    let declarations = BTreeMap::from([("phy".into(), phy)]);
    let catalog = ScenarioCatalog::default();
    let index = fixture.load().unwrap();
    assert_eq!(index.scenarios["exchange"].len(), 1);
    assert_eq!(index.scenarios["bluetooth-check"].len(), 1);
    let mut review = record(&fixture, &index, &catalog);
    let binding = property(&wifi, &declarations, &requirement(), &catalog, &fixture.0).unwrap();
    review.capability = wifi.id.clone();
    review.property_sha256 = binding.sha256;
    review.inputs = binding.current_inputs;
    save(&fixture, &review);
    let (reviewed, decisions) = apply(&fixture.0, &wifi, &declarations, &index, &catalog).unwrap();
    assert_eq!(decisions[0].status, "applied");
    let mut decision = reviewed.decision_for(&requirement(), &catalog);
    decision
        .attach_reviews(&fixture.0, &wifi, &declarations, &catalog, &decisions)
        .unwrap();
    assert_eq!(decision.status, EvidenceStatus::Satisfied);
    assert!(decision.next_work().is_none());

    fs::write(fixture.0.join("phy.rs"), "changed shared PHY ownership").unwrap();
    let (reviewed, decisions) = apply(&fixture.0, &wifi, &declarations, &index, &catalog).unwrap();
    assert_eq!(decisions[0].status, "current-input-changed");
    let mut decision = reviewed.decision_for(&requirement(), &catalog);
    decision
        .attach_reviews(&fixture.0, &wifi, &declarations, &catalog, &decisions)
        .unwrap();
    assert_eq!(decision.status, EvidenceStatus::Missing);
    assert!(matches!(
        decision.next_work(),
        Some((crate::model::WorkKind::AssessApplicability, _))
    ));
}

#[test]
fn legacy_property_identity_remains_usable_only_for_its_exact_contract() {
    let fixture = setup();
    let mut catalog = ScenarioCatalog::default();
    let index = fixture.load().unwrap();
    let mut review = record(&fixture, &index, &catalog);
    review.property_sha256 = property(
        &declaration(),
        &BTreeMap::new(),
        &requirement(),
        &catalog,
        &fixture.0,
    )
    .unwrap()
    .legacy_sha256;
    save(&fixture, &review);
    assert_eq!(evaluated(&fixture, &index, &catalog).1[0].status, "applied");
    catalog
        .definitions
        .insert("exchange".into(), json!({"repetitions":2}));
    assert_eq!(
        evaluated(&fixture, &index, &catalog).1[0].status,
        "property-changed"
    );
}

#[test]
fn build_only_destination_admits_existing_observation_without_creating_a_pass() {
    let fixture = setup();
    let catalog = ScenarioCatalog::default();
    let index = fixture.load().unwrap();
    let mut review = record(&fixture, &index, &catalog);
    let run = fixture.0.join("runs/new");
    let manifest: Value = read_json(&run.join("manifest.json")).unwrap();
    fs::remove_file(run.join("manifest.json")).unwrap();
    fs::remove_file(run.join("suite.json")).unwrap();
    fs::remove_file(run.join("integrity.json")).unwrap();
    fs::remove_dir_all(run.join("scenarios")).unwrap();
    write(
        &run.join("build.json"),
        &json!({"schema":1,"kind":"open-esp-radio-build","target":"esp32s31",
        "repository":manifest["repository"],"firmware":manifest["firmware"]}),
    );
    let build = fixture.0.join("builds/B");
    fs::create_dir_all(build.parent().unwrap()).unwrap();
    fs::rename(run, &build).unwrap();
    let files = collect_inventory(&build, true).unwrap().into_iter().map(|(path, size_bytes)| {
        json!({"sha256":sha256_file(&build.join(&path)).unwrap(),"path":path,"size_bytes":size_bytes})
    }).collect::<Vec<_>>();
    let seal = json!({"schema":2,"run_id":"build-only","files":files});
    write(&build.join("integrity.json"), &seal);
    review.destination.build_record = Some("builds/B".into());
    review.destination.id = sha256_file(&build.join("integrity.json")).unwrap();
    save(&fixture, &review);
    let index = fixture.load().unwrap();
    assert_eq!(index.scenarios["exchange"].len(), 1);
    assert_eq!(evaluated(&fixture, &index, &catalog).1[0].status, "applied");
    assert_eq!(
        evaluated(&fixture, &index, &catalog).0,
        EvidenceStatus::Satisfied
    );
    fs::write(build.join("application.bin"), "unbound firmware").unwrap();
    assert!(
        apply(
            &fixture.0,
            &declaration(),
            &BTreeMap::new(),
            &index,
            &catalog
        )
        .is_err()
    );
}

#[test]
fn evidence_classification_excludes_supporting_tests_but_never_required_owners() {
    let fixture = setup();
    let index = fixture.load().unwrap();
    let catalog = ScenarioCatalog::default();
    let mut review = record(&fixture, &index, &catalog);
    review.inputs.push(InputBinding {
        path: "tests/old-host-check.rs".into(),
        sha256: "ab".repeat(32),
        kind: InputKind::Evidence,
        reason: Some("Supporting host test; not a runtime input".into()),
    });
    save(&fixture, &review);
    assert_eq!(evaluated(&fixture, &index, &catalog).1[0].status, "applied");
    let owner = review
        .inputs
        .iter_mut()
        .find(|i| i.path == Path::new("driver.rs"))
        .unwrap();
    owner.kind = InputKind::Evidence;
    owner.reason = Some("Attempt to omit the required implementation".into());
    save(&fixture, &review);
    assert_eq!(
        evaluated(&fixture, &index, &catalog).1[0].status,
        "owner-bindings-incomplete"
    );
}

#[test]
fn procedure_binding_normalizes_defaults_and_annotations_but_binds_execution() {
    let original =
        "schema = 4\nid = \"boot\"\nimage = \"boot-smoke\"\n[workload]\nkind = \"boot-smoke\"\n";
    let annotated = original.replace(
        "[workload]",
        "description = \"rewritten\"\ntags = [\"new\"]\nrepetitions = 1\n[workload]",
    );
    let hash = input_hash(original.as_bytes(), &InputKind::Procedure).unwrap();
    assert_eq!(
        input_hash(annotated.as_bytes(), &InputKind::Procedure).unwrap(),
        hash
    );
    assert_ne!(
        input_hash(
            annotated
                .replace("repetitions = 1", "repetitions = 2")
                .as_bytes(),
            &InputKind::Procedure
        )
        .unwrap(),
        hash
    );
    assert!(
        input_hash(
            b"[hardware]\ndescription = 'contract'",
            &InputKind::Procedure
        )
        .is_err()
    );
}
