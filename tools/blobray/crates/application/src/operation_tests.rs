use super::*;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

struct Host {
    started: Instant,
    release: Arc<AtomicBool>,
    launched: Arc<AtomicUsize>,
    reaped: Arc<AtomicUsize>,
    cleaning: Arc<AtomicUsize>,
    cleanup_release: Arc<AtomicBool>,
    fail: AtomicBool,
}
impl Host {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            started: Instant::now(),
            release: Arc::new(AtomicBool::new(true)),
            launched: Arc::new(AtomicUsize::new(0)),
            reaped: Arc::new(AtomicUsize::new(0)),
            cleaning: Arc::new(AtomicUsize::new(0)),
            cleanup_release: Arc::new(AtomicBool::new(true)),
            fail: AtomicBool::new(false),
        })
    }
}
impl RunEnvironment for Host {
    fn now_ms(&self) -> u64 {
        OperationHost::now_ms(self)
    }
    fn cancelled(&self) -> bool {
        false
    }
    fn observe(&self, _: &RunProgress) -> Result<()> {
        Ok(())
    }
}
struct Worker {
    release: Arc<AtomicBool>,
    reaped: Arc<AtomicUsize>,
    cleaning: Arc<AtomicUsize>,
    cleanup_release: Arc<AtomicBool>,
    execute: Option<Box<dyn FnOnce() -> Result<WorkerReport> + Send>>,
    cancel: bool,
    progress: AtomicU64,
}
impl OperationWorker for Worker {
    fn poll(&mut self) -> Result<Option<WorkerReport>> {
        if self.cancel {
            return Ok(Some(WorkerReport {
                schema: 6,
                state: RunState::Cancelled,
                error: Some(Error::new(ErrorCode::Cancelled, "fixture cancelled")),
                prepared: None,
                diagnostics: RunDiagnostics::default(),
            }));
        }
        if !self.release.load(Ordering::SeqCst) {
            return Ok(None);
        }
        self.execute.take().unwrap()().map(Some)
    }
    fn cancel(&mut self) -> Result<()> {
        self.cancel = true;
        Ok(())
    }
    fn progress(&self) -> Result<Option<RunProgress>> {
        // Observe progress only while blocked. Completed fixtures return their real counters.
        if self.release.load(Ordering::SeqCst) {
            return Ok(None);
        }
        let sequence = self.progress.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(Some(RunProgress {
            sequence,
            ..Default::default()
        }))
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.cleaning.fetch_add(1, Ordering::SeqCst);
        wait_for(|| self.cleanup_release.load(Ordering::SeqCst));
        self.reaped.fetch_add(1, Ordering::SeqCst);
    }
}
impl OperationHost for Host {
    fn temporary_root(&self, requested: Option<&Path>) -> Result<PathBuf> {
        let root = requested.map(Path::to_owned).unwrap_or_else(|| {
            std::env::temp_dir().join(format!("blobray-test-runtime-{}", std::process::id()))
        });
        std::fs::create_dir_all(&root).map_err(storage_io)?;
        Ok(root)
    }
    fn now_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }
    fn save_progress(&self, _: &Path, _: &ProgressRecord) -> Result<()> {
        Ok(())
    }
    fn owner(&self) -> Result<OwnerIdentity> {
        Ok(OwnerIdentity {
            pid: 1,
            start_ticks: 1,
            boot_id: "test".into(),
        })
    }
    fn alive(&self, _: &OwnerIdentity) -> Result<bool> {
        Ok(false)
    }
    fn reclaim(&self, _: &Path) -> Result<()> {
        Ok(())
    }
    fn launch(&self, stage: &Path, _: &ResourceBudget, _: u64) -> Result<Box<dyn OperationWorker>> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(Error::new(ErrorCode::Unavailable, "fixture launch refused"));
        }
        let stage = stage.to_owned();
        let started = self.started;
        let execute = Box::new(move || {
            struct Environment(Instant);
            impl RunEnvironment for Environment {
                fn now_ms(&self) -> u64 {
                    self.0.elapsed().as_millis() as u64
                }
                fn cancelled(&self) -> bool {
                    false
                }
                fn observe(&self, _: &RunProgress) -> Result<()> {
                    Ok(())
                }
            }
            let env = Environment(started);
            let (result, progress) = if stage.join("query.json").exists() {
                let work: QueryWork =
                    serde_json::from_slice(&std::fs::read(stage.join("query.json")).unwrap())
                        .unwrap();
                let mut control =
                    RunContext::new(&env, work.started_ms, work.deadline_ms, &work.budget, None)?;
                let result = prepare_query(&stage, &work, &mut control).map(|_| None);
                (result, control.snapshot())
            } else {
                let work: ImportWork =
                    serde_json::from_slice(&std::fs::read(stage.join("request.json")).unwrap())
                        .unwrap();
                let mut control =
                    RunContext::new(&env, work.started_ms, work.deadline_ms, &work.budget, None)?;
                let result = prepare_import(&stage, work, &mut control)
                    .map(|p| Some(PreparedReceipt::Import(p)));
                (result, control.snapshot())
            };
            let (state, prepared, error) = match result {
                Ok(prepared) => (RunState::Completed, prepared, None),
                Err(error) => (failure_state(&error), None, Some(error)),
            };
            Ok(WorkerReport {
                schema: 6,
                state,
                prepared,
                error,
                diagnostics: RunDiagnostics {
                    progress: Some(progress),
                    ..Default::default()
                },
            })
        });
        self.launched.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(Worker {
            release: self.release.clone(),
            reaped: self.reaped.clone(),
            cleaning: self.cleaning.clone(),
            cleanup_release: self.cleanup_release.clone(),
            execute: Some(execute),
            cancel: false,
            progress: AtomicU64::new(0),
        }))
    }
}
fn budget() -> ResourceBudget {
    ResourceBudget {
        mode: LimitMode::Watchdog,
        poll_ms: 1,
        ..Default::default()
    }
}
fn seed(app: &Application, project: &Path, bytes: &[u8]) -> RevisionId {
    let path = project.join("input.o");
    std::fs::write(&path, bytes).unwrap();
    app.import(
        project,
        vec![ImportInput {
            role: "input".into(),
            path,
            expected: None,
        }],
        Target::Riscv32Ilp32,
        budget(),
    )
    .unwrap()
    .revision
    .unwrap()
}
fn wait_for(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready() {
        assert!(Instant::now() < deadline, "fixture deadline");
        thread::sleep(Duration::from_millis(1));
    }
}
#[test]
fn common_shutdown_cancels_import_and_query_after_client_handles_are_dropped() {
    let directory = tempfile::tempdir().unwrap();
    create_project(directory.path()).unwrap();
    let host = Host::new();
    let app = Application::new(host.clone());
    let first = seed(&app, directory.path(), b"original");
    host.release.store(false, Ordering::SeqCst);
    let query = app
        .start_query(
            directory.path(),
            ReadQuery::Inventory { revision: None },
            budget(),
        )
        .unwrap();
    let import = app
        .start_import(
            directory.path(),
            vec![ImportInput {
                role: "input".into(),
                path: directory.path().join("input.o"),
                expected: None,
            }],
            Target::Riscv32Ilp32,
            budget(),
        )
        .unwrap();
    wait_for(|| host.launched.load(Ordering::SeqCst) == 3);
    drop(query);
    drop(import);
    app.shutdown();
    assert_eq!(host.reaped.load(Ordering::SeqCst), 3);
    assert_eq!(
        inventory(directory.path(), None).unwrap().revision_id,
        first
    );
    let records = runs(directory.path()).unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(records[1].state, RunState::Cancelled);
    assert!(matches!(
        app.start_query(directory.path(), ReadQuery::Doctor, budget()),
        Err(Error {
            code: ErrorCode::InvalidRequest,
            ..
        })
    ));
}
#[test]
fn query_selection_is_fixed_before_worker_reads_and_output_transfers_once() {
    let directory = tempfile::tempdir().unwrap();
    create_project(directory.path()).unwrap();
    let host = Host::new();
    let app = Application::new(host.clone());
    let first = seed(&app, directory.path(), b"original");
    host.release.store(false, Ordering::SeqCst);
    let query = app
        .start_query(
            directory.path(),
            ReadQuery::Inventory { revision: None },
            budget(),
        )
        .unwrap();
    let other = Application::new(Host::new());
    let second = seed(&other, directory.path(), b"changed");
    assert_ne!(first, second);
    host.release.store(true, Ordering::SeqCst);
    assert_eq!(query.wait().state, RunState::Completed);
    let mut output = query.take_output().unwrap();
    assert!(matches!(
        query.take_output(),
        Err(Error {
            code: ErrorCode::InvalidRequest,
            ..
        })
    ));
    assert_eq!(query.wait().state, RunState::Completed);
    output
        .manifest(&|| false, |summary, source, control| {
            assert!(
                matches!(summary,QuerySummary::Inventory {revision_id,..} if revision_id==&first)
            );
            assert_eq!(hash_source(source, control)?.as_str(), first.as_str());
            Ok(())
        })
        .unwrap();
    assert!(output.manifest(&|| false, |_, _, _| Ok(())).is_err());
    assert_eq!(runs(directory.path()).unwrap().len(), 2);
}
#[test]
fn admission_accounts_for_retained_results_and_rejects_oversized_messages() {
    let directory = tempfile::tempdir().unwrap();
    create_project(directory.path()).unwrap();
    let host = Host::new();
    let app = Application::with_limits(
        host,
        ApplicationLimits {
            max_operations: 1,
            event_capacity: 3,
        },
    )
    .unwrap();
    seed(&app, directory.path(), b"original");
    let handle = app
        .start_query(directory.path(), ReadQuery::Doctor, budget())
        .unwrap();
    handle.wait();
    let output = handle.take_output().unwrap();
    drop(handle);
    assert!(matches!(
        app.start_query(directory.path(), ReadQuery::Doctor, budget()),
        Err(Error {
            code: ErrorCode::Busy,
            ..
        })
    ));
    drop(output);
    let next = app
        .start_query(directory.path(), ReadQuery::Doctor, budget())
        .unwrap();
    next.wait();
    drop(next);
    let before = runs(directory.path()).unwrap().len();
    let error = app.start_import(
        directory.path(),
        vec![ImportInput {
            role: "x".repeat(65537),
            path: "input".into(),
            expected: None,
        }],
        Target::Riscv32Ilp32,
        budget(),
    );
    assert!(matches!(
        error,
        Err(Error {
            code: ErrorCode::InvalidRequest,
            ..
        })
    ));
    assert_eq!(runs(directory.path()).unwrap().len(), before);
}
#[test]
fn bounded_events_retain_terminal_status_and_launch_failure_does_not_journal_query() {
    let directory = tempfile::tempdir().unwrap();
    create_project(directory.path()).unwrap();
    let host = Host::new();
    let app = Application::with_limits(
        host.clone(),
        ApplicationLimits {
            max_operations: 2,
            event_capacity: 3,
        },
    )
    .unwrap();
    host.release.store(false, Ordering::SeqCst);
    let query = app
        .start_query(directory.path(), ReadQuery::Doctor, budget())
        .unwrap();
    wait_for(|| query.events(0).last().is_some_and(|e| e.sequence > 8));
    assert_eq!(query.events(0).len(), 3);
    assert!(query.cancel());
    assert_eq!(query.wait().state, RunState::Cancelled);
    assert_eq!(query.events(0).last().unwrap().state, RunState::Cancelled);
    assert!(!query.cancel());
    drop(query);
    host.fail.store(true, Ordering::SeqCst);
    let failed = app
        .start_query(directory.path(), ReadQuery::Doctor, budget())
        .unwrap();
    assert_eq!(failed.wait().error.unwrap().code, ErrorCode::Unavailable);
    assert!(runs(directory.path()).unwrap().is_empty());
}

