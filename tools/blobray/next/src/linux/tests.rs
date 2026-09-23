use super::*;
use std::{
    thread,
    time::{Duration, Instant},
};

#[test]
fn guard_fixture() {
    let Some(stage) = std::env::var_os("BLOBRAY_GUARD_TEST_STAGE") else {
        return;
    };
    let stage = PathBuf::from(stage);
    let config: GuardConfig =
        serde_json::from_slice(&fs::read(stage.join("guard.json")).unwrap()).unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command.args(["--exact", "linux::tests::worker_fixture", "--nocapture"]);
    let report = guard::run_session(&stage, &config, command).unwrap();
    fs::write(
        stage.join("test-report.json"),
        serde_json::to_vec(&report).unwrap(),
    )
    .unwrap();
    std::process::exit(0);
}

#[test]
fn worker_fixture() {
    let Some(stage) = std::env::var_os("BLOBRAY_GUARD_TEST_STAGE") else {
        return;
    };
    let stage = PathBuf::from(stage);
    let kind = std::env::var("BLOBRAY_GUARD_TEST_KIND").unwrap();
    // SAFETY: this test runs only in a dedicated fixture process.
    unsafe {
        libc::signal(libc::SIGTERM, libc::SIG_IGN);
    }
    if kind == "escape" {
        // SAFETY: change only this isolated descendant's session.
        assert!(unsafe { libc::setsid() } > 0);
        fs::write(stage.join("descendant.pid"), std::process::id().to_string()).unwrap();
    } else {
        fs::write(stage.join("worker.pid"), std::process::id().to_string()).unwrap();
        if kind == "tree" {
            // This intentionally orphaned child must be adopted/reaped by the guard.
            #[allow(clippy::zombie_processes)]
            let _child = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "linux::tests::worker_fixture", "--nocapture"])
                .env("BLOBRAY_GUARD_TEST_KIND", "escape")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .spawn()
                .unwrap();
        }
    }
    if matches!(
        kind.as_str(),
        "abort" | "killed" | "missing-report" | "stderr"
    ) {
        if kind == "stderr" {
            use std::io::Write;
            for _ in 0..64 {
                std::io::stderr().write_all(&[b'x'; 4096]).unwrap();
            }
            std::io::stderr().write_all(b"FINAL-STDERR").unwrap();
            std::process::exit(17);
        }
        if kind == "abort" {
            std::process::abort();
        }
        if kind == "killed" {
            // SAFETY: terminate only this dedicated fixture process.
            unsafe {
                libc::kill(std::process::id() as i32, libc::SIGKILL);
            }
        }
        std::process::exit(0);
    }
    if kind == "cooperative" {
        let run: RunId = "01".repeat(32).parse().unwrap();
        let environment = WorkerEnvironment::new(&stage, run).unwrap();
        let mut context = blobray_application::RunContext::new(
            &environment,
            now_ms(),
            now_ms() + 3000,
            &budget(),
            None,
        )
        .unwrap();
        context.phase(RunPhase::Elf).unwrap();
        fs::write(stage.join("cooperative.ready"), []).unwrap();
        let error = loop {
            if let Err(error) = context.checkpoint(1) {
                break error;
            }
        };
        environment.finish(&context.snapshot()).unwrap();
        let report = WorkerReport {
            schema: 6,
            state: RunState::Cancelled,
            error: Some(error),
            prepared: None,
            diagnostics: RunDiagnostics {
                progress: Some(context.snapshot()),
                ..Default::default()
            },
        };
        fs::write(
            stage.join("worker-report.json"),
            serde_json::to_vec(&report).unwrap(),
        )
        .unwrap();
        std::process::exit(0);
    }
    if kind == "spin" {
        loop {
            // Intentionally uncooperative host fixture, not production radio code.
            #[allow(clippy::disallowed_methods)]
            std::hint::spin_loop();
        }
    }
    let allocation = if kind == "allocate" {
        vec![17u8; 64 * 1024 * 1024]
    } else {
        Vec::new()
    };
    loop {
        std::hint::black_box(&allocation);
        thread::sleep(Duration::from_millis(10));
    }
}

