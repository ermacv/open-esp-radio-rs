use super::*;

fn environment() -> Environment {
    Environment {
        common: CommonEnvironment {
            image: "fixture".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            rustc: "rustc".into(),
            cargo: "cargo".into(),
            flags: BTreeMap::new(),
        },
        programs: BTreeMap::new(),
    }
}

fn manifest() -> Manifest {
    Manifest {
        schema: SCHEMA,
        workflow: Workflow::Docs,
        preparation: "prepare".into(),
        commit: "commit".into(),
        tree: "tree".into(),
        environment: environment(),
        jobs: BTreeMap::from([(
            "check".into(),
            Job {
                key: "inputs".into(),
                checks: vec!["doc".into()],
                inputs: 1,
            },
        )]),
    }
}

fn proof() -> Proof {
    Proof {
        schema: SCHEMA,
        workflow: Workflow::Docs,
        run_id: 10,
        attempt: 1,
        commit: "old".into(),
        tree: "tree".into(),
        jobs: BTreeMap::from([(
            "check".into(),
            Record {
                inputs: manifest().jobs["check"].clone(),
                verified_at: 1000,
                run_id: 10,
            },
        )]),
    }
}

fn needs(result: JobResult) -> BTreeMap<String, Need> {
    BTreeMap::from([
        (
            "prepare".into(),
            Need {
                reusable: true,
                result: JobResult::Success,
            },
        ),
        (
            "check".into(),
            Need {
                result,
                reusable: true,
            },
        ),
    ])
}

#[test]
fn missing_or_different_inputs_run_even_when_a_previous_run_was_green() {
    for proofs in [
        vec![],
        {
            let mut proof = proof();
            proof.schema = SCHEMA + 1;
            vec![proof]
        },
        {
            let mut proof = proof();
            proof.workflow = Workflow::Ci;
            vec![proof]
        },
        {
            let mut proof = proof();
            proof.jobs.get_mut("check").unwrap().inputs.key = "changed".into();
            vec![proof]
        },
        {
            let mut proof = proof();
            proof
                .jobs
                .get_mut("check")
                .unwrap()
                .inputs
                .checks
                .push("new check".into());
            vec![proof]
        },
    ] {
        let plan = select(manifest(), 20, &proofs, Mode::Reuse, 1100, None).unwrap();
        assert!(plan.actions["check"].run);
        assert!(finish(&plan, &needs(JobResult::Success), 20, 1, 1101).is_ok());
        for result in [JobResult::Skipped, JobResult::Cancelled, JobResult::Failure] {
            assert!(finish(&plan, &needs(result), 20, 1, 1101).is_err());
        }
    }
}

#[test]
fn observation_and_full_execute_every_job() {
    for mode in [Mode::Observe, Mode::Full] {
        let plan = select(manifest(), 20, &[proof()], mode, 1100, None).unwrap();
        assert!(plan.actions["check"].run);
        assert!(finish(&plan, &needs(JobResult::Skipped), 20, 1, 1101).is_err());
        assert!(finish(&plan, &needs(JobResult::Success), 20, 1, 1101).is_ok());
        assert_eq!(
            plan.actions["check"].source.is_some(),
            mode == Mode::Observe
        );
    }
}

#[test]
fn reuse_requires_explicit_successful_coverage_and_preserves_its_age() {
    let mut plan = select(manifest(), 20, &[proof()], Mode::Reuse, 1100, None).unwrap();
    assert!(!plan.actions["check"].run);
    let result = finish(&plan, &needs(JobResult::Skipped), 20, 1, 1101).unwrap();
    assert_eq!(result.jobs["check"].verified_at, 1000);
    assert_eq!(result.jobs["check"].run_id, 10);
    for outcome in [JobResult::Failure, JobResult::Cancelled, JobResult::Success] {
        assert!(finish(&plan, &needs(outcome), 20, 1, 1101).is_err());
    }
    assert!(finish(&plan, &needs(JobResult::Skipped), 20, 1, 1000 + MAX_AGE + 1).is_err());
    plan.actions.get_mut("check").unwrap().source = None;
    assert!(finish(&plan, &needs(JobResult::Skipped), 20, 1, 1101).is_err());
}

#[test]
fn expired_future_or_incomplete_evidence_never_allows_skipping() {
    for at in [999, 1000 + MAX_AGE + 1] {
        assert!(
            select(manifest(), 20, &[proof()], Mode::Reuse, at, None)
                .unwrap()
                .actions["check"]
                .run
        );
    }
    let plan = select(manifest(), 20, &[], Mode::Full, 1100, None).unwrap();
    for result in [JobResult::Skipped, JobResult::Failure, JobResult::Cancelled] {
        let mut needs = needs(JobResult::Success);
        needs.get_mut("prepare").unwrap().result = result;
        assert!(finish(&plan, &needs, 20, 1, 1101).is_err());
    }
    let mut needs = needs(JobResult::Success);
    needs.remove("check");
    assert!(finish(&plan, &needs, 20, 1, 1101).is_err());
    assert!("unexpected".parse::<Mode>().is_err());
}