#[test]
fn query_delivery_continues_work_and_preserves_failure_without_retry() {
    let directory = tempfile::tempdir().unwrap();
    create_project(directory.path()).unwrap();
    let app = Application::new(Host::new());
    seed(&app, directory.path(), b"original");
    let mut output = app
        .query(
            directory.path(),
            ReadQuery::Inventory { revision: None },
            budget(),
        )
        .unwrap();
    let prior = output.report.diagnostics.progress.unwrap();
    assert!(prior.work_used > 0);
    let remaining = budget().max_work_units.unwrap() - prior.work_used;
    let error = output
        .manifest(&|| false, |_, _, control| control.checkpoint(remaining + 1))
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::ResourceLimited);
    let progress = output.report.diagnostics.progress.unwrap();
    assert_eq!(progress.work_used, prior.work_used);
    assert!(
        progress.working_memory.unwrap().peak_reserved_bytes
            >= prior.working_memory.unwrap().peak_reserved_bytes
    );
    assert_eq!(
        output
            .manifest(&|| false, |_, _, _| panic!("retry must not run"))
            .unwrap_err()
            .code,
        ErrorCode::InvalidRequest
    );

    let mut output = app
        .query(
            directory.path(),
            ReadQuery::Inventory { revision: None },
            budget(),
        )
        .unwrap();
    let error = output
        .manifest(&|| false, |_, _, _| {
            Err(Error::new(ErrorCode::Io, "destination closed"))
        })
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Io);
    assert_eq!(error.message, "destination closed");
    assert_eq!(
        output
            .manifest(&|| false, |_, _, _| panic!("retry must not run"))
            .unwrap_err()
            .code,
        ErrorCode::InvalidRequest
    );
}