struct Fixture {
    stage: tempfile::TempDir,
    child: Child,
    input: Option<ChildStdin>,
}
impl Fixture {
    fn new(kind: &str, budget: ResourceBudget, kernel_root: Option<&Path>) -> Self {
        let stage = tempfile::tempdir().unwrap();
        File::create(stage.path().join("lease.lock")).unwrap();
        let cgroup = kernel_root.map(|root| {
            create_cgroup(root, stage.path().file_name().unwrap(), budget.memory_bytes).unwrap()
        });
        let config = GuardConfig {
            deadline_ms: now_ms() + budget.timeout_ms,
            budget,
            cgroup,
        };
        fs::write(
            stage.path().join("guard.json"),
            serde_json::to_vec(&config).unwrap(),
        )
        .unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "linux::tests::guard_fixture", "--nocapture"])
            .env("BLOBRAY_GUARD_TEST_STAGE", stage.path())
            .env("BLOBRAY_GUARD_TEST_KIND", kind)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let input = child.stdin.take();
        Self {
            stage,
            child,
            input,
        }
    }
    fn ready(&self) -> Vec<u32> {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !self.stage.path().join("descendant.pid").exists() {
            assert!(
                Instant::now() < deadline,
                "fixture did not start descendant"
            );
            thread::sleep(Duration::from_millis(10));
        }
        ["worker.pid", "descendant.pid"]
            .into_iter()
            .map(|p| {
                fs::read_to_string(self.stage.path().join(p))
                    .unwrap()
                    .parse()
                    .unwrap()
            })
            .collect()
    }
    fn result(&mut self) -> WorkerReport {
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.child.try_wait().unwrap().is_none() {
            assert!(Instant::now() < deadline, "guard failed to finish cleanup");
            thread::sleep(Duration::from_millis(10));
        }
        serde_json::from_slice(&fs::read(self.stage.path().join("test-report.json")).unwrap())
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.input.take();
        let _ = self.child.wait();
    }
}
fn budget() -> ResourceBudget {
    ResourceBudget {
        mode: LimitMode::Watchdog,
        poll_ms: 10,
        grace_ms: 20,
        timeout_ms: 3000,
        ..ResourceBudget::default()
    }
}
fn reaped(pids: &[u32]) {
    for pid in pids {
        assert!(
            !Path::new(&format!("/proc/{pid}")).exists(),
            "descendant {pid} was not reaped"
        );
    }
}

#[test]
fn cancellation_kills_and_reaps_descendant_in_a_different_session() {
    let mut fixture = Fixture::new("tree", budget(), None);
    let pids = fixture.ready();
    fixture.input.as_mut().unwrap().write_all(b"c").unwrap();
    assert_eq!(fixture.result().state, RunState::Cancelled);
    reaped(&pids);
}
#[test]
fn coordinator_death_channel_cleans_the_entire_tree() {
    let mut fixture = Fixture::new("tree", budget(), None);
    let pids = fixture.ready();
    fixture.input.take();
    assert_eq!(fixture.result().state, RunState::Cancelled);
    reaped(&pids);
}
#[test]
fn timeout_is_distinct_from_cancellation() {
    let mut fixture = Fixture::new(
        "tree",
        ResourceBudget {
            timeout_ms: 500,
            ..budget()
        },
        None,
    );
    let pids = fixture.ready();
    assert_eq!(fixture.result().state, RunState::TimedOut);
    reaped(&pids);
}
#[test]
fn watchdog_detects_real_memory_allocation() {
    let mut fixture = Fixture::new(
        "allocate",
        ResourceBudget {
            memory_bytes: 16 * 1024 * 1024,
            ..budget()
        },
        None,
    );
    assert_eq!(fixture.result().state, RunState::ResourceLimited);
}
#[test]
fn owner_detection_rejects_pid_reuse_and_previous_boot() {
    let owner = procfs::identity(std::process::id()).unwrap();
    assert!(procfs::alive(&owner).unwrap());
    let mut previous = owner.clone();
    previous.start_ticks += 1;
    assert!(!procfs::alive(&previous).unwrap());
    previous = owner;
    previous.boot_id.push('x');
    assert!(!procfs::alive(&previous).unwrap());
}
#[test]
fn kernel_mode_never_falls_back_to_watchdog() {
    let stage = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let host = LinuxHost::new(std::env::current_exe().unwrap(), Some(root.path().into()));
    assert!(matches!(
        host.launch(stage.path(), &ResourceBudget::default(), now_ms() + 1000),
        Err(Error {
            code: ErrorCode::Unavailable,
            ..
        })
    ));
    assert!(!stage.path().join("guard.json").exists());
}
#[test]
#[ignore = "requires memory delegation with the test process in a coordinator subgroup"]
fn kernel_cgroup_enforces_real_memory_allocation() {
    let current = procfs::current_cgroup().unwrap();
    assert_eq!(current.file_name().unwrap(), "coordinator");
    let root = current.parent().unwrap();
    let mut fixture = Fixture::new(
        "allocate",
        ResourceBudget {
            mode: LimitMode::Kernel,
            memory_bytes: 16 * 1024 * 1024,
            ..budget()
        },
        Some(root),
    );
    assert_eq!(fixture.result().state, RunState::ResourceLimited);
}

