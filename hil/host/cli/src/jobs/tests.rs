use super::*;

fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

fn job(id: &str, state: JobState) -> Job {
    Job {
        id: id.into(),
        owner: String::from("stand"),
        command: vec![String::from("run"), String::from("icmp-latency")],
        checkout: PathBuf::from("/checkout"),
        after: None,
        after_any: false,
        enqueued_unix_millis: unix_millis(),
        pid: Some(std::process::id()),
        pid_started_unix_millis: oer_hil_arbiter::process_started_unix_millis(std::process::id()),
        log: Some(PathBuf::from("/checkout/target/hil/jobs/x.log")),
        fixed: Vec::new(),
        state,
    }
}

#[test]
fn a_job_names_what_it_was_fixed_with_and_older_records_still_read() {
    let frozen = Frozen {
        cli: PathBuf::from("/c/target/hil/jobs/cli/1/oer-hil-cli"),
        runner: PathBuf::from("/c/target/hil/observers/2/runner"),
        receipt: PathBuf::from("/c/target/hil/observers/2/receipt-3.json"),
        snapshot: Some(PathBuf::from("/c/target/hil/esp32s31/source-snapshots/4")),
    };
    let mut fixed = job("1-a", JobState::Pending);
    fixed.fixed = frozen.paths();
    let text = serde_json::to_string(&fixed).unwrap();
    let read: Job = serde_json::from_str(&text).unwrap();
    assert_eq!(read.fixed.len(), 4);
    assert!(
        read.fixed
            .contains(&PathBuf::from("/c/target/hil/esp32s31/source-snapshots/4"))
    );
    // A record written before jobs were fixed has no such list.
    let mut older: serde_json::Value = serde_json::from_str(&text).unwrap();
    older.as_object_mut().unwrap().remove("fixed");
    let older: Job = serde_json::from_value(older).unwrap();
    assert!(older.fixed.is_empty());
}

#[test]
fn enqueue_and_after_are_taken_before_a_double_dash() {
    let (enqueue, after, rest) = take(args(&[
        "run",
        "s",
        "--enqueue",
        "--after",
        "12-a",
        "--",
        "--enqueue",
    ]))
    .unwrap();
    assert!(enqueue);
    assert_eq!(
        after,
        Some(After {
            job: String::from("12-a"),
            any: false
        })
    );
    assert_eq!(rest, args(&["run", "s", "--", "--enqueue"]));
    let (enqueue, after, _) = take(args(&["run", "--after=7-b"])).unwrap();
    assert!(!enqueue);
    assert_eq!(after.map(|after| after.job), Some(String::from("7-b")));
    let (_, after, _) = take(args(&["run", "--after-any", "8-c"])).unwrap();
    assert_eq!(
        after,
        Some(After {
            job: String::from("8-c"),
            any: true
        })
    );
    assert!(take(args(&["run", "--after", "a", "--after-any", "b"])).is_err());
    assert!(take(args(&["run", "--after"])).is_err());
    assert!(take(args(&["run", "--after", "a", "--after=b"])).is_err());
}

#[test]
fn a_job_outcome_is_the_worst_of_its_runs() {
    assert_eq!(JobOutcome::of_runs(&[]), JobOutcome::NoRun);
    assert_eq!(
        JobOutcome::of_runs(&[Some(Outcome::Passed), Some(Outcome::Passed)]),
        JobOutcome::Passed
    );
    assert_eq!(
        JobOutcome::of_runs(&[Some(Outcome::Passed), Some(Outcome::Failed)]),
        JobOutcome::Failed
    );
    assert_eq!(
        JobOutcome::of_runs(&[Some(Outcome::Failed), Some(Outcome::Broken)]),
        JobOutcome::Broken
    );
    assert_eq!(
        JobOutcome::of_runs(&[Some(Outcome::Skipped)]),
        JobOutcome::Blocked
    );
    assert_eq!(
        JobOutcome::of_runs(&[Some(Outcome::BoardQuarantined), Some(Outcome::Passed)]),
        JobOutcome::Interrupted
    );
    // Every outcome has its own exit code.
    let codes = [
        JobOutcome::Passed,
        JobOutcome::Failed,
        JobOutcome::Interrupted,
        JobOutcome::Blocked,
        JobOutcome::Broken,
        JobOutcome::NoRun,
        JobOutcome::Abandoned,
    ]
    .map(JobOutcome::exit_code);
    assert_eq!(
        codes
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        codes.len()
    );
    assert_eq!(JobOutcome::Passed.exit_code(), 0);
}