#[test]
fn shutdown_rejects_admission_while_worker_cleanup_is_still_running() {
    let directory = tempfile::tempdir().unwrap();
    create_project(directory.path()).unwrap();
    let host = Host::new();
    host.release.store(false, Ordering::SeqCst);
    host.cleanup_release.store(false, Ordering::SeqCst);
    let app = Arc::new(Application::new(host.clone()));
    let query = app
        .start_query(directory.path(), ReadQuery::Doctor, budget())
        .unwrap();
    wait_for(|| host.launched.load(Ordering::SeqCst) == 1);
    let closing = app.clone();
    let shutdown = thread::spawn(move || closing.shutdown());
    wait_for(|| host.cleaning.load(Ordering::SeqCst) == 1);
    let (send, receive) = std::sync::mpsc::channel();
    let path = directory.path().to_owned();
    let caller = app.clone();
    let admission = thread::spawn(move || {
        send.send(
            caller
                .start_query(&path, ReadQuery::Doctor, budget())
                .err()
                .map(|e| e.code),
        )
        .unwrap();
    });
    let result = receive.recv_timeout(Duration::from_secs(1));
    // Release the fixture even if admission incorrectly waited on teardown.
    host.cleanup_release.store(true, Ordering::SeqCst);
    admission.join().unwrap();
    shutdown.join().unwrap();
    assert_eq!(result.unwrap(), Some(ErrorCode::InvalidRequest));
    assert_eq!(host.reaped.load(Ordering::SeqCst), 1);
    assert_eq!(query.wait().state, RunState::Cancelled);
}

