#![cfg(target_os = "linux")]
mod support;
use blobray_application as app;
use blobray_domain::*;
use blobray_next_host::linux::LinuxHost;
use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};

fn application(root: &Path, limit: u64) -> app::Application {
    app::Application::with_temporary_storage(
        Arc::new(LinuxHost::new(env!("CARGO_BIN_EXE_blobray").into(), None)),
        app::ApplicationLimits::default(),
        app::TemporaryStoragePolicy {
            root: Some(root.into()),
            operation_bytes: limit,
            total_bytes: 4 * limit,
        },
    )
    .unwrap()
}
fn budget() -> ResourceBudget {
    ResourceBudget {
        mode: LimitMode::Watchdog,
        poll_ms: 2,
        grace_ms: 50,
        ..Default::default()
    }
}
fn seed(base: &Path) -> std::path::PathBuf {
    let project = base.join("project");
    app::create_project(&project).unwrap();
    let source = base.join("source.a");
    let elf = support::elf();
    fs::write(&source, support::archive(&[(b"one.o", &elf)], false)).unwrap();
    application(&base.join("runtime"), 4 * TEMPORARY_CONTROL_BYTES)
        .import(
            &project,
            vec![app::ImportInput {
                role: "input".into(),
                path: source,
                expected: None,
            }],
            Target::Riscv32Ilp32,
            budget(),
        )
        .unwrap();
    project
}
fn contents(root: &Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    let mut result = Vec::new();
    let mut queue = vec![root.to_owned()];
    while let Some(path) = queue.pop() {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                queue.push(path);
            } else {
                result.push((
                    path.strip_prefix(root).unwrap().into(),
                    fs::read(path).unwrap(),
                ));
            }
        }
    }
    result.sort();
    result
}
#[test]
fn cli_disk_exhaustion_is_structured_and_queries_do_not_modify_project() {
    let dir = tempfile::tempdir().unwrap();
    let project = seed(dir.path());
    let before = contents(&project);
    let output = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args([
            "inventory",
            "--format",
            "json",
            "--limit-mode",
            "watchdog",
            "--temporary-mib",
            "1",
            "--temporary-total-mib",
            "1",
            "--temporary-root",
        ])
        .arg(dir.path().join("runtime"))
        .arg("--project")
        .arg(&project)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let report: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(report["query"]["state"], "resource-limited");
    assert_eq!(report["query"]["error"]["storage"]["available_bytes"], 0);
    assert_eq!(contents(&project), before);
    assert_eq!(fs::read_dir(dir.path().join("runtime")).unwrap().count(), 1);
}
#[test]
fn saved_plan_reopens_unchanged_and_local_limit_applies_to_execution() {
    let dir = tempfile::tempdir().unwrap();
    let project = seed(dir.path());
    let before = contents(&project);
    let normal = application(&dir.path().join("runtime"), 4 * TEMPORARY_CONTROL_BYTES);
    let plan = normal
        .plan(
            &project,
            app::PlanRequest {
                revision: None,
                scope: InspectionScope::Revision,
                budget: budget(),
            },
            budget(),
        )
        .unwrap();
    let mut saved = Vec::new();
    plan.write(&mut saved, &|| false).unwrap();
    let reopened = normal
        .reopen_plan(
            &project,
            app::PlanDescription::read(saved.as_slice()).unwrap(),
            budget(),
        )
        .unwrap();
    let mut rewritten = Vec::new();
    reopened.write(&mut rewritten, &|| false).unwrap();
    assert_eq!(saved, rewritten);
    let limited = application(&dir.path().join("low-runtime"), TEMPORARY_CONTROL_BYTES);
    let run = limited.start_run(&reopened).unwrap();
    let record = run.wait();
    assert_eq!(record.state, RunState::ResourceLimited);
    assert!(record.error.unwrap().storage.is_some());
    assert!(run.take_output().is_err());
    assert_eq!(limited.temporary_storage_status().reserved_bytes, 0);
    assert_eq!(saved, serde_json::to_vec(reopened.description()).unwrap());
    assert_eq!(contents(&project), before);
}
// A separate real coordinator process retains a result until the parent kills it.
#[test]
fn retained_result_process_fixture() {
    let Some(base) = std::env::var_os("BLOBRAY_STORAGE_FIXTURE") else {
        return;
    };
    let base = std::path::PathBuf::from(base);
    let app = application(&base.join("runtime"), 4 * TEMPORARY_CONTROL_BYTES);
    let plan = if std::env::var_os("BLOBRAY_STORAGE_PLAN").is_some() {
        Some(
            app.plan(
                &base.join("project"),
                app::PlanRequest {
                    revision: None,
                    scope: InspectionScope::Revision,
                    budget: budget(),
                },
                budget(),
            )
            .unwrap(),
        )
    } else {
        None
    };
    let output = if plan.is_none() {
        Some(
            app.query(
                &base.join("project"),
                app::ReadQuery::Inventory { revision: None },
                budget(),
            )
            .unwrap(),
        )
    } else {
        None
    };
    fs::write(base.join("ready"), b"ready").unwrap();
    std::hint::black_box((&plan, &output));
    loop {
        std::thread::park();
    }
}
#[test]
fn sigkill_coordinator_reclaims_query_and_plan_on_next_runtime_open() {
    for plan in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let project = seed(dir.path());
        let before = contents(&project);
        let root = dir.path().join("runtime");
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "retained_result_process_fixture", "--nocapture"])
            .env("BLOBRAY_STORAGE_FIXTURE", dir.path())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        if plan {
            command.env("BLOBRAY_STORAGE_PLAN", "1");
        }
        let mut child = command.spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !dir.path().join("ready").exists() {
            assert!(
                child.try_wait().unwrap().is_none(),
                "coordinator fixture exited"
            );
            if Instant::now() >= deadline {
                child.kill().unwrap();
                panic!("fixture did not retain result");
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let orphan = fs::read_dir(&root)
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| p.is_dir())
            .unwrap();
        // A second application cannot remove the live coordinator's result.
        let app = application(&root, 4 * TEMPORARY_CONTROL_BYTES);
        drop(
            app.query(&project, app::ReadQuery::Doctor, budget())
                .unwrap(),
        );
        assert!(orphan.exists());
        child.kill().unwrap();
        child.wait().unwrap();
        drop(
            app.query(&project, app::ReadQuery::Doctor, budget())
                .unwrap(),
        );
        assert!(!orphan.exists());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        assert_eq!(contents(&project), before);
    }
}
