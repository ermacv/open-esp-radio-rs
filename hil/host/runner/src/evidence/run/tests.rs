use super::*;
use crate::evidence::build::{SourceLimitation, SourceRebuildStatus, capture_source_material};

fn temporary_directory(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "open-radio-hil-{label}-{}-{}",
        std::process::id(),
        UNIQUE_FILE_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

fn manifest() -> RunManifest {
    RunManifest {
        schema: RUN_SCHEMA,
        run_id: String::from("run<&>"),
        target: String::from("esp32s31"),
        state: RunState::Completed,
        started_unix_millis: 1,
        finished_unix_millis: Some(2),
        duration_millis: Some(1),
        invocation: vec![String::from("cargo hil")],
        repository: RepositoryProvenance {
            commit: String::from("abc123"),
            dirty: false,
            workspace_sha256: String::from("00"),
        },
        runner: RunnerProvenance {
            package: String::from("runner"),
            version: String::from("1"),
            protocol_version: 1,
            host_os: String::from("linux"),
            host_arch: String::from("x86_64"),
            tools: Vec::new(),
        },
        cell: CellProvenance {
            cell_id: String::from("cell-1"),
            device_id: String::from("dut-1"),
            serial_device: PathBuf::from("/dev/ttyACM0"),
        },
        lab_provenance_path: None,
        firmware: Vec::new(),
    }
}

fn failed_suite() -> SuiteResult {
    let failure = Failure::new(FailureKind::Scenario, "bad <frame> & timeout");
    let scenarios = vec![ScenarioResult::from_repetitions(
        String::from("udp-rx"),
        ImageClass::Correctness,
        1,
        vec![RepetitionResult {
            schema: RUN_SCHEMA,
            repetition: 1,
            outcome: Outcome::Failed,
            started_unix_millis: 1,
            duration_millis: 250,
            artifact_directory: PathBuf::from("scenarios/udp-rx/repetition-001"),
            attachments: Vec::new(),
            measurements: vec![
                Measurement::observed("udp.rx.loss", 2, MeasurementUnit::Count)
                    .evaluated(Comparison::AtMost, 0),
            ],
            failure: Some(failure),
        }],
    )];
    SuiteResult {
        schema: RUN_SCHEMA,
        run_id: String::from("run<&>"),
        target: String::from("esp32s31"),
        outcome: Outcome::Failed,
        started_unix_millis: 1,
        finished_unix_millis: 251,
        duration_millis: 250,
        counts: SuiteCounts::from_results(&scenarios),
        scenarios,
    }
}

fn session(directory: &Path) -> RunSession {
    let mut manifest = manifest();
    manifest.run_id = directory
        .file_name()
        .expect("test run directory has a name")
        .to_string_lossy()
        .into_owned();
    manifest.state = RunState::Running;
    manifest.finished_unix_millis = None;
    manifest.duration_millis = None;
    atomic_json(&directory.join("manifest.json"), &manifest).unwrap();
    let repository_root = directory
        .parent()
        .expect("test run directory has a parent")
        .to_owned();
    manifest.repository = RepositoryProvenance {
        commit: String::new(),
        dirty: true,
        workspace_sha256: String::from(
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ),
    };
    RunSession {
        repository_root: repository_root.clone(),
        target_directory: directory
            .parent()
            .expect("test run directory has a parent")
            .to_owned(),
        directory: directory.to_owned(),
        source_materials: vec![SourceMaterial {
            name: String::from("repository"),
            checkout_path: repository_root,
            remote: Some(String::from("https://example.invalid/repository.git")),
            commit: String::new(),
            dirty: true,
            workspace_sha256: String::from(
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            ),
            rebuild_status: SourceRebuildStatus::Incomplete,
            tracked_patch_path: None,
            tracked_patch_size_bytes: None,
            tracked_patch_sha256: None,
            untracked_files: Vec::new(),
            limitations: vec![SourceLimitation::RepositoryStateNotCaptured],
        }],
        manifest,
        started: Instant::now(),
        events: File::create(directory.join("events.jsonl")).unwrap(),
        finished: false,
    }
}

fn integrated_session(target_directory: &Path) -> RunSession {
    let directory = target_directory.join("runs").join(manifest().run_id);
    fs::create_dir_all(&directory).unwrap();
    let mut session = session(&directory);
    session.target_directory = target_directory.to_owned();
    session
}

fn write_test_build_materials(root: &Path) {
    for relative in [
        "Cargo.lock",
        "hil/targets/esp32s31/Cargo.lock",
        "hil/targets/esp32s31/Cargo.toml",
        "hil/targets/esp32s31/stack.toml",
        "platform/esp32s31/partitions/applications.csv",
    ] {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, format!("test material: {relative}\n")).unwrap();
    }
}