fn disk_app(root: &Path, operation_bytes: u64, total_bytes: u64) -> Application {
    Application::with_temporary_storage(
        Host::new(),
        ApplicationLimits::default(),
        TemporaryStoragePolicy {
            root: Some(root.into()),
            operation_bytes,
            total_bytes,
        },
    )
    .unwrap()
}
#[test]
fn capture_capacity_failure_is_durable_and_does_not_publish() {
    let directory = tempfile::tempdir().unwrap();
    let project = directory.path().join("project");
    create_project(&project).unwrap();
    let root = directory.path().join("runtime");
    let app = disk_app(
        &root,
        TEMPORARY_CONTROL_BYTES + 1024,
        TEMPORARY_CONTROL_BYTES + 1024,
    );
    let source = directory.path().join("source");
    std::fs::write(&source, vec![0; 4096]).unwrap();
    let record = app
        .start_import(
            &project,
            vec![ImportInput {
                role: "input".into(),
                path: source,
                expected: None,
            }],
            Target::Riscv32Ilp32,
            budget(),
        )
        .unwrap()
        .wait();
    assert_eq!(record.state, RunState::ResourceLimited);
    let storage = record.error.as_ref().unwrap().storage.as_ref().unwrap();
    assert_eq!(storage.position.phase, RunPhase::Capture);
    assert_eq!(storage.owner.as_ref(), Some(&record.id));
    assert_eq!(runs(&project).unwrap()[0].error, record.error);
    assert!(revisions(&project).unwrap().is_empty());
    assert_eq!(app.temporary_storage_status().reserved_bytes, 0);
    assert!(
        record
            .diagnostics
            .unwrap()
            .progress
            .unwrap()
            .temporary_storage
            .is_some()
    );
}
#[test]
fn query_spool_limit_returns_no_output_and_removes_workspace() {
    let directory = tempfile::tempdir().unwrap();
    let project = directory.path().join("project");
    create_project(&project).unwrap();
    seed(&Application::new(Host::new()), &project, b"seed");
    let root = directory.path().join("runtime");
    let app = disk_app(&root, TEMPORARY_CONTROL_BYTES, TEMPORARY_CONTROL_BYTES);
    let handle = app
        .start_query(&project, ReadQuery::Inventory { revision: None }, budget())
        .unwrap();
    let record = handle.wait();
    assert_eq!(record.state, RunState::ResourceLimited);
    assert!(record.error.unwrap().storage.is_some());
    assert!(handle.take_output().is_err());
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
    assert_eq!(app.temporary_storage_status().reserved_bytes, 0);
}
#[test]
fn plan_clones_hold_disk_capacity_and_policy_does_not_change_identity() {
    let directory = tempfile::tempdir().unwrap();
    let project = directory.path().join("project");
    create_project(&project).unwrap();
    seed(&Application::new(Host::new()), &project, b"seed");
    let limit = TEMPORARY_CONTROL_BYTES + 65536;
    let root = directory.path().join("runtime");
    let app = disk_app(&root, limit, limit);
    let request = PlanRequest {
        revision: None,
        scope: InspectionScope::Revision,
        budget: budget(),
    };
    let plan = app.plan(&project, request.clone(), budget()).unwrap();
    let retained = app.temporary_storage_status().reserved_bytes;
    assert!(retained > 0 && retained < limit);
    assert!(matches!(
        app.start_query(&project, ReadQuery::Doctor, budget()),
        Err(Error {
            code: ErrorCode::ResourceLimited,
            ..
        })
    ));
    let clone = plan.clone();
    let mut bytes = Vec::new();
    plan.write(&mut bytes, &|| false).unwrap();
    drop(plan);
    assert_eq!(app.temporary_storage_status().reserved_bytes, retained);
    let other = disk_app(&directory.path().join("runtime2"), 2 * limit, 4 * limit);
    let other_plan = other.plan(&project, request, budget()).unwrap();
    let mut other_bytes = Vec::new();
    other_plan.write(&mut other_bytes, &|| false).unwrap();
    assert_eq!(bytes, other_bytes);
    drop(clone);
    app.shutdown();
    assert_eq!(app.temporary_storage_status().reserved_bytes, 0);
}

#[test]
fn manifest_capacity_failure_does_not_publish_after_capture_succeeds() {
    let directory = tempfile::tempdir().unwrap();
    let project = directory.path().join("project");
    create_project(&project).unwrap();
    let source = directory.path().join("source");
    std::fs::write(&source, b"seed").unwrap();
    let limit = TEMPORARY_CONTROL_BYTES + 512;
    let app = disk_app(&directory.path().join("runtime"), limit, limit);
    let record = app
        .start_import(
            &project,
            vec![ImportInput {
                role: "x".repeat(500),
                path: source,
                expected: None,
            }],
            Target::Riscv32Ilp32,
            budget(),
        )
        .unwrap()
        .wait();
    assert_eq!(record.state, RunState::ResourceLimited);
    assert_ne!(
        record.error.unwrap().storage.unwrap().position.phase,
        RunPhase::Capture
    );
    assert!(revisions(&project).unwrap().is_empty());
    assert_eq!(app.temporary_storage_status().reserved_bytes, 0);
}