#[test]
fn exit_signal_and_missing_report_have_distinct_diagnostics() {
    for (kind, signal, code) in [
        ("abort", Some(libc::SIGABRT), ErrorCode::WorkerExited),
        ("killed", Some(libc::SIGKILL), ErrorCode::WorkerExited),
        ("missing-report", None, ErrorCode::WorkerProtocol),
    ] {
        let result = Fixture::new(kind, budget(), None).result();
        assert_eq!(result.state, RunState::Failed);
        assert_eq!(result.error.unwrap().code, code);
        let exit = result.diagnostics.exit.unwrap();
        assert_eq!(exit.signal, signal);
        assert!(!exit.cgroup_oom, "SIGKILL is not evidence of OOM");
    }
}
#[test]
fn stderr_flood_is_drained_and_only_a_bounded_tail_is_retained() {
    let result = Fixture::new("stderr", budget(), None).result();
    assert_eq!(result.diagnostics.exit.unwrap().code, Some(17));
    assert_eq!(result.diagnostics.stderr_tail.len(), 8192);
    assert!(result.diagnostics.stderr_truncated);
    assert!(result.diagnostics.stderr_tail.ends_with(b"FINAL-STDERR"));
    assert!(serde_json::to_vec(&result).unwrap().len() <= 65536);
}

#[test]
fn uncooperative_cpu_loop_is_forced_to_stop_at_deadline() {
    let mut fixture = Fixture::new(
        "spin",
        ResourceBudget {
            timeout_ms: 300,
            ..budget()
        },
        None,
    );
    let result = fixture.result();
    assert_eq!(result.state, RunState::TimedOut);
    assert_eq!(result.diagnostics.exit.unwrap().signal, Some(libc::SIGKILL));
    let pid = fs::read_to_string(fixture.stage.path().join("worker.pid"))
        .unwrap()
        .parse()
        .unwrap();
    reaped(&[pid]);
}

#[test]
fn cancellation_is_observed_inside_a_cpu_work_loop_before_forced_kill() {
    let mut fixture = Fixture::new(
        "cooperative",
        ResourceBudget {
            grace_ms: 500,
            ..budget()
        },
        None,
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    while !fixture.stage.path().join("cooperative.ready").exists() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    }
    fixture.input.as_mut().unwrap().write_all(b"c").unwrap();
    let result = fixture.result();
    assert_eq!(result.state, RunState::Cancelled);
    let exit = result.diagnostics.exit.unwrap();
    assert_eq!(exit.code, Some(0));
    assert_eq!(exit.signal, None);
    assert_eq!(
        result.diagnostics.progress.unwrap().stop.unwrap().reason,
        StopReason::Cancelled
    );
}
#[test]
fn recovery_can_reclaim_guard_configuration_from_schema_one_runs() {
    let stage = tempfile::tempdir().unwrap();
    fs::write(stage.path().join("guard.json"), br#"{"budget":{"mode":"watchdog","memory_bytes":4096,"timeout_ms":1000,"grace_ms":10,"poll_ms":10},"cgroup":null}"#).unwrap();
    let host = LinuxHost::new(std::env::current_exe().unwrap(), None);
    host.reclaim(stage.path()).unwrap();
}