#[test]
fn proof_provenance_binds_the_workflow_run_attempt_and_commit() {
    let proof = proof();
    assert!(belongs_to(&proof, Workflow::Docs, 10, 1, "old"));
    for (workflow, run, attempt, commit) in [
        (Workflow::Ci, 10, 1, "old"),
        (Workflow::Docs, 11, 1, "old"),
        (Workflow::Docs, 10, 2, "old"),
        (Workflow::Docs, 10, 1, "other"),
    ] {
        assert!(!belongs_to(&proof, workflow, run, attempt, commit));
    }
}

#[test]
fn environment_drift_passes_successful_jobs_without_issuing_coverage() {
    for mode in [Mode::Observe, Mode::Full, Mode::Reuse] {
        let plan = select(manifest(), 20, &[], mode, 1100, None).unwrap();
        let mut outcomes = needs(JobResult::Success);
        outcomes.get_mut("check").unwrap().reusable = false;
        let proof = finish(&plan, &outcomes, 20, 2, 1101).unwrap();
        assert!(proof.jobs.is_empty());
        assert_eq!(proof.attempt, 2);
        assert!(
            select(manifest(), 30, &[proof], Mode::Reuse, 1102, None)
                .unwrap()
                .actions["check"]
                .run
        );
        for status in [JobResult::Failure, JobResult::Cancelled, JobResult::Skipped] {
            outcomes.get_mut("check").unwrap().result = status;
            assert!(finish(&plan, &outcomes, 20, 2, 1101).is_err());
        }
    }
}

/// A manifest whose job has two checks, so a change can reach one of them.
fn two_checks() -> Manifest {
    let mut manifest = manifest();
    manifest.jobs.get_mut("check").unwrap().checks = vec!["doc".into(), "docs".into()];
    manifest
}

fn scope(checks: &[&str]) -> Scope {
    Scope {
        base: "base".into(),
        checks: BTreeMap::from([(
            "check".into(),
            checks.iter().map(|check| (*check).to_owned()).collect(),
        )]),
    }
}

#[test]
fn a_change_that_reaches_no_check_skips_the_job_and_proves_nothing() {
    let plan = select(two_checks(), 20, &[], Mode::Reuse, 1100, Some(&scope(&[]))).unwrap();
    assert!(plan.actions["check"].unaffected());
    assert_eq!(plan.change_base.as_deref(), Some("base"));
    validate_plan(&plan).unwrap();
    let proof = finish(&plan, &needs(JobResult::Skipped), 20, 1, 1100).unwrap();
    assert!(proof.jobs.is_empty());
    // The skipped job must not have run, and only a change-scoped plan skips.
    assert!(finish(&plan, &needs(JobResult::Success), 20, 1, 1100).is_err());
    let mut whole = plan.clone();
    whole.change_base = None;
    assert!(validate_plan(&whole).is_err());
}

#[test]
fn a_change_that_reaches_some_checks_runs_them_without_proving_the_job() {
    let plan = select(
        two_checks(),
        20,
        &[],
        Mode::Reuse,
        1100,
        Some(&scope(&["docs"])),
    )
    .unwrap();
    let action = &plan.actions["check"];
    assert!(action.run);
    assert_eq!(action.checks, Some(vec!["docs".to_owned()]));
    let proof = finish(&plan, &needs(JobResult::Success), 20, 1, 1100).unwrap();
    assert!(
        proof.jobs.is_empty(),
        "a partial run is no coverage to reuse"
    );
    assert!(finish(&plan, &needs(JobResult::Failure), 20, 1, 1100).is_err());
    // A change that reaches every check runs the job as a whole run would.
    let every = scope(&["doc", "docs"]);
    let plan = select(two_checks(), 20, &[], Mode::Reuse, 1100, Some(&every)).unwrap();
    assert_eq!(plan.actions["check"].checks, None);
    let proof = finish(&plan, &needs(JobResult::Success), 20, 1, 1100).unwrap();
    assert_eq!(proof.jobs.len(), 1);
}

#[test]
fn reuse_covers_a_job_before_its_change_selection() {
    let plan = select(
        manifest(),
        20,
        &[proof()],
        Mode::Reuse,
        1100,
        Some(&scope(&[])),
    )
    .unwrap();
    let action = &plan.actions["check"];
    assert!(!action.run && action.source.is_some() && action.checks.is_none());
    finish(&plan, &needs(JobResult::Skipped), 20, 1, 1100).unwrap();
}

#[test]
fn a_selection_of_checks_the_job_lacks_is_rejected() {
    let docs = scope(&["docs"]);
    let mut plan = select(two_checks(), 20, &[], Mode::Full, 1100, Some(&docs)).unwrap();
    validate_plan(&plan).unwrap();
    plan.actions.get_mut("check").unwrap().checks = Some(vec!["elsewhere".into()]);
    assert!(validate_plan(&plan).is_err());
}