#[test]
fn a_job_settles_when_it_finishes_or_its_process_is_gone() {
    let directory = tempfile::tempdir().unwrap();
    let jobs = Jobs::at(directory.path().to_owned());
    jobs.write(&job("1-a", JobState::Started)).unwrap();
    assert_eq!(jobs.settled("1-a").unwrap(), None);
    jobs.write(&job(
        "2-a",
        JobState::Finished {
            outcome: JobOutcome::Failed,
            runs: vec![String::from("r")],
        },
    ))
    .unwrap();
    assert_eq!(jobs.settled("2-a").unwrap(), Some(JobOutcome::Failed));
    assert_eq!(jobs.wait("2-a").unwrap(), JobOutcome::Failed);
    // A recycled PID has another start time.
    let mut gone = job("3-a", JobState::Started);
    gone.pid_started_unix_millis = gone.pid_started_unix_millis.map(|started| started + 5_000);
    jobs.write(&gone).unwrap();
    assert_eq!(jobs.settled("3-a").unwrap(), Some(JobOutcome::Abandoned));
    assert!(jobs.read("nope").is_err());
    assert!(jobs.read("../x").is_err());
    assert_eq!(
        jobs.unfinished()
            .iter()
            .map(|job| job.id.as_str())
            .collect::<Vec<_>>(),
        ["1-a"]
    );
}

#[test]
fn the_queue_names_each_job_s_phase_and_who_waits_for_whom() {
    let mut waiting = job("5-b", JobState::Pending);
    waiting.after = Some(String::from("4-a"));
    let started = job("4-a", JobState::Started);
    let text = describe_views(&[
        JobView {
            job: &started,
            phase: Phase::Building,
        },
        JobView {
            job: &waiting,
            phase: Phase::WaitingForJob,
        },
    ]);
    assert!(
        text.contains("job:     4-a stand `cargo hil run icmp-latency` building images\n"),
        "{text}"
    );
    assert!(
        text.contains("5-b stand `cargo hil run icmp-latency` waits for job 4-a\n"),
        "{text}"
    );
}

#[test]
fn a_job_s_phase_follows_its_process_and_its_runner_in_the_arbiter() {
    let started = job("1-a", JobState::Started);
    let me = started.pid.unwrap();
    let runner = me + 1;
    let parent = |pid: u32| (pid == runner).then_some(me);
    assert_eq!(phase_of(&started, &[], &[], parent), Phase::Building);
    assert_eq!(
        phase_of(&started, &[], &[runner], parent),
        Phase::WaitingForStand
    );
    assert_eq!(phase_of(&started, &[runner], &[], parent), Phase::Holding);
    assert_eq!(phase_of(&started, &[me], &[], parent), Phase::Holding);
    // Another owner's lease is not this job's.
    assert_eq!(phase_of(&started, &[me + 7], &[], parent), Phase::Building);
    let mut after = job("2-a", JobState::Pending);
    after.after = Some(String::from("1-a"));
    assert_eq!(phase_of(&after, &[], &[], parent), Phase::WaitingForJob);
    assert_eq!(
        parent_pid(std::process::id()),
        Some(std::os::unix::process::parent_id())
    );
}

#[test]
fn a_job_whose_process_is_gone_is_recorded_abandoned_and_left_out() {
    let directory = tempfile::tempdir().unwrap();
    let jobs = Jobs::at(directory.path().to_owned());
    let mut gone = job("9-a", JobState::Started);
    gone.pid_started_unix_millis = gone.pid_started_unix_millis.map(|started| started + 5_000);
    jobs.write(&gone).unwrap();
    assert!(jobs.unfinished().is_empty());
    assert_eq!(jobs.settled("9-a").unwrap(), Some(JobOutcome::Abandoned));
}

#[test]
fn only_a_judged_run_lets_a_dependent_start() {
    assert!(JobOutcome::Passed.lets_dependents_start());
    assert!(JobOutcome::Failed.lets_dependents_start());
    for outcome in [
        JobOutcome::NoRun,
        JobOutcome::Blocked,
        JobOutcome::Broken,
        JobOutcome::Interrupted,
        JobOutcome::Abandoned,
    ] {
        assert!(!outcome.lets_dependents_start(), "{outcome}");
    }
}

#[test]
fn the_queue_shows_recent_jobs_that_ended_without_a_judged_run() {
    let directory = tempfile::tempdir().unwrap();
    let jobs = Jobs::at(directory.path().to_owned());
    let log = directory.path().join("x.log");
    std::fs::write(&log, "building\nerror: unknown HIL scenario 'a b'\n\n").unwrap();
    let mut lost = job(
        "1-a",
        JobState::Finished {
            outcome: JobOutcome::NoRun,
            runs: Vec::new(),
        },
    );
    lost.log = Some(log);
    jobs.write(&lost).unwrap();
    jobs.write(&job(
        "2-a",
        JobState::Finished {
            outcome: JobOutcome::Passed,
            runs: vec![String::from("r")],
        },
    ))
    .unwrap();
    let mut old = job(
        "0-a",
        JobState::Finished {
            outcome: JobOutcome::NoRun,
            runs: Vec::new(),
        },
    );
    old.enqueued_unix_millis = 1;
    jobs.write(&old).unwrap();
    let ended = jobs.recently_ended_unjudged(Duration::from_secs(3600), 5);
    assert_eq!(
        ended.iter().map(|job| job.id.as_str()).collect::<Vec<_>>(),
        ["1-a"]
    );
    // A week-old record is gone.
    assert!(jobs.read("0-a").is_err());
    assert!(
        describe_ended(&ended).contains(
            "1-a stand `cargo hil run icmp-latency` no-run: error: unknown HIL scenario 'a b'"
        ),
        "{}",
        describe_ended(&ended)
    );
}
