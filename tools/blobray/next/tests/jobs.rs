#![cfg(target_os = "linux")]
mod support;
use blobray_application::RunRecord;

use blobray_application::{self as app, Application, ImportInput};
use blobray_domain::*;
use blobray_next_host::linux::LinuxHost;
use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

fn application() -> Application {
    Application::new(Arc::new(LinuxHost::new(
        env!("CARGO_BIN_EXE_blobray").into(),
        None,
    )))
}
fn input(path: &Path) -> ImportInput {
    ImportInput {
        role: "vendor".into(),
        path: path.to_owned(),
        expected: None,
    }
}
fn budget() -> ResourceBudget {
    ResourceBudget {
        mode: LimitMode::Watchdog,
        poll_ms: 5,
        grace_ms: 50,
        ..ResourceBudget::default()
    }
}
fn seed(path: &Path) -> Snapshot {
    app::create_project(path).unwrap();
    let source = path.join("source.o");
    let elf = support::elf();
    fs::write(&source, support::archive(&[(b"fixture.o", &elf)], false)).unwrap();
    application()
        .import(path, vec![input(&source)], Target::Riscv32Ilp32, budget())
        .unwrap();
    app::inventory(path, None).unwrap()
}
fn large(path: &Path) {
    fs::write(path, support::elf()).unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_len(128 * 1024 * 1024)
        .unwrap();
}
fn wait_running(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(runs) = app::runs(path)
            && runs.last().is_some_and(|r| r.state == RunState::Running)
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "run did not start: {:?}",
            app::runs(path)
        );
        thread::sleep(Duration::from_millis(1));
    }
}
#[test]
fn sigint_returns_cancelled_without_replacing_current() {
    let temp = tempfile::tempdir().unwrap();
    let before = seed(temp.path());
    let source = temp.path().join("large");
    large(&source);
    let mut child = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args([
            "import",
            "--limit-mode",
            "watchdog",
            "--format",
            "json",
            "--project",
        ])
        .arg(temp.path())
        .arg("--input")
        .arg(format!("vendor={}", source.display()))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    wait_running(temp.path());
    // SAFETY: this test owns the unreaped child process.
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGINT) }, 0);
    let deadline = Instant::now() + Duration::from_secs(5);
    while child.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(json["run"]["state"], "cancelled");
    assert_eq!(app::inventory(temp.path(), None).unwrap(), before);
}
#[test]
fn killed_coordinator_is_recovered_without_reimport_or_revision_loss() {
    let temp = tempfile::tempdir().unwrap();
    let before = seed(temp.path());
    let source = temp.path().join("large");
    large(&source);
    let mut child = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args(["import", "--limit-mode", "watchdog", "--project"])
        .arg(temp.path())
        .arg("--input")
        .arg(format!("vendor={}", source.display()))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    wait_running(temp.path());
    child.kill().unwrap();
    child.wait().unwrap();
    fs::remove_file(&source).unwrap();
    let app = application();
    let deadline = Instant::now() + Duration::from_secs(5);
    let recovered = loop {
        match app.recover(temp.path()) {
            Ok(runs) => break runs,
            Err(Error {
                code: ErrorCode::Busy,
                ..
            }) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Err(error) => panic!("recovery failed: {error}"),
        }
    };
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].state, RunState::Abandoned);
    assert!(app.recover(temp.path()).unwrap().is_empty());
    assert_eq!(app::inventory(temp.path(), None).unwrap(), before);
    assert_eq!(
        fs::read_dir(temp.path().join(".blobray-next/staging"))
            .unwrap()
            .count(),
        0
    );
    let doctor = app::doctor(temp.path()).unwrap();
    assert!(doctor.errors.is_empty());
    assert!(doctor.unfinished_runs.is_empty());
}
#[test]
fn large_real_import_obeys_memory_limit_and_retains_prior_snapshot() {
    let temp = tempfile::tempdir().unwrap();
    let before = seed(temp.path());
    let source = temp.path().join("large");
    large(&source);
    let app = application();
    let handle = app
        .start_import(
            temp.path(),
            vec![input(&source)],
            Target::Riscv32Ilp32,
            ResourceBudget {
                memory_bytes: 16 * 1024 * 1024,
                ..budget()
            },
        )
        .unwrap();
    let result = handle.wait();
    assert_eq!(result.state, RunState::ResourceLimited, "{result:?}");
    assert_eq!(app::inventory(temp.path(), None).unwrap(), before);
    assert_eq!(app::runs(temp.path()).unwrap().last().unwrap(), &result);
}
#[test]
fn unavailable_kernel_backend_is_a_persisted_failure_with_no_fallback() {
    let temp = tempfile::tempdir().unwrap();
    let before = seed(temp.path());
    let root = tempfile::tempdir().unwrap();
    let app = Application::new(Arc::new(LinuxHost::new(
        env!("CARGO_BIN_EXE_blobray").into(),
        Some(root.path().into()),
    )));
    let run = app
        .start_import(
            temp.path(),
            vec![input(&temp.path().join("source.o"))],
            Target::Riscv32Ilp32,
            ResourceBudget::default(),
        )
        .unwrap()
        .wait();
    assert_eq!(run.state, RunState::Failed);
    assert_eq!(run.budget.mode, LimitMode::Kernel);
    assert_eq!(run.error.unwrap().code, ErrorCode::Unavailable);
    assert_eq!(app::inventory(temp.path(), None).unwrap(), before);
}

