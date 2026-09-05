use super::*;
use crate::{
    evidence::run::{
        Comparison, Failure, FailureKind, Measurement, MeasurementUnit, RepetitionResult,
        ScenarioResult,
    },
    image::ImageClass,
};
use std::sync::atomic::{AtomicU64, Ordering};

static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

fn temporary_target(label: &str) -> PathBuf {
    let target = std::env::temp_dir().join(format!(
        "open-radio-hil-history-{label}-{}-{}",
        std::process::id(),
        TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(target.join("runs")).unwrap();
    target
}

fn write_completed_run(target: &Path, run_id: &str, started_unix_millis: u64, outcome: Outcome) {
    let directory = target.join("runs").join(run_id);
    fs::create_dir_all(&directory).unwrap();
    atomic_json(
        &directory.join("manifest.json"),
        &serde_json::json!({
            "schema": RUN_SCHEMA,
            "run_id": run_id,
            "target": "esp32s31",
            "state": "completed",
            "started_unix_millis": started_unix_millis,
            "finished_unix_millis": started_unix_millis + 100,
            "duration_millis": 100,
            "invocation": ["cargo", "hil"],
            "repository": {
                "commit": "0123456789abcdef",
                "dirty": false,
                "workspace_sha256": "00"
            },
            "runner": {
                "package": "runner",
                "version": "1",
                "protocol_version": 1,
                "host_os": "linux",
                "host_arch": "x86_64",
                "tools": []
            },
            "cell": {
                "cell_id": "cell-1",
                "device_id": "dut-1",
                "serial_device": "/dev/ttyACM0"
            },
            "firmware": []
        }),
    )
    .unwrap();
    let scenarios = vec![ScenarioResult::from_repetitions(
        String::from("udp-rx"),
        ImageClass::Correctness,
        1,
        vec![RepetitionResult {
            schema: RUN_SCHEMA,
            repetition: 1,
            outcome,
            started_unix_millis,
            duration_millis: 100,
            artifact_directory: PathBuf::from("scenarios/udp-rx/repetition-001"),
            attachments: Vec::new(),
            measurements: vec![
                Measurement::observed(
                    "icmp.rtt.p95",
                    if outcome == Outcome::Passed {
                        900
                    } else {
                        1_100
                    },
                    MeasurementUnit::Microseconds,
                )
                .evaluated(Comparison::AtMost, 1_000),
            ],
            failure: (outcome != Outcome::Passed)
                .then(|| Failure::new(FailureKind::Scenario, "measurement failed")),
        }],
    )];
    let counts = SuiteCounts {
        scenarios: 1,
        passed: usize::from(outcome == Outcome::Passed),
        failed: usize::from(outcome == Outcome::Failed),
        broken: usize::from(outcome == Outcome::Broken),
        skipped: usize::from(outcome == Outcome::Skipped),
        blocked: usize::from(outcome == Outcome::Blocked),
        interrupted: usize::from(outcome == Outcome::Interrupted),
    };
    atomic_json(
        &directory.join("suite.json"),
        &SuiteResult {
            schema: RUN_SCHEMA,
            run_id: run_id.to_owned(),
            target: String::from("esp32s31"),
            outcome: if outcome == Outcome::Passed {
                Outcome::Passed
            } else {
                Outcome::Failed
            },
            started_unix_millis,
            finished_unix_millis: started_unix_millis + 100,
            duration_millis: 100,
            counts,
            scenarios,
        },
    )
    .unwrap();
}

#[test]
fn history_is_rebuilt_from_runs_and_exposes_flakiness() {
    let target = temporary_target("trends");
    write_completed_run(&target, "run-a", 1, Outcome::Passed);
    write_completed_run(&target, "run-b", 2, Outcome::Failed);

    let completion = rebuild_at(&target, "esp32s31").unwrap();
    let first_json = fs::read(&completion.history_report).unwrap();
    let first_html = fs::read(&completion.html_report).unwrap();
    let report: HistoryReport = read_json(&completion.history_report).unwrap();
    assert_eq!(report.counts.runs, 2);
    assert_eq!(report.counts.passed, 1);
    assert_eq!(report.counts.failed, 1);
    assert_eq!(report.runs[0].run_id, "run-b");
    assert_eq!(report.scenarios.len(), 1);
    assert_eq!(report.scenarios[0].pass_rate_basis_points, 5_000);
    assert!(report.scenarios[0].flaky);
    assert_eq!(report.scenarios[0].consecutive_non_passed, 1);
    assert_eq!(report.scenarios[0].last_outcome, Outcome::Failed);
    assert_eq!(report.measurements.len(), 1);
    assert_eq!(report.measurements[0].minimum, 900);
    assert_eq!(report.measurements[0].latest, 1_100);
    assert_eq!(report.measurements[0].maximum, 1_100);
    assert_eq!(report.measurements[0].failed_verdicts, 1);
    let html = fs::read_to_string(completion.html_report).unwrap();
    assert!(html.contains("Scenario stability"));
    assert!(html.contains("Measurement trends"));
    assert!(html.contains("runs/run-b/report.html"));
    let rebuilt = rebuild_at(&target, "esp32s31").unwrap();
    assert_eq!(fs::read(rebuilt.history_report).unwrap(), first_json);
    assert_eq!(fs::read(rebuilt.html_report).unwrap(), first_html);
    fs::remove_dir_all(target).unwrap();
}

#[test]
fn concurrent_history_publication_waits_for_the_previous_snapshot() {
    use std::{sync::mpsc, time::Duration};
    let target = temporary_target("publication-lock");
    let guard = crate::evidence::run::IndexGuard::acquire(&target).unwrap();
    let (entered, entering) = mpsc::channel();
    let (finished, completion) = mpsc::channel();
    let other_target = target.clone();
    let worker = std::thread::spawn(move || {
        entered.send(()).unwrap();
        let update = crate::evidence::run::IndexGuard::acquire(&other_target).unwrap();
        finished.send(()).unwrap();
        drop(update);
    });
    entering.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(completion.recv_timeout(Duration::from_millis(50)).is_err());
    drop(guard);
    completion.recv_timeout(Duration::from_secs(2)).unwrap();
    worker.join().unwrap();
    fs::remove_dir_all(target).unwrap();
}

#[test]
fn malformed_run_bundle_fails_closed() {
    let target = temporary_target("invalid");
    fs::create_dir(target.join("runs/run-without-manifest")).unwrap();
    let error = rebuild_at(&target, "esp32s31").unwrap_err();
    assert!(error.to_string().contains("has no manifest"));
    fs::remove_dir_all(target).unwrap();
}