#[test]
fn junit_preserves_failure_and_escapes_xml() {
    let xml = render::junit(&failed_suite(), &manifest());
    roxmltree::Document::parse(&xml).expect("valid JUnit XML");
    assert!(xml.contains("tests=\"1\" failures=\"1\""));
    assert!(xml.contains("bad &lt;frame&gt; &amp; timeout"));
    assert!(xml.contains("run_id\" value=\"run&lt;&amp;&gt;"));
    assert!(xml.contains("repetition-001"));
    assert!(xml.contains("measurement.udp.rx.loss=2 count"));
}

#[test]
fn html_is_derived_from_the_same_suite_record() {
    let html = render::html(&failed_suite(), &manifest());
    assert!(html.contains("udp-rx"));
    assert!(html.contains("bad &lt;frame&gt; &amp; timeout"));
    assert!(html.contains("0/1 scenarios passed"));
    assert!(html.contains("udp.rx.loss"));
    assert!(html.contains("&lt;= 0 count"));
}

#[test]
fn evaluated_measurement_binds_threshold_and_verdict() {
    let passed = Measurement::observed("icmp.rtt.p95", 900, MeasurementUnit::Microseconds)
        .evaluated(Comparison::AtMost, 1_000);
    let failed = Measurement::observed("icmp.rtt.p95", 1_001, MeasurementUnit::Microseconds)
        .evaluated(Comparison::AtMost, 1_000);
    assert_eq!(passed.verdict, Some(MeasurementVerdict::Passed));
    assert_eq!(failed.verdict, Some(MeasurementVerdict::Failed));
    assert!(passed.is_consistent());
    assert!(failed.is_consistent());
}