#[test]
fn work_exhaustion_is_durable_and_cli_preserves_the_selected_snapshot() {
    let temp = tempfile::tempdir().unwrap();
    let before = seed(temp.path());
    let source = temp.path().join("source.o");
    let output = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args([
            "import",
            "--limit-mode",
            "watchdog",
            "--format",
            "json",
            "--max-work-units",
            "1",
            "--project",
        ])
        .arg(temp.path())
        .arg("--input")
        .arg(format!("vendor={}", source.display()))
        .output()
        .unwrap();
    assert!(!output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(value["schema"], 3);
    let record: RunRecord = serde_json::from_value(value["run"].clone()).unwrap();
    assert_eq!(record.schema, 15);
    assert_eq!(record.state, RunState::ResourceLimited);
    assert_eq!(
        record.error.as_ref().unwrap().code,
        ErrorCode::ResourceLimited
    );
    let progress = record.diagnostics.as_ref().unwrap().progress.unwrap();
    assert_eq!(progress.position.phase, RunPhase::Serialize);
    assert_eq!(progress.position.input, None);
    assert_eq!(progress.stop.unwrap().reason, StopReason::Work);
    assert_eq!(app::runs(temp.path()).unwrap().last().unwrap(), &record);
    assert_eq!(app::inventory(temp.path(), None).unwrap(), before);
    assert!(
        fs::read_dir(temp.path().join(".blobray-next/staging"))
            .unwrap()
            .next()
            .is_none()
    );
}

#[test]
fn equal_imports_have_deterministic_charges_and_retention_uses_remaining_budget() {
    let mut charged = Vec::new();
    for _ in 0..2 {
        let temp = tempfile::tempdir().unwrap();
        seed(temp.path());
        let source = temp.path().join("next.o");
        fs::write(&source, support::elf()).unwrap();
        let record = application()
            .import(
                temp.path(),
                vec![input(&source)],
                Target::Riscv32Ilp32,
                budget(),
            )
            .unwrap();
        charged.push(record.diagnostics.unwrap().progress.unwrap().work_used);
    }
    assert_eq!(charged[0], charged[1]);
    let temp = tempfile::tempdir().unwrap();
    let before = seed(temp.path());
    let source = temp.path().join("next.o");
    fs::write(&source, support::elf()).unwrap();
    // The worker succeeds, but the complete import cannot fit the smaller total.
    let app = application();
    let handle = app
        .start_import(
            temp.path(),
            vec![input(&source)],
            Target::Riscv32Ilp32,
            ResourceBudget {
                max_work_units: Some(charged[0] - 1),
                ..budget()
            },
        )
        .unwrap();
    // Keep the application alive while its handle completes.
    let record = handle.wait();
    assert_eq!(record.state, RunState::ResourceLimited);
    assert_eq!(
        record.diagnostics.unwrap().progress.unwrap().position.phase,
        RunPhase::Retain
    );
    assert_eq!(app::inventory(temp.path(), None).unwrap(), before);
}
