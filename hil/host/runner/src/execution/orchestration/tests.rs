use std::collections::BTreeSet;

use super::*;

fn catalog() -> Catalog {
    Catalog::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios"))
        .expect("load scenario catalog")
}

fn passed(scenario: &Scenario) -> ScenarioResult {
    ScenarioResult {
        schema: RUN_SCHEMA,
        scenario: scenario.id().to_owned(),
        image: scenario.image(),
        outcome: Outcome::Passed,
        required_repetitions: scenario.repetitions(),
        repetitions: (1..=scenario.repetitions())
            .map(|repetition| RepetitionResult {
                schema: RUN_SCHEMA,
                repetition,
                outcome: Outcome::Passed,
                started_unix_millis: 0,
                duration_millis: 0,
                artifact_directory: PathBuf::from("scenarios")
                    .join(scenario.id())
                    .join(format!("repetition-{repetition:03}")),
                attachments: Vec::new(),
                measurements: Vec::new(),
                failure: None,
            })
            .collect(),
        failure: None,
    }
}

fn session(root: &Path) -> RunSession {
    crate::tests::register();
    RunSession::create(
        root,
        "esp32s31",
        "test-cell",
        "test-dut",
        Path::new("/test/no-device"),
        vec![OsString::from("test")],
    )
    .unwrap()
}

#[derive(Default)]
struct FakeSuite {
    log: Vec<String>,
    preparation_failures: Vec<(ImageClass, FailureKind)>,
    preflight_failures: BTreeSet<String>,
    cancel_on_check: Option<usize>,
    checks: usize,
}

impl SuiteEffects for FakeSuite {
    fn check_cancelled(&mut self) -> Result<()> {
        self.checks += 1;
        self.log.push(format!("cancel-check:{}", self.checks));
        if self.cancel_on_check == Some(self.checks) {
            return Err("injected cancellation".into());
        }
        Ok(())
    }

    fn preflight(&mut self, scenario: &Scenario) -> Option<Failure> {
        self.log.push(format!("preflight:{}", scenario.id()));
        self.preflight_failures
            .contains(scenario.id())
            .then(|| Failure::new(FailureKind::Precondition, "injected preflight failure"))
    }

    fn prepare_image(
        &mut self,
        class: ImageClass,
        _session: &mut RunSession,
    ) -> Result<Option<Failure>> {
        self.log.push(format!("prepare:{}", class.id()));
        Ok(self
            .preparation_failures
            .iter()
            .find(|(candidate, _)| *candidate == class)
            .map(|(_, kind)| Failure::new(*kind, "injected image failure")))
    }

    fn execute_scenario(
        &mut self,
        scenario: &Scenario,
        _session: &RunSession,
    ) -> Result<ScenarioResult> {
        self.log.push(format!("execute:{}", scenario.id()));
        Ok(passed(scenario))
    }
}

fn two_same_image(catalog: &Catalog) -> [&Scenario; 2] {
    for class in ImageClass::ALL {
        let candidates = catalog
            .all()
            .iter()
            .filter(|scenario| scenario.image() == class)
            .take(2)
            .collect::<Vec<_>>();
        if let [first, second] = candidates.as_slice() {
            return [*first, *second];
        }
    }
    panic!("catalog has no shared image class")
}

#[test]
fn successful_group_prepares_once_and_preserves_scenario_order() {
    let catalog = catalog();
    let selected = two_same_image(&catalog);
    let root = tempfile::tempdir().unwrap();
    let mut session = session(root.path());
    let mut fake = FakeSuite::default();
    let results = execute_selected(&mut session, &mut fake, &selected).unwrap();

    assert_eq!(results.len(), 2);
    assert_eq!(
        fake.log
            .iter()
            .filter(|entry| entry.starts_with("prepare:"))
            .count(),
        1
    );
    let executed = fake
        .log
        .iter()
        .filter_map(|entry| entry.strip_prefix("execute:"))
        .collect::<Vec<_>>();
    assert_eq!(executed, [selected[0].id(), selected[1].id()]);
}

