use super::*;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

pub(in crate::hil) struct Fixture(pub(in crate::hil) PathBuf);

impl Fixture {
    pub(in crate::hil) fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "oer-evidence-decisions-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }

    pub(in crate::hil) fn write_run(&self, id: &str, scenarios: Vec<Value>) -> PathBuf {
        let run = self.0.join("runs").join(id);
        fs::create_dir_all(&run).unwrap();
        let mut counts = json!({"scenarios":scenarios.len(),"passed":0,"failed":0,"broken":0,"blocked":0,"skipped":0,"interrupted":0});
        for scenario in &scenarios {
            let key = scenario["outcome"].as_str().unwrap();
            counts[key] = json!(counts[key].as_u64().unwrap() + 1);
        }
        let passed = scenarios.iter().all(|s| s["outcome"] == "passed");
        write(
            &run.join("manifest.json"),
            &json!({
                "schema":2,"run_id":id,"target":"esp32s31","state":"completed",
                "started_unix_millis":100,"finished_unix_millis":200,"duration_millis":100,
                "repository":{"commit":"current","dirty":false,"workspace_sha256":"00".repeat(32)}
            }),
        );
        write(
            &run.join("suite.json"),
            &json!({
                "schema":2,"run_id":id,"target":"esp32s31","outcome":if passed {"passed"} else {"failed"},
                "started_unix_millis":100,"finished_unix_millis":200,"duration_millis":100,
                "counts":counts,"scenarios":scenarios
            }),
        );
        super::super::tests::add_current_build(&self.0, &run);
        super::super::tests::seal(&run);
        run
    }