#[test]
fn unique_run_directories_never_replace_a_previous_run() {
    let root = temporary_directory("run");
    let first = create_unique_directory(&root, "123-abc").unwrap();
    fs::write(first.join("evidence"), b"retained").unwrap();
    let second = create_unique_directory(&root, "123-abc").unwrap();
    assert_ne!(first, second);
    assert_eq!(fs::read(first.join("evidence")).unwrap(), b"retained");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn repository_archive_seals_tracked_patch_and_marks_untracked_content_incomplete() {
    let base = temporary_directory("source-archive");
    let repository = base.join("repository");
    fs::create_dir(&repository).unwrap();
    for arguments in [
        &["init", "-q", "-b", "main"][..],
        &["config", "user.name", "HIL Test"],
        &["config", "user.email", "hil@example.invalid"],
        &[
            "remote",
            "add",
            "origin",
            "https://example.invalid/repository.git",
        ],
    ] {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&repository)
                .args(arguments)
                .status()
                .unwrap()
                .success()
        );
    }
    fs::write(repository.join("tracked.txt"), b"base\n").unwrap();
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&repository)
            .args(["add", "tracked.txt"])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&repository)
            .args(["commit", "-m", "base"])
            .status()
            .unwrap()
            .success()
    );
    fs::write(repository.join("tracked.txt"), b"changed\n").unwrap();
    let tracked_run = base.join("tracked-run");
    fs::create_dir(&tracked_run).unwrap();
    let tracked_source = capture_source_material(
        "repository",
        &repository,
        &tracked_run,
        Path::new("source/repository.patch"),
    )
    .unwrap();
    assert!(tracked_source.dirty);
    assert_eq!(
        tracked_source.rebuild_status,
        SourceRebuildStatus::TrackedPatch
    );
    assert!(tracked_source.limitations.is_empty());
    assert!(tracked_source.tracked_patch_size_bytes.unwrap() != 0);
    assert!(
        fs::read_to_string(
            tracked_run.join(tracked_source.tracked_patch_path.expect("tracked patch"))
        )
        .unwrap()
        .contains("+changed")
    );

    fs::write(repository.join("untracked.txt"), b"untracked\n").unwrap();
    let incomplete_run = base.join("incomplete-run");
    fs::create_dir(&incomplete_run).unwrap();
    let incomplete_source = capture_source_material(
        "repository",
        &repository,
        &incomplete_run,
        Path::new("source/repository.patch"),
    )
    .unwrap();
    assert_eq!(
        incomplete_source.rebuild_status,
        SourceRebuildStatus::Incomplete
    );
    assert_eq!(
        incomplete_source.limitations,
        [SourceLimitation::UntrackedContentNotArchived]
    );
    assert_eq!(incomplete_source.untracked_files.len(), 1);
    assert_eq!(
        incomplete_source.untracked_files[0].path,
        Path::new("untracked.txt")
    );
    assert_eq!(incomplete_source.untracked_files[0].size_bytes, 10);
    assert_eq!(incomplete_source.untracked_files[0].sha256.len(), 64);
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn attachments_are_sorted_and_content_addressed() {
    let root = temporary_directory("attachments");
    fs::create_dir(root.join("nested")).unwrap();
    fs::write(root.join("z.log"), b"serial evidence").unwrap();
    fs::write(root.join("nested/capture.pcapng"), b"pcap evidence").unwrap();
    let attachments =
        collect_attachments(&root, Path::new("scenario/repetition-001")).expect("index artifacts");
    assert_eq!(attachments.len(), 2);
    assert_eq!(
        attachments[0].path,
        PathBuf::from("scenario/repetition-001/nested/capture.pcapng")
    );
    assert_eq!(attachments[0].media_type, "application/vnd.tcpdump.pcap");
    assert_eq!(attachments[1].media_type, "text/plain");
    assert_eq!(attachments[1].size_bytes, 15);
    assert_eq!(attachments[1].sha256.len(), 64);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn firmware_record_archives_the_exact_application() {
    let root = temporary_directory("firmware");
    write_test_build_materials(&root);
    let run_directory = root.join("run");
    fs::create_dir(&run_directory).unwrap();
    let application = root.join("application.bin");
    let runtime_elf = root.join("runtime.elf");
    let runtime_bin = root.join("runtime.bin");
    let bootstrap_elf = root.join("bootstrap.elf");
    let effective_embedded_lock = root.join("hil/targets/esp32s31/Cargo.lock");
    fs::write(&application, b"application bytes").unwrap();
    fs::write(&runtime_elf, b"runtime elf").unwrap();
    fs::write(&runtime_bin, b"runtime bin").unwrap();
    fs::write(&bootstrap_elf, b"bootstrap elf").unwrap();

    let mut first_session = session(&run_directory);
    first_session
        .record_firmware(
            ImageClass::Correctness,
            &application,
            &runtime_elf,
            &runtime_bin,
            &bootstrap_elf,
            (&effective_embedded_lock, &effective_embedded_lock),
        )
        .unwrap();
    let artifact = &first_session.manifest.firmware[0];
    assert_eq!(
        artifact.application_path,
        PathBuf::from("firmware/correctness/application.bin")
    );
    assert_eq!(artifact.application_size_bytes, 17);
    assert_eq!(
        fs::read(run_directory.join(&artifact.application_path)).unwrap(),
        b"application bytes"
    );
    assert_eq!(artifact.application_sha256.len(), 64);
    assert_eq!(
        fs::read(
            run_directory.join(
                artifact
                    .runtime_elf_path
                    .as_ref()
                    .expect("runtime ELF path")
            )
        )
        .unwrap(),
        b"runtime elf"
    );
    assert_eq!(artifact.runtime_elf_size_bytes, Some(11));
    assert_eq!(
        fs::read(
            run_directory.join(
                artifact
                    .runtime_bin_path
                    .as_ref()
                    .expect("runtime bin path")
            )
        )
        .unwrap(),
        b"runtime bin"
    );
    assert_eq!(
        fs::read(
            run_directory.join(
                artifact
                    .bootstrap_elf_path
                    .as_ref()
                    .expect("bootstrap ELF path")
            )
        )
        .unwrap(),
        b"bootstrap elf"
    );
    let provenance_path = artifact
        .build_provenance_path
        .as_ref()
        .expect("build provenance path");
    let provenance: crate::evidence::build::BuildProvenance =
        serde_json::from_slice(&fs::read(run_directory.join(provenance_path)).unwrap()).unwrap();
    assert_eq!(provenance.build_id, artifact.build_id.clone().unwrap());
    assert_eq!(provenance.subjects.len(), 4);
    for name in ["embedded-lock", "bootstrap-lock"] {
        assert!(provenance.files.iter().any(|file| file.name == name));
    }
    assert!(!provenance.source_reconstructable);
    let object_root = root.join("objects/sha256");
    let first_objects = collect_integrity_files(&object_root).unwrap();
    assert_eq!(first_objects.len(), 5);
    first_session.finished = true;
    drop(first_session);

    let second_run_directory = root.join("run-2");
    fs::create_dir(&second_run_directory).unwrap();
    let mut second = session(&second_run_directory);
    second
        .record_firmware(
            ImageClass::Correctness,
            &application,
            &runtime_elf,
            &runtime_bin,
            &bootstrap_elf,
            (&effective_embedded_lock, &effective_embedded_lock),
        )
        .unwrap();
    assert_eq!(
        collect_integrity_files(&object_root).unwrap(),
        first_objects
    );
    second.finished = true;
    drop(second);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn replayed_firmware_bundle_is_self_contained_after_origin_removal() {
    let root = temporary_directory("firmware-replay");
    let target_directory = root.join("target/hil/esp32s31");
    let runs_directory = target_directory.join("runs");
    let repository_root = target_directory.clone();
    fs::create_dir_all(&runs_directory).unwrap();
    write_test_build_materials(&repository_root);

    let application = repository_root.join("application.bin");
    let runtime_elf = repository_root.join("runtime.elf");
    let runtime_bin = repository_root.join("runtime.bin");
    let bootstrap_elf = repository_root.join("bootstrap.elf");
    let effective_embedded_lock = repository_root.join("hil/targets/esp32s31/Cargo.lock");
    fs::write(&application, b"application bytes").unwrap();
    fs::write(&runtime_elf, b"runtime elf").unwrap();
    fs::write(&runtime_bin, b"runtime bin").unwrap();
    fs::write(&bootstrap_elf, b"bootstrap elf").unwrap();

    let source_directory = runs_directory.join("source-run");
    fs::create_dir(&source_directory).unwrap();
    let mut source = session(&source_directory);
    source.target_directory = target_directory.clone();
    source.repository_root = repository_root.clone();
    source.source_materials[0].checkout_path = repository_root.clone();
    source
        .record_firmware(
            ImageClass::Correctness,
            &application,
            &runtime_elf,
            &runtime_bin,
            &bootstrap_elf,
            (&effective_embedded_lock, &effective_embedded_lock),
        )
        .unwrap();
    source.finish(Vec::new()).unwrap();

    let archived = crate::evidence::verify::archived_firmware(
        &root,
        "esp32s31",
        "source-run",
        ImageClass::Correctness,
    )
    .unwrap();
    let replay_directory = runs_directory.join("replay-run");
    fs::create_dir(&replay_directory).unwrap();
    let mut replay = session(&replay_directory);
    replay.target_directory = target_directory;
    replay.repository_root = repository_root.clone();
    replay.source_materials[0].checkout_path = repository_root;
    let replayed_application = replay.record_replayed_firmware(&archived).unwrap();
    assert_eq!(
        fs::read(&replayed_application).unwrap(),
        b"application bytes"
    );
    assert_eq!(replay.manifest.firmware.len(), 1);
    assert_eq!(
        replay.manifest.firmware[0]
            .replayed_from
            .as_ref()
            .expect("replay origin")
            .source_run_id,
        "source-run"
    );
    replay.finish(Vec::new()).unwrap();

    fs::remove_dir_all(source_directory).unwrap();
    let verified = crate::evidence::verify::verify(&root, "esp32s31", Some("replay-run")).unwrap();
    assert_eq!(verified.verified_run_ids, ["replay-run"]);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn replay_import_rejects_paths_outside_the_sealed_bundle() {
    assert!(archive::validate_replayed_source_path(Path::new("../application.bin")).is_err());
    assert!(archive::validate_replayed_source_path(Path::new("/tmp/application.bin")).is_err());
    assert!(archive::validate_replayed_source_path(Path::new("firmware/runtime.elf")).is_ok());
}

#[test]
fn outcome_aggregation_is_fail_closed() {
    assert_eq!(aggregate_outcome([]), Outcome::Skipped);
    assert_eq!(aggregate_outcome([Outcome::Passed]), Outcome::Passed);
    assert_eq!(
        aggregate_outcome([Outcome::Passed, Outcome::Blocked]),
        Outcome::Blocked
    );
    assert_eq!(
        aggregate_outcome([Outcome::Failed, Outcome::Interrupted]),
        Outcome::Interrupted
    );
}

#[test]
fn finish_writes_all_views_and_completes_manifest() {
    let root = temporary_directory("finish");
    let scenarios = failed_suite().scenarios;
    let (suite, completion) = integrated_session(&root).finish(scenarios).unwrap();
    assert_eq!(suite.outcome, Outcome::Failed);
    assert!(completion.suite_report.is_file());
    assert!(completion.junit_report.is_file());
    assert!(completion.html_report.is_file());
    assert!(completion.integrity_report.is_file());
    assert!(completion.history_report.as_ref().unwrap().is_file());
    assert!(completion.history_html.as_ref().unwrap().is_file());
    assert!(completion.history_failure.is_none());
    let history: crate::reporting::history::HistoryReport =
        serde_json::from_slice(&fs::read(completion.history_report.as_ref().unwrap()).unwrap())
            .unwrap();
    assert_eq!(history.counts.runs, 1);
    let final_manifest: RunManifest =
        serde_json::from_slice(&fs::read(completion.run_directory.join("manifest.json")).unwrap())
            .unwrap();
    assert_eq!(final_manifest.state, RunState::Completed);
    assert!(final_manifest.finished_unix_millis.is_some());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn dropped_session_marks_manifest_interrupted() {
    let root = temporary_directory("interrupted");
    drop(session(&root));
    let final_manifest: RunManifest =
        serde_json::from_slice(&fs::read(root.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(final_manifest.state, RunState::Interrupted);
    assert!(final_manifest.finished_unix_millis.is_some());
    assert!(root.join("integrity.json").is_file());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn unrelated_history_failure_cannot_revoke_a_sealed_run() {
    for failed in [false, true] {
        let root = temporary_directory("history-failure");
        let session = integrated_session(&root);
        let unrelated = root.join("runs/unrelated-incomplete-bundle");
        fs::create_dir_all(&unrelated).unwrap();
        let scenarios = if failed {
            failed_suite().scenarios
        } else {
            Vec::new()
        };
        let (suite, completion) = session.finish(scenarios).unwrap();
        assert_eq!(
            suite.outcome,
            if failed {
                Outcome::Failed
            } else {
                Outcome::Passed
            }
        );
        assert_eq!(suite.outcome, completion.outcome);
        assert!(completion.history_report.is_none() && completion.history_html.is_none());
        assert!(completion.history_failure.unwrap().contains("no manifest"));
        let manifest: RunManifest = serde_json::from_slice(
            &fs::read(completion.run_directory.join("manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest.state, RunState::Completed);
        let sealed = fs::read(&completion.integrity_report).unwrap();
        fs::remove_dir(unrelated).unwrap();
        crate::reporting::history::rebuild_at(&root, "esp32s31").unwrap();
        assert_eq!(fs::read(completion.integrity_report).unwrap(), sealed);
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn broken_or_interrupted_repetitions_can_retain_failed_measurements() {
    for outcome in [Outcome::Broken, Outcome::Interrupted] {
        let mut suite = failed_suite();
        suite.scenarios[0].repetitions[0].outcome = outcome;
        suite.scenarios[0].outcome = outcome;
        suite.counts = SuiteCounts::from_results(&suite.scenarios);
        let mut manifest = manifest();
        manifest.finished_unix_millis = Some(suite.finished_unix_millis);
        manifest.duration_millis = Some(suite.duration_millis);
        validation::validate_suite(&suite, &manifest).unwrap();
    }
}