#[test]
fn campaign_executes_only_the_control_and_experiment_with_one_preparation() {
    let catalog = catalog();
    let experiment = catalog
        .get("diagnostic-station-phy-combined-high-load-delivery-rx")
        .unwrap();
    let plan =
        hil_core::campaign::Plan::create(&catalog, &[experiment], Integration::UpstreamXarxa)
            .unwrap();
    let (selected, _) = plan.resolve(&catalog).unwrap();
    let root = tempfile::tempdir().unwrap();
    let mut session = session(root.path());
    let mut fake = FakeSuite::default();
    execute_selected(&mut session, &mut fake, &selected).unwrap();
    assert_eq!(
        fake.log
            .iter()
            .filter(|entry| entry.starts_with("prepare:"))
            .count(),
        1
    );
    assert_eq!(
        fake.log
            .iter()
            .filter_map(|entry| entry.strip_prefix("execute:"))
            .collect::<Vec<_>>(),
        [experiment.control().unwrap(), experiment.id()]
    );
}

#[test]
fn image_build_and_flash_failures_block_the_group_but_continue_later_classes() {
    let catalog = catalog();
    let first = catalog
        .all()
        .iter()
        .find(|scenario| scenario.image() == ImageClass::BootSmoke)
        .expect("boot-smoke scenario");
    let second = catalog
        .all()
        .iter()
        .find(|scenario| scenario.image() == ImageClass::Performance)
        .expect("performance scenario");

    for kind in [FailureKind::ImageBuild, FailureKind::ImageFlash] {
        let root = tempfile::tempdir().unwrap();
        let mut session = session(root.path());
        let mut fake = FakeSuite {
            preparation_failures: vec![(first.image(), kind)],
            ..FakeSuite::default()
        };
        let results = execute_selected(&mut session, &mut fake, &[first, second]).unwrap();
        assert_eq!(results[0].outcome, Outcome::Blocked);
        assert_eq!(results[0].failure.as_ref().unwrap().kind, kind);
        assert_eq!(results[1].outcome, Outcome::Passed);
        assert!(fake.log.contains(&format!("execute:{}", second.id())));
    }
}

#[test]
fn preflight_failure_has_no_image_or_workload_side_effect() {
    let catalog = catalog();
    let scenario = catalog.get("boot-smoke").expect("boot-smoke");
    let root = tempfile::tempdir().unwrap();
    let mut session = session(root.path());
    let mut fake = FakeSuite {
        preflight_failures: BTreeSet::from([scenario.id().to_owned()]),
        ..FakeSuite::default()
    };
    let results = execute_selected(&mut session, &mut fake, &[scenario]).unwrap();
    assert_eq!(results[0].outcome, Outcome::Blocked);
    assert!(!fake.log.iter().any(|entry| entry.starts_with("prepare:")));
    assert!(!fake.log.iter().any(|entry| entry.starts_with("execute:")));
}

#[test]
fn cancellation_stops_before_the_next_scenario() {
    let catalog = catalog();
    let selected = two_same_image(&catalog);
    let root = tempfile::tempdir().unwrap();
    let mut session = session(root.path());
    let mut fake = FakeSuite {
        cancel_on_check: Some(3),
        ..FakeSuite::default()
    };
    let error = execute_selected(&mut session, &mut fake, &selected).unwrap_err();
    assert_eq!(error.to_string(), "injected cancellation");
    assert!(fake.log.contains(&format!("execute:{}", selected[0].id())));
    assert!(!fake.log.contains(&format!("execute:{}", selected[1].id())));
    let seal = session
        .directory()
        .join("attempts")
        .join(format!("{}.json", selected[0].id()));
    let before = fs::read(&seal).unwrap();
    assert!(
        !session
            .directory()
            .join("attempts")
            .join(format!("{}.json", selected[1].id()))
            .exists()
    );
    let record: serde_json::Value = serde_json::from_slice(&before).unwrap();
    assert_eq!(record["suite"]["scenarios"][0]["outcome"], "passed");
    drop(session);
    assert_eq!(fs::read(seal).unwrap(), before);
}