    pub(in crate::hil) fn load(&self) -> Result<HilEvidenceIndex> {
        HilEvidenceIndex::load(
            &self.0,
            Path::new("runs"),
            "esp32s31",
            &RepositoryState {
                commit: "current".into(),
                dirty: false,
            },
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

pub(in crate::hil) fn write(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

pub(in crate::hil) fn scenario(id: &str, outcomes: &[&str]) -> Value {
    let parsed: Vec<Outcome> = outcomes
        .iter()
        .map(|s| serde_json::from_value(json!(s)).unwrap())
        .collect();
    let outcome = aggregate_outcome(parsed);
    json!({
        "schema":2,"scenario":id,"outcome":outcome,"required_repetitions":outcomes.len(),
        "failure":null,
        "repetitions":outcomes.iter().enumerate().map(|(i,outcome)|json!({
            "schema":2,"repetition":i+1,"outcome":outcome,"measurements":[],
            "failure":if *outcome == "passed" {Value::Null} else {json!({"kind":"scenario","message":"test failure"})}
        })).collect::<Vec<_>>()
    })
}

fn requirement(id: &str, count: u8) -> HilRequirement {
    HilRequirement {
        scenario: id.into(),
        checks: vec![],
        minimum_repetitions: count,
    }
}

#[test]
fn subject_and_failure_identity_survive_a_different_evaluator_checkout() {
    let fixture = Fixture::new();
    let run = fixture.write_run("observed", vec![scenario("ble-att", &["failed"])]);
    fs::write(run.join("application.bin"), b"the observed firmware").unwrap();
    let mut manifest: Value = read_json(&run.join("manifest.json")).unwrap();
    let artifact = &mut manifest["firmware"][0];
    artifact["image"] = json!("ble-radio");
    artifact["application_path"] = json!("application.bin");
    artifact["application_size_bytes"] =
        json!(fs::metadata(run.join("application.bin")).unwrap().len());
    artifact["application_sha256"] = json!(sha256_file(&run.join("application.bin")).unwrap());
    write(&run.join("manifest.json"), &manifest);
    fs::create_dir_all(run.join("scenarios/ble-att")).unwrap();
    write(
        &run.join("scenarios/ble-att/scenario.json"),
        &json!({"id":"ble-att", "image":"ble-radio"}),
    );
    write(
        &run.join("lab-provenance.json"),
        &json!({"peer":"test-peer"}),
    );
    super::super::tests::seal(&run);
    let requirement = requirement("ble-att", 1);
    let current = fixture
        .load()
        .unwrap()
        .decision_for(&requirement, &ScenarioCatalog::default());
    let current = serde_json::to_value(current).unwrap();
    let observation = &current["observations"][0];
    assert_eq!(observation["completion_seal"]["path"], "integrity.json");
    assert!(valid_sha256(
        observation["observation_id"].as_str().unwrap()
    ));
    assert_eq!(
        observation["subject"]["firmware"][0]["application"]["sha256"],
        manifest["firmware"][0]["application_sha256"]
    );
    assert_eq!(
        observation["subject"]["procedure"]["path"],
        "scenarios/ble-att/scenario.json"
    );
    assert_eq!(
        observation["subject"]["fixture"]["path"],
        "lab-provenance.json"
    );
    assert_eq!(
        observation["repetition_failures"][0]["message"],
        "test failure"
    );
    let historical = HilEvidenceIndex::load(
        &fixture.0,
        Path::new("runs"),
        "esp32s31",
        &RepositoryState {
            commit: "changed".into(),
            dirty: true,
        },
    )
    .unwrap();
    let historical =
        serde_json::to_value(historical.decision_for(&requirement, &ScenarioCatalog::default()))
            .unwrap();
    assert_eq!(historical["status"], "missing");
    assert_eq!(
        historical["observations"][0]["observation_id"],
        observation["observation_id"]
    );
    assert_eq!(
        historical["observations"][0]["subject"],
        observation["subject"]
    );
    assert_eq!(
        historical["observations"][0]["repetition_failures"],
        observation["repetition_failures"]
    );
    assert_ne!(
        historical["observations"][0]["exclusions"],
        observation["exclusions"]
    );

    // An edited and resealed procedure is a new observation, even with the same run name.
    write(
        &run.join("scenarios/ble-att/scenario.json"),
        &json!({"id":"ble-att", "image":"ble-radio", "changed":true}),
    );
    super::super::tests::seal(&run);
    let changed = serde_json::to_value(
        fixture
            .load()
            .unwrap()
            .decision_for(&requirement, &ScenarioCatalog::default()),
    )
    .unwrap();
    assert_ne!(
        changed["observations"][0]["observation_id"],
        observation["observation_id"]
    );
}

#[test]
fn a_seal_cannot_hide_an_application_identity_that_disagrees_with_its_bytes() {
    let fixture = Fixture::new();
    let run = fixture.write_run("observed", vec![scenario("ble-att", &["passed"])]);
    fs::write(run.join("application.bin"), b"firmware").unwrap();
    let mut manifest: Value = read_json(&run.join("manifest.json")).unwrap();
    let artifact = &mut manifest["firmware"][0];
    artifact["application_path"] = json!("application.bin");
    artifact["application_size_bytes"] = json!(8);
    artifact["application_sha256"] = json!("00".repeat(32));
    write(&run.join("manifest.json"), &manifest);
    super::super::tests::seal(&run);
    assert!(
        fixture
            .load()
            .unwrap_err()
            .to_string()
            .contains("disagrees with its archived bytes")
    );

    manifest["firmware"][0]
        .as_object_mut()
        .unwrap()
        .remove("application_size_bytes");
    write(&run.join("manifest.json"), &manifest);
    super::super::tests::seal(&run);
    assert!(
        fixture
            .load()
            .unwrap_err()
            .to_string()
            .contains("identity is incomplete")
    );
}

#[test]
fn engineering_work_distinguishes_unseen_incomplete_historical_and_failed_runs() {
    use crate::model::WorkKind;
    let fixture = Fixture::new();
    let catalog = ScenarioCatalog::default();
    let requirement = requirement("exchange", 1);
    let decision = fixture.load().unwrap().decision_for(&requirement, &catalog);
    assert_eq!(decision.next_work().unwrap().0, WorkKind::Experiment);
    fixture.write_run("broken", vec![scenario("exchange", &["broken"])]);
    let decision = fixture.load().unwrap().decision_for(&requirement, &catalog);
    assert_eq!(decision.next_work().unwrap().0, WorkKind::Recheck);
    let old = HilEvidenceIndex::load(
        &fixture.0,
        Path::new("runs"),
        "esp32s31",
        &RepositoryState {
            commit: "other".into(),
            dirty: false,
        },
    )
    .unwrap();
    assert_eq!(
        old.decision_for(&requirement, &catalog)
            .next_work()
            .unwrap()
            .0,
        WorkKind::AssessApplicability
    );
    fixture.write_run("failure", vec![scenario("exchange", &["failed"])]);
    fixture.write_run("pass", vec![scenario("exchange", &["passed"])]);
    let decision = fixture.load().unwrap().decision_for(&requirement, &catalog);
    assert_eq!(
        decision.next_work().unwrap().0,
        WorkKind::InvestigateFailure
    );
    assert!(decision.evidence.is_none());
}

#[test]
fn complete_scenario_survives_an_independent_failure_in_the_same_sealed_suite() {
    let fixture = Fixture::new();
    fixture.write_run(
        "run-1",
        vec![
            scenario("ble-att", &["passed"]),
            scenario("wifi-restart", &["failed"]),
        ],
    );
    let index = fixture.load().unwrap();
    assert_eq!(index.summary.passing, 0);
    assert_eq!(index.summary.qualifying, 1);
    assert_eq!(
        index
            .decision_for(&requirement("ble-att", 1), &ScenarioCatalog::default())
            .status,
        EvidenceStatus::Satisfied
    );
    assert_eq!(
        index
            .decision_for(&requirement("wifi-restart", 1), &ScenarioCatalog::default())
            .status,
        EvidenceStatus::UnresolvedFailure
    );
}

#[test]
fn neither_older_nor_newer_pass_explains_a_current_failure() {
    for outcomes in [["passed", "failed"], ["failed", "passed"]] {
        let fixture = Fixture::new();
        for (i, outcome) in outcomes.iter().enumerate() {
            fixture.write_run(
                &format!("run-{i}"),
                vec![scenario("ble-lifecycle", &[outcome])],
            );
        }
        let decision = fixture.load().unwrap().decision_for(
            &requirement("ble-lifecycle", 1),
            &ScenarioCatalog::default(),
        );
        assert_eq!(decision.status, EvidenceStatus::UnresolvedFailure);
        assert!(decision.evidence.is_none());
        assert_eq!(decision.observations.len(), 2);
    }
}

#[test]
fn blocked_or_broken_attempt_is_not_a_product_failure_or_a_pass() {
    for outcome in ["blocked", "broken", "interrupted", "skipped"] {
        let fixture = Fixture::new();
        fixture.write_run("run-1", vec![scenario("phy-restoration", &[outcome])]);
        let index = fixture.load().unwrap();
        assert_eq!(
            index
                .decision_for(
                    &requirement("phy-restoration", 1),
                    &ScenarioCatalog::default()
                )
                .status,
            EvidenceStatus::Missing
        );
        fixture.write_run("run-2", vec![scenario("phy-restoration", &["passed"])]);
        assert_eq!(
            fixture
                .load()
                .unwrap()
                .decision_for(
                    &requirement("phy-restoration", 1),
                    &ScenarioCatalog::default()
                )
                .status,
            EvidenceStatus::Satisfied
        );
    }
}

#[test]
fn infrastructure_failure_does_not_erase_another_repetitions_product_failure() {
    for terminal in ["broken", "interrupted"] {
        let fixture = Fixture::new();
        fixture.write_run(
            "run-1",
            vec![scenario("ble-lifecycle", &["failed", terminal])],
        );
        fixture.write_run(
            "run-2",
            vec![scenario("ble-lifecycle", &["passed", "passed"])],
        );
        let decision = fixture.load().unwrap().decision_for(
            &requirement("ble-lifecycle", 2),
            &ScenarioCatalog::default(),
        );
        assert_eq!(decision.status, EvidenceStatus::UnresolvedFailure);
        assert!(decision.evidence.is_none());
        assert_eq!(
            decision.observations[0].repetition_outcomes[0],
            Outcome::Failed
        );
    }
}

#[test]
fn repetitions_cannot_be_cherry_picked_or_assembled_across_runs() {
    let fixture = Fixture::new();
    for n in 0..3 {
        fixture.write_run(
            &format!("run-{n}"),
            vec![scenario("memory-workload", &["passed"])],
        );
    }
    assert_eq!(
        fixture
            .load()
            .unwrap()
            .decision_for(
                &requirement("memory-workload", 3),
                &ScenarioCatalog::default()
            )
            .status,
        EvidenceStatus::Missing
    );
    fixture.write_run(
        "run-3",
        vec![scenario("memory-workload", &["passed", "failed", "passed"])],
    );
    assert_eq!(
        fixture
            .load()
            .unwrap()
            .decision_for(
                &requirement("memory-workload", 2),
                &ScenarioCatalog::default()
            )
            .status,
        EvidenceStatus::UnresolvedFailure
    );
}

#[test]
fn failed_old_subject_is_retained_but_does_not_block_current_composition() {
    let fixture = Fixture::new();
    let old = fixture.write_run("old", vec![scenario("ble-lifecycle", &["failed"])]);
    let mut manifest: Value = read_json(&old.join("manifest.json")).unwrap();
    manifest["repository"]["commit"] = json!("before-fix");
    write(&old.join("manifest.json"), &manifest);
    super::super::tests::seal(&old);
    fixture.write_run("current", vec![scenario("ble-lifecycle", &["passed"])]);
    let decision = fixture.load().unwrap().decision_for(
        &requirement("ble-lifecycle", 1),
        &ScenarioCatalog::default(),
    );
    assert_eq!(decision.status, EvidenceStatus::Satisfied);
    assert!(
        decision
            .observations
            .iter()
            .any(|o| o.outcome == Outcome::Failed && o.exclusions == [Exclusion::DifferentCommit])
    );
    let counts = decision.observation_counts();
    assert_eq!(
        (counts.total, counts.passed, counts.failed, counts.excluded),
        (2, 1, 1, 1)
    );
    assert!(decision.next_work().is_none());
}

#[test]
fn ineligible_observations_remain_explained_in_the_serialized_decision() {
    for (field, value, expected) in [
        ("dirty", json!(true), "producer-dirty"),
        ("commit", json!("another-commit"), "different-commit"),
    ] {
        let fixture = Fixture::new();
        let run = fixture.write_run("run-1", vec![scenario("ble-att", &["passed"])]);
        let mut manifest: Value = read_json(&run.join("manifest.json")).unwrap();
        manifest["repository"][field] = value;
        write(&run.join("manifest.json"), &manifest);
        super::super::tests::seal(&run);
        let decision = fixture
            .load()
            .unwrap()
            .decision_for(&requirement("ble-att", 1), &ScenarioCatalog::default());
        let report = serde_json::to_value(decision).unwrap();
        assert_eq!(report["status"], "missing");
        assert!(report["evidence"].is_null());
        assert_eq!(report["observations"][0]["outcome"], "passed");
        assert_eq!(report["observations"][0]["exclusions"], json!([expected]));
        assert_eq!(report["completion_boundary"], "scenario-repetition-set");
        assert_eq!(report["applicability_policy"], "current-source-composition");
    }
}

#[test]
fn latest_eligible_observation_uses_time_not_lexicographic_run_id() {
    let mut index = HilEvidenceIndex::synthetic(&[("phy-tracking", 1)]);
    let observations = index.scenarios.get_mut("phy-tracking").unwrap();
    observations[0].run_id = "z-earlier".into();
    observations[0].started_unix_millis = 100;
    let mut newer = observations[0].clone();
    newer.run_id = "a-later".into();
    newer.started_unix_millis = 200;
    observations.push(newer);
    let decision = index.decision_for(&requirement("phy-tracking", 1), &ScenarioCatalog::default());
    assert_eq!(decision.status, EvidenceStatus::Satisfied);
    assert!(decision.evidence.unwrap().starts_with("hil:a-later/"));
}

#[test]
fn other_scenario_and_wrong_units_cannot_supply_a_requested_obligation() {
    let id = "ble-att";
    let index = HilEvidenceIndex::synthetic(&[("ble-acl", 3)]);
    assert_eq!(
        index
            .decision_for(&requirement(id, 1), &ScenarioCatalog::default())
            .status,
        EvidenceStatus::Missing
    );

    let id = "wifi-traffic";
    let mut catalog = ScenarioCatalog::default();
    catalog.checks.insert(
        id.into(),
        checks::contracts(&json!({
            "workload":{"kind":"udp","direction":"rx"},
            "criteria":{"minimum_rx_bps":100_000}
        }))
        .unwrap(),
    );
    let mut index = HilEvidenceIndex::synthetic(&[(id, 1)]);
    index.scenarios.get_mut(id).unwrap()[0].measurements = vec![vec![json!({
        "name":"udp.rx.target-rate", "value":100_000, "unit":"bytes"
    })]];
    let selected = HilRequirement {
        checks: vec!["udp.rx.target-rate".into()],
        ..requirement(id, 1)
    };
    let decision = index.decision_for(&selected, &catalog);
    assert_eq!(decision.status, EvidenceStatus::Missing);
    assert_eq!(
        decision.observations[0].obligation_gaps,
        [ObligationGap::RequiredChecksUnavailable]
    );
}

#[test]
fn forged_pass_fails_even_when_no_named_checks_are_requested() {
    let fixture = Fixture::new();
    let mut claimed = scenario("ble-lifecycle", &["passed"]);
    claimed["repetitions"][0]["measurements"] = json!([{
        "name":"ble.credits","value":0,"unit":"count",
        "threshold":{"comparison":"exactly","value":4},"verdict":"failed"
    }]);
    fixture.write_run("run-1", vec![claimed]);
    assert!(
        fixture
            .load()
            .unwrap_err()
            .to_string()
            .contains("contradicts repetition outcome")
    );
}

#[test]
fn check_inside_failed_lifecycle_is_not_promoted_to_independent_evidence() {
    let mut index = HilEvidenceIndex::synthetic(&[("ble-lifecycle", 1)]);
    let observation = &mut index.scenarios.get_mut("ble-lifecycle").unwrap()[0];
    observation.outcome = Outcome::Failed;
    observation.measurements = vec![vec![json!({"name":"ble.att","value":1,"unit":"count",
        "threshold":{"comparison":"exactly","value":1},"verdict":"passed"})]];
    let decision = index.decision_for(
        &requirement("ble-lifecycle", 1),
        &ScenarioCatalog::default(),
    );
    assert_eq!(decision.status, EvidenceStatus::UnresolvedFailure);
    assert!(decision.evidence.is_none());
}

#[test]
fn completed_numeric_observation_is_reassessed_but_absence_is_not_a_failure() {
    let id = "wifi-traffic";
    let mut catalog = ScenarioCatalog::default();
    catalog.checks.insert(
        id.into(),
        checks::contracts(&json!({
            "workload":{"kind":"udp","direction":"rx"},
            "criteria":{"minimum_rx_bps":100_000}
        }))
        .unwrap(),
    );
    let mut requested = requirement(id, 1);
    requested.checks.push("udp.rx.target-rate".into());
    let mut index = HilEvidenceIndex::synthetic(&[(id, 1)]);
    assert_eq!(
        index.decision_for(&requested, &catalog).status,
        EvidenceStatus::Missing
    );
    let measurement = json!({"name":"udp.rx.target-rate","value":90_000,"unit":"bits-per-second",
        "threshold":{"comparison":"at-least","value":80_000},"verdict":"passed"});
    index.scenarios.get_mut(id).unwrap()[0].measurements = vec![vec![measurement.clone()]];
    assert_eq!(
        index.decision_for(&requested, &catalog).status,
        EvidenceStatus::UnresolvedFailure
    );
    catalog.checks.insert(
        id.into(),
        checks::contracts(&json!({
            "workload":{"kind":"udp","direction":"rx"},
            "criteria":{"minimum_rx_bps":85_000}
        }))
        .unwrap(),
    );
    assert_eq!(
        index.decision_for(&requested, &catalog).status,
        EvidenceStatus::Satisfied
    );
    assert_eq!(index.scenarios[id][0].measurements[0][0], measurement);
}

#[test]
fn unresolved_control_is_explained_and_cannot_be_replaced_by_an_old_passing_pair() {
    let mut index = HilEvidenceIndex::synthetic(&[("control", 1), ("experiment", 1)]);
    index.scenarios.get_mut("control").unwrap()[0].run_id = "pair".into();
    index.scenarios.get_mut("experiment").unwrap()[0].run_id = "pair".into();
    let catalog = ScenarioCatalog {
        controls: BTreeMap::from([("experiment".into(), "control".into())]),
        ..Default::default()
    };
    assert_eq!(
        index
            .decision_for(&requirement("experiment", 1), &catalog)
            .status,
        EvidenceStatus::Satisfied
    );
    let mut failure = index.scenarios["control"][0].clone();
    failure.run_id = "later-control".into();
    failure.outcome = Outcome::Failed;
    index.scenarios.get_mut("control").unwrap().push(failure);
    let decision = index.decision_for(&requirement("experiment", 1), &catalog);
    assert!(decision.evidence.is_none());
    assert_eq!(
        decision.next_work().unwrap().0,
        crate::model::WorkKind::InvestigateFailure
    );
    assert_eq!(
        decision.control.unwrap().status,
        EvidenceStatus::UnresolvedFailure
    );
}
