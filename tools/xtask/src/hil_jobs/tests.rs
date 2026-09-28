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
        enqueued_unix_millis: unix_millis(),
        pid: Some(std::process::id()),
        pid_started_unix_millis: oer_hil_arbiter::process_started_unix_millis(std::process::id()),
        log: PathBuf::from("/checkout/target/hil/jobs/x.log"),
        state,
    }
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
    assert_eq!(after.as_deref(), Some("12-a"));
    assert_eq!(rest, args(&["run", "s", "--", "--enqueue"]));
    let (enqueue, after, _) = take(args(&["run", "--after=7-b"])).unwrap();
    assert!(!enqueue);
    assert_eq!(after.as_deref(), Some("7-b"));
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
        ["1-a", "3-a"]
    );
}

#[test]
fn the_queue_names_who_waits_for_whom() {
    let mut waiting = job("5-b", JobState::Pending);
    waiting.after = Some(String::from("4-a"));
    let text = describe(&[job("4-a", JobState::Started), waiting]);
    assert!(
        text.contains("job:     4-a stand `cargo hil run icmp-latency` started\n"),
        "{text}"
    );
    assert!(
        text.contains("5-b stand `cargo hil run icmp-latency` waits for job 4-a\n"),
        "{text}"
    );
}