#[test]
fn attempt_seal_is_write_once_and_partial_series_cannot_claim_completion() {
    let catalog = catalog();
    let scenario = catalog.get("boot-smoke").unwrap();
    let root = tempfile::tempdir().unwrap();
    let mut session = session(root.path());
    let complete = passed(scenario);
    let mut partial = complete.clone();
    partial.repetitions.pop();
    assert!(session.seal_scenario(scenario, &partial).is_err());
    session.seal_scenario(scenario, &complete).unwrap();
    let seal = session.directory().join("attempts/boot-smoke.json");
    let before = fs::read(&seal).unwrap();
    assert!(session.seal_scenario(scenario, &complete).is_err());
    assert_eq!(fs::read(seal).unwrap(), before);
}

#[test]
fn single_scenario_preserves_its_original_preflight_prepare_event_order() {
    let catalog = catalog();
    let scenario = catalog.get("boot-smoke").expect("boot-smoke");
    let root = tempfile::tempdir().unwrap();
    let mut session = session(root.path());
    let mut fake = FakeSuite::default();
    let results = execute_one(&mut session, &mut fake, scenario).unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(fake.checks, 0);
    assert_eq!(
        fake.log,
        [
            format!("preflight:{}", scenario.id()),
            format!("prepare:{}", scenario.image().id()),
            format!("execute:{}", scenario.id()),
        ]
    );
    let events = fs::read_to_string(session.directory().join("events.jsonl")).unwrap();
    let kinds = events
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap()["kind"].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        &kinds[kinds.len() - 2..],
        ["scenario-started", "scenario-finished"]
    );
}

#[test]
fn cleanup_is_written_before_attachment_indexing_and_preserves_partial_output() {
    hil_core::fixture::cleanup::reset_for_test();
    let output = tempfile::tempdir().unwrap();
    fs::write(output.path().join("partial.json"), b"{\"seen\":true}\n").unwrap();
    let cleanup = hil_core::fixture::cleanup::Scope::new(output.path());
    hil_core::fixture::cleanup::record("restore fixture", || Err("injected cleanup error".into()));
    let started = std::time::Instant::now();
    let result = finalize_repetition(
        1,
        Path::new("scenarios/test/repetition-001"),
        output.path(),
        1,
        started,
        cleanup,
        Outcome::Passed,
        None,
        Vec::new(),
    )
    .unwrap();

    assert_eq!(result.outcome, Outcome::Broken);
    assert_eq!(
        result.failure.as_ref().unwrap().kind,
        FailureKind::Infrastructure
    );
    let names = result
        .attachments
        .iter()
        .map(|attachment| attachment.path.file_name().unwrap().to_string_lossy())
        .collect::<BTreeSet<_>>();
    assert!(names.contains("cleanup.json"));
    assert!(names.contains("partial.json"));
    assert!(hil_core::fixture::cleanup::require_healthy().is_err());
    hil_core::fixture::cleanup::reset_for_test();
}

#[test]
fn cleanup_error_appends_to_the_primary_workload_failure() {
    let mut outcome = Outcome::Failed;
    let mut failure = Some(Failure::new(FailureKind::Scenario, "workload timed out"));
    apply_cleanup_failures(&mut outcome, &mut failure, &["restore failed"]);
    let failure = failure.unwrap();
    assert_eq!(outcome, Outcome::Failed);
    assert_eq!(failure.kind, FailureKind::Scenario);
    assert!(failure.message.contains("workload timed out"));
    assert!(failure.message.contains("restore failed"));
}

#[test]
fn run_all_selection_has_one_ordered_image_plan_and_requirement_union() {
    let catalog = catalog();
    let tags = vec![String::from("qualification")];
    let selected = crate::cli::Selection {
        scenario: None,
        tag: tags,
    }
    .resolve(&catalog)
    .unwrap();
    let groups = group_selected_scenarios(&selected);
    let flattened = groups
        .iter()
        .flat_map(|(_, scenarios)| scenarios.iter().copied())
        .collect::<Vec<_>>();
    assert_eq!(flattened.len(), selected.len());
    assert_eq!(
        crate::scenario::requirements(&selected),
        crate::scenario::requirements(&flattened)
    );
    for scenario in selected {
        assert_eq!(
            flattened
                .iter()
                .filter(|entry| entry.id() == scenario.id())
                .count(),
            1
        );
    }
}
