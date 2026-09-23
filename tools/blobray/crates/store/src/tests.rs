use super::*;

fn revision(project: &Project) -> Revision {
    Revision {
        schema: 1,
        project: project.id().clone(),
        parent: None,
        target: Target::Riscv32Ilp32,
        inventory_producer: "test/1".into(),
        inputs: vec![InputRecord {
            role: "vendor".into(),
            origin: OriginPath::UnixBytes {
                bytes: b"/missing".to_vec(),
            },
            expected: None,
            capture: Capture::Unavailable {
                diagnostic: Diagnostic {
                    code: DiagnosticCode::UnavailableInput,
                    context: "input".into(),
                    message: "fixture".into(),
                },
            },
            external_members: Vec::new(),
            inventory: None,
        }],
    }
}

#[test]
fn publication_failure_rolls_back_both_revision_and_current() {
    let temp = tempfile::tempdir().unwrap();
    let project = Project::create(temp.path()).unwrap();
    let connection = open_connection(&project.root, true).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_update BEFORE UPDATE ON project BEGIN SELECT RAISE(ABORT, 'injected publication failure'); END;").unwrap();
    let candidate = revision(&project);
    assert_eq!(
        project
            .writer()
            .unwrap()
            .commit(candidate.clone())
            .unwrap_err()
            .code,
        ErrorCode::Storage
    );
    let reopened = Project::open(temp.path()).unwrap();
    assert_eq!(reopened.current().unwrap(), None);
    assert!(reopened.revisions().unwrap().is_empty());
    connection
        .execute_batch("DROP TRIGGER fail_update")
        .unwrap();
    let published = reopened
        .writer()
        .unwrap()
        .commit(candidate.clone())
        .unwrap();
    assert_eq!(
        project
            .writer()
            .unwrap()
            .commit(candidate)
            .unwrap_err()
            .code,
        ErrorCode::Busy
    );
    assert_eq!(reopened.snapshot(None).unwrap(), published);
    assert_eq!(project.revisions().unwrap(), vec![published.revision_id]);
}

#[test]
fn incompatible_database_is_rejected_without_migration() {
    let temp = tempfile::tempdir().unwrap();
    let project = Project::create(temp.path()).unwrap();
    open_connection(&project.root, true)
        .unwrap()
        .pragma_update(None, "user_version", 99)
        .unwrap();
    assert!(matches!(
        Project::open(temp.path()),
        Err(Error {
            code: ErrorCode::Incompatible,
            ..
        })
    ));
    let connection = Connection::open(project.root.join("project.sqlite3")).unwrap();
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap(),
        99
    );
}

#[test]
fn manifest_ownership_is_checked_before_publication() {
    let temp = tempfile::tempdir().unwrap();
    let project = Project::create(temp.path()).unwrap();
    let mut candidate = revision(&project);
    candidate.project = "0".repeat(64).parse().unwrap();
    assert_eq!(
        project
            .writer()
            .unwrap()
            .commit(candidate)
            .unwrap_err()
            .code,
        ErrorCode::Integrity
    );
    assert!(project.revisions().unwrap().is_empty());
}

#[test]
fn interrupted_transaction_child() {
    let Some(path) = std::env::var_os("BLOBRAY_TEST_CRASH_PROJECT") else {
        return;
    };
    let writer = Writer::open(Path::new(&path)).unwrap();
    let connection = open_connection(&writer.project.root, true).unwrap();
    connection
        .execute_batch(
            "PRAGMA cache_size=1;
        CREATE TABLE crash_test (payload BLOB);
        BEGIN IMMEDIATE;
        UPDATE project SET current_revision='uncommitted';
        INSERT INTO crash_test VALUES (zeroblob(4194304));",
        )
        .unwrap();
    // Exit without Rust destructors or SQLite rollback, as in abrupt process loss.
    std::process::exit(0);
}

#[test]
fn explicit_writer_recovers_interrupted_metadata_transaction() {
    let temp = tempfile::tempdir().unwrap();
    let project = Project::create(temp.path()).unwrap();
    let before = project
        .writer()
        .unwrap()
        .commit(revision(&project))
        .unwrap();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "tests::interrupted_transaction_child"])
        .env("BLOBRAY_TEST_CRASH_PROJECT", temp.path())
        .status()
        .unwrap();
    assert!(status.success());
    let writer = Writer::open(temp.path()).unwrap();
    assert_eq!(writer.project().snapshot(None).unwrap(), before);
    assert_eq!(
        writer.project().revisions().unwrap(),
        vec![before.revision_id]
    );
}

fn owner() -> OwnerIdentity {
    OwnerIdentity {
        pid: 123,
        start_ticks: 42,
        boot_id: "test".into(),
    }
}

#[test]
fn upgrade_preserves_schema_one_manifests_and_is_explicit_and_idempotent() {
    let temp = tempfile::tempdir().unwrap();
    let project = Project::create(temp.path()).unwrap();
    let before = project
        .writer()
        .unwrap()
        .commit(revision(&project))
        .unwrap();
    let connection = open_connection(&project.root, true).unwrap();
    connection
        .execute_batch(
            "DROP TABLE legacy_imports; DROP TABLE knowledge_revisions; DROP TABLE publications; DROP TABLE current_publication; DROP TABLE analyses; DROP TABLE images; DROP TABLE runs; PRAGMA user_version=1;",
        )
        .unwrap();
    assert_eq!(
        Project::open(temp.path()).unwrap().snapshot(None).unwrap(),
        before
    );
    assert!(matches!(
        Writer::open(temp.path()),
        Err(Error {
            code: ErrorCode::Incompatible,
            ..
        })
    ));
    Project::upgrade(temp.path()).unwrap();
    Project::upgrade(temp.path()).unwrap();
    let after = Project::open(temp.path()).unwrap();
    assert_eq!(after.snapshot(None).unwrap(), before);
    assert!(after.runs().unwrap().is_empty());
}

#[test]
fn recovery_requires_dead_owner_and_free_stage_lease() {
    let temp = tempfile::tempdir().unwrap();
    let project = Project::create(temp.path()).unwrap();
    let mut writer = project.writer().unwrap();
    let (run, stage) = writer.register(ResourceBudget::default(), owner()).unwrap();
    assert_eq!(
        writer.recover(&|_| Ok(true), &|_| Ok(())).unwrap_err().code,
        ErrorCode::Busy
    );
    let lease = OpenOptions::new()
        .read(true)
        .write(true)
        .open(stage.join("lease.lock"))
        .unwrap();
    lease.lock_shared().unwrap();
    assert_eq!(
        writer
            .recover(&|_| Ok(false), &|_| Ok(()))
            .unwrap_err()
            .code,
        ErrorCode::Busy
    );
    assert!(stage.exists());
    drop(lease);
    let recovered = writer.recover(&|_| Ok(false), &|_| Ok(())).unwrap();
    assert_eq!(recovered[0].id, run.id);
    assert_eq!(recovered[0].state, RunState::Abandoned);
    assert!(!stage.exists());
    assert!(
        writer
            .recover(&|_| Ok(false), &|_| Ok(()))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn run_success_and_revision_publication_are_one_transaction() {
    let temp = tempfile::tempdir().unwrap();
    let project = Project::create(temp.path()).unwrap();
    let mut writer = project.writer().unwrap();
    let (mut run, stage) = writer.register(ResourceBudget::default(), owner()).unwrap();
    let prepared = Staging::open(&stage)
        .unwrap()
        .prepare(revision(&project))
        .unwrap();
    let retained = writer
        .retain_candidate(&run, &prepared, &mut || Ok(()))
        .unwrap();
    let connection = open_connection(&project.root, true).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_run BEFORE UPDATE ON runs BEGIN SELECT RAISE(ABORT,'injected run write failure'); END;").unwrap();
    assert!(writer.publish_run(&mut run, retained).is_err());
    assert_eq!(project.current().unwrap(), None);
    assert!(project.revisions().unwrap().is_empty());
    assert_eq!(project.runs().unwrap()[0].state, RunState::Registered);
    connection.execute_batch("DROP TRIGGER fail_run").unwrap();
    let retained = writer
        .retain_candidate(&run, &prepared, &mut || Ok(()))
        .unwrap();
    writer.publish_run(&mut run, retained).unwrap();
    drop(writer); // Simulate lost delivery after a successful commit.
    let reopened = Project::open(temp.path()).unwrap();
    assert_eq!(reopened.current().unwrap(), run.revision);
    assert_eq!(reopened.runs().unwrap()[0].state, RunState::Completed);
}

#[test]
fn cancellation_during_retention_and_corrupt_candidate_never_publish() {
    let temp = tempfile::tempdir().unwrap();
    let project = Project::create(temp.path()).unwrap();
    let mut writer = project.writer().unwrap();
    let (run, stage) = writer.register(ResourceBudget::default(), owner()).unwrap();
    let prepared = Staging::open(&stage)
        .unwrap()
        .prepare(revision(&project))
        .unwrap();
    assert!(matches!(
        writer.retain_candidate(&run, &prepared, &mut || Err(Error::new(
            ErrorCode::Cancelled,
            "cancel retention"
        ))),
        Err(Error {
            code: ErrorCode::Cancelled,
            ..
        })
    ));
    fs::write(
        stage.join("objects").join(prepared.revision.as_str()),
        b"corrupt manifest",
    )
    .unwrap();
    assert!(matches!(
        writer.retain_candidate(&run, &prepared, &mut || Ok(())),
        Err(Error {
            code: ErrorCode::Integrity,
            ..
        })
    ));
    assert!(project.current().unwrap().is_none());
}

#[test]
fn ordinary_reader_overlap_does_not_fail_writer_progress() {
    let temp = tempfile::tempdir().unwrap();
    let project = Project::create(temp.path()).unwrap();
    let mut writer = project.writer().unwrap();
    let (mut run, _) = writer.register(ResourceBudget::default(), owner()).unwrap();
    let root = project.root.clone();
    let (ready, wait) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let connection = open_connection(&root, false).unwrap();
        connection
            .execute_batch("BEGIN; SELECT * FROM runs;")
            .unwrap();
        ready.send(()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        connection.execute_batch("COMMIT").unwrap();
    });
    wait.recv().unwrap();
    run.state = RunState::Running;
    writer.update_run(&run).unwrap();
    reader.join().unwrap();
    assert_eq!(project.run(&run.id).unwrap().state, RunState::Running);
}

#[test]
fn ordinary_update_cannot_fabricate_completion() {
    let temp = tempfile::tempdir().unwrap();
    let project = Project::create(temp.path()).unwrap();
    let mut writer = project.writer().unwrap();
    let (mut run, _) = writer.register(ResourceBudget::default(), owner()).unwrap();
    run.state = RunState::Completed;
    assert_eq!(
        writer.update_run(&run).unwrap_err().code,
        ErrorCode::Integrity
    );
    assert_eq!(project.run(&run.id).unwrap().state, RunState::Registered);
}

#[test]
fn old_run_unknown_metrics_and_future_versions_use_one_decoder() {
    let temp = tempfile::tempdir().unwrap();
    let project = Project::create(temp.path()).unwrap();
    let mut writer = project.writer().unwrap();
    let (run, _) = writer.register(ResourceBudget::default(), owner()).unwrap();
    let mut value = serde_json::to_value(&run).unwrap();
    value["schema"] = 1.into();
    value["budget"]
        .as_object_mut()
        .unwrap()
        .remove("working_memory_bytes");
    value.as_object_mut().unwrap().remove("diagnostics");
    value["budget"]
        .as_object_mut()
        .unwrap()
        .remove("max_work_units");
    value["budget"]
        .as_object_mut()
        .unwrap()
        .remove("work_policy");
    let connection = open_connection(&project.root, true).unwrap();
    connection
        .execute("UPDATE runs SET record=?1", [value.to_string()])
        .unwrap();
    let old = project.run(&run.id).unwrap();
    assert_eq!(old, project.runs().unwrap()[0]);
    assert!(old.budget.max_work_units.is_none());
    assert!(old.budget.work_policy.is_none());
    assert!(old.budget.working_memory_bytes.is_none());
    assert!(old.diagnostics.is_none());
    value["schema"] = 999.into();
    connection
        .execute("UPDATE runs SET record=?1", [value.to_string()])
        .unwrap();
    assert_eq!(
        project.run(&run.id).unwrap_err().code,
        ErrorCode::Incompatible
    );
    assert_eq!(project.runs().unwrap_err().code, ErrorCode::Incompatible);
    assert_eq!(
        writer
            .recover(&|_| Ok(false), &|_| Ok(()))
            .unwrap_err()
            .code,
        ErrorCode::Incompatible
    );
}

#[test]
fn recovery_persists_checkpoint_before_removing_staging() {
    let temp = tempfile::tempdir().unwrap();
    let project = Project::create(temp.path()).unwrap();
    let mut writer = project.writer().unwrap();
    let (run, stage) = writer.register(ResourceBudget::default(), owner()).unwrap();
    let progress = RunProgress {
        sequence: 23,
        work_used: 999,
        position: RunPosition {
            phase: RunPhase::Elf,
            input: Some(2),
            member: Some(4),
            entry: Some(15),
            ..Default::default()
        },
        ..Default::default()
    };
    fs::write(
        stage.join("progress.json"),
        serde_json::to_vec(&ProgressRecord {
            schema: 1,
            run: run.id.clone(),
            progress,
        })
        .unwrap(),
    )
    .unwrap();
    let result = writer.recover(&|_| Ok(false), &|_| Ok(())).unwrap();
    assert_eq!(
        result[0].diagnostics.as_ref().unwrap().progress,
        Some(progress)
    );
    assert_eq!(
        project.run(&run.id).unwrap().diagnostics.unwrap().progress,
        Some(progress)
    );
    assert!(!stage.exists());
}

#[test]
fn range_reader_preserves_reordered_schema_one_fields_and_rejects_duplicate_keys() {
    let temp = tempfile::tempdir().unwrap();
    let project = Project::create(temp.path()).unwrap();
    let candidate = revision(&project);
    // Value uses a different field ordering than the original struct serializer.
    let value = serde_json::to_value(&candidate).unwrap();
    let bytes = serde_json::to_vec_pretty(&value).unwrap();
    let id = RevisionId::of_manifest(&bytes);
    fs::write(project.object_path(&id.as_str().parse().unwrap()), &bytes).unwrap();
    let connection = open_connection(&project.root, true).unwrap();
    connection
        .execute("INSERT INTO revisions(id) VALUES (?1)", [id.as_str()])
        .unwrap();
    connection
        .execute("UPDATE project SET current_revision=?1", [id.as_str()])
        .unwrap();
    let memory = WorkingMemory::new(128 * 1024).unwrap();
    let view = project
        .read_inventory(None, &memory, &mut || Ok(()), &mut ())
        .unwrap();
    assert_eq!(view.revision_id, id);
    assert_eq!(memory.used(), 0);
    assert_eq!(project.snapshot(None).unwrap().revision, candidate);
    let text = String::from_utf8(bytes).unwrap();
    let duplicate = text.replacen('{', "{\"schema\":1,", 1);
    let bad_id = RevisionId::of_manifest(duplicate.as_bytes());
    fs::write(
        project.object_path(&bad_id.as_str().parse().unwrap()),
        duplicate,
    )
    .unwrap();
    connection
        .execute("INSERT INTO revisions(id) VALUES (?1)", [bad_id.as_str()])
        .unwrap();
    assert!(matches!(
        project.read_inventory(Some(&bad_id), &memory, &mut || Ok(()), &mut ()),
        Err(Error {
            code: ErrorCode::Integrity,
            ..
        })
    ));
    assert_eq!(memory.used(), 0);
    // Explicit historical selection remains readable with an invalid current pointer.
    connection
        .execute("UPDATE project SET current_revision='broken'", [])
        .unwrap();
    assert!(
        project
            .read_inventory(Some(&id), &memory, &mut || Ok(()), &mut ())
            .is_ok()
    );
}

#[test]
fn doctor_admits_run_cell_before_loading_it_and_propagates_resource_failure() {
    let temp = tempfile::tempdir().unwrap();
    let project = Project::create(temp.path()).unwrap();
    let connection = open_connection(&project.root, true).unwrap();
    connection
        .execute(
            "INSERT INTO runs(id,record) VALUES (?1,zeroblob(4194304))",
            ["a".repeat(64)],
        )
        .unwrap();
    struct Sink;
    impl DoctorSink for Sink {
        fn error(&mut self, _: &Error, _: &mut dyn RunControl) -> Result<()> {
            panic!("resource failure cannot become a doctor finding");
        }
        fn unfinished(&mut self, _: &RunId, _: &mut dyn RunControl) -> Result<()> {
            panic!("invalid fixture cannot be an unfinished run");
        }
    }
    let memory = WorkingMemory::new(1024 * 1024).unwrap();
    let error = match project.doctor_stream(&memory, &mut || Ok(()), &mut Sink) {
        Err(error) => error,
        _ => panic!("expected admission failure"),
    };
    assert_eq!(error.code, ErrorCode::ResourceLimited);
    assert!(error.memory.unwrap().requested_bytes > 1024 * 1024);
    assert_eq!(memory.used(), 0);
}

#[test]
fn missing_captured_payload_is_integrity_failure_even_if_origin_survives() {
    let temp = tempfile::tempdir().unwrap();
    let project = Project::create(&temp.path().join("project")).unwrap();
    let source = temp.path().join("source");
    fs::write(&source, b"original captured bytes").unwrap();
    let captured = project.writer().unwrap().capture(&source, None).unwrap();
    let id = captured.artifact().unwrap();
    fs::remove_file(project.object_path(id)).unwrap();
    assert!(matches!(
        project.open_payload(id, &mut || Ok(())),
        Err(Error {
            code: ErrorCode::Integrity,
            ..
        })
    ));
    assert!(source.exists());
    assert!(!project.object_path(id).exists());
}

#[test]
fn registration_assigns_stage_ownership_before_setup_and_keeps_primary_failure() {
    let temp = tempfile::tempdir().unwrap();
    let project = Project::create(temp.path()).unwrap();
    let mut writer = project.writer().unwrap();
    let mut owned = None;
    let error = writer
        .register_with_stage_owner(ResourceBudget::default(), owner(), |path| {
            assert!(!path.exists());
            owned = Some(path.to_owned());
            // Inject an unexpected file between ownership assignment and directory setup.
            fs::write(path, b"injected setup obstruction").unwrap();
        })
        .unwrap_err();
    let path = owned.unwrap();
    assert!(path.exists());
    let record = project.runs().unwrap().pop().unwrap();
    assert_eq!(record.state, RunState::Failed);
    assert_eq!(record.error.as_ref(), Some(&error));
    assert!(!record.diagnostics.unwrap().secondary.is_empty());
}

#[test]
fn image_and_completed_run_publish_atomically_without_advancing_current() {
    use std::io::Write;
    let temp = tempfile::tempdir().unwrap();
    let project = Project::create(temp.path()).unwrap();
    let snapshot = project
        .writer()
        .unwrap()
        .commit(revision(&project))
        .unwrap();
    let payload = ArtifactId::of_bytes(b"validated worker output");
    let selected = EntrySelection {
        input: 0,
        symbol: SymbolId {
            object: ObjectId {
                artifact: payload.clone(),
                location: ObjectLocation::Standalone,
            },
            table: SymbolTableKind::Static,
            table_section: 1,
            index: 1,
        },
    };
    let recipe = LinkRecipe {
        companions: Vec::new(),
        schema: 1,
        policy: 4,
        project: project.id().clone(),
        revision: snapshot.revision_id.clone(),
        inputs: vec![0],
        entry: selected.clone(),
        roots: vec![],
        layout: ImageLayout {
            code: ImageRegion {
                start: 0x1000,
                length: 4096,
            },
            data: ImageRegion {
                start: 0x2000,
                length: 4096,
            },
        },
        linker: LinkerIdentity {
            implementation: "lld-elf-22".into(),
            version: "LLD 22.1.8".into(),
            executable: payload.clone(),
        },
    };
    let plan = LinkPlanDescription {
        id: ArtifactId::of_bytes(&serde_json::to_vec(&recipe).unwrap())
            .as_str()
            .parse()
            .unwrap(),
        recipe,
        blockers: vec![],
    };
    let mut writer = project.writer().unwrap();
    let (mut run, stage) = writer
        .register_operation(
            ResourceBudget::default(),
            owner(),
            RunOperation::PrepareImage {
                revision: snapshot.revision_id.clone(),
                plan: plan.id.clone(),
            },
            |_| {},
        )
        .unwrap();
    let manifest = ImageManifest {
        abi: RiscvAbi::Ilp32,
        schema: 2,
        synthetic: true,
        plan,
        elf: payload.clone(),
        map: payload.clone(),
        extraction: payload.clone(),
        provenance: payload.clone(),
        entry: 0x1000,
        roots: vec![ResolvedRoot {
            selection: selected,
            address: 0x1000,
            size: 4,
            name: b"entry".to_vec(),
        }],
        segments: vec![],
        linker_diagnostics: LinkerDiagnostics {
            exit_code: Some(0),
            signal: None,
            stderr_tail: vec![],
            stderr_truncated: false,
        },
    };
    let staging = Staging::open(&stage).unwrap();
    let disk = TemporaryBudget::open(&stage).unwrap();
    let mut file = disk.temporary(&stage.join("staging")).unwrap();
    file.write_all(b"validated worker output").unwrap();
    staging.retain_temporary(file, &mut || Ok(())).unwrap();
    let mut file = disk.temporary(&stage.join("staging")).unwrap();
    file.write_all(&serde_json::to_vec(&manifest).unwrap())
        .unwrap();
    let receipt = staging.image_receipt(file, &mut || Ok(())).unwrap();
    let retained = writer.retain_image(&run, &receipt, &mut || Ok(())).unwrap();
    let connection = open_connection(&project.root, true).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_image_run BEFORE UPDATE ON runs BEGIN SELECT RAISE(ABORT,'injected failure'); END;").unwrap();
    assert!(writer.publish_image(&mut run, retained).is_err());
    let mut count = 0;
    project
        .images(&mut || Ok(()), &mut |_, _| {
            count += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(count, 0);
    assert_eq!(project.run(&run.id).unwrap().state, RunState::Registered);
    assert_eq!(
        project.current().unwrap(),
        Some(snapshot.revision_id.clone())
    );
    connection
        .execute_batch("DROP TRIGGER fail_image_run")
        .unwrap();
    let retained = writer.retain_image(&run, &receipt, &mut || Ok(())).unwrap();
    writer.publish_image(&mut run, retained).unwrap();
    drop(writer);
    let reopened = Project::open(temp.path()).unwrap();
    assert_eq!(reopened.current().unwrap(), Some(snapshot.revision_id));
    let record = reopened.run(&run.id).unwrap();
    assert_eq!(record.state, RunState::Completed);
    assert_eq!(record.image, Some(receipt.image.clone()));
    assert_eq!(
        reopened
            .image(&receipt.image, &mut || Ok(()))
            .unwrap()
            .manifest,
        manifest
    );
}

#[test]
fn schema_two_upgrade_adds_images_without_rewriting_revision_or_runs() {
    let temp = tempfile::tempdir().unwrap();
    let project = Project::create(temp.path()).unwrap();
    let snapshot = project
        .writer()
        .unwrap()
        .commit(revision(&project))
        .unwrap();
    let bytes =
        fs::read(project.object_path(&snapshot.revision_id.as_str().parse().unwrap())).unwrap();
    let connection = open_connection(&project.root, true).unwrap();
    connection
        .execute_batch("DROP TABLE legacy_imports; DROP TABLE knowledge_revisions; DROP TABLE publications; DROP TABLE current_publication; DROP TABLE analyses; DROP TABLE images; PRAGMA user_version=2;")
        .unwrap();
    drop(connection);
    let old = Project::open(temp.path()).unwrap();
    let mut count = 0;
    old.images(&mut || Ok(()), &mut |_, _| {
        count += 1;
        Ok(())
    })
    .unwrap();
    assert_eq!(count, 0);
    assert!(old.writer().is_err());
    Project::upgrade(temp.path()).unwrap();
    Project::upgrade(temp.path()).unwrap();
    let reopened = Project::open(temp.path()).unwrap();
    assert_eq!(reopened.snapshot(None).unwrap(), snapshot);
    assert_eq!(
        fs::read(reopened.object_path(&snapshot.revision_id.as_str().parse().unwrap())).unwrap(),
        bytes
    );
    assert!(reopened.runs().unwrap().is_empty());
}

#[test]
fn function_publication_is_atomic_and_preserves_semantic_incompleteness() {
    for decoding in [false, true] {
        check_function_publication(decoding);
    }
}
fn check_function_publication(decoding: bool) {
    use std::io::Write;
    let temp = tempfile::tempdir().unwrap();
    let project = Project::create(temp.path()).unwrap();
    let snapshot = project
        .writer()
        .unwrap()
        .commit(revision(&project))
        .unwrap();
    let symbol = SymbolId {
        object: ObjectId {
            artifact: ArtifactId::of_bytes(b"captured"),
            location: ObjectLocation::Standalone,
        },
        table: SymbolTableKind::Static,
        table_section: 1,
        index: 1,
    };
    let request = FunctionRequest {
        research: None,
        revision: Some(snapshot.revision_id.clone()),
        source: FunctionSource::Input { input: 0 },
        symbol: symbol.clone(),
        extent: Some(CodeRange {
            start: 0,
            length: 4,
        }),
    };
    let mut writer = project.writer().unwrap();
    let (mut run, path) = writer
        .register_operation(
            ResourceBudget::default(),
            owner(),
            RunOperation::AnalyzeFunction {
                request: request.clone(),
            },
            |_| {},
        )
        .unwrap();
    let stage = Staging::open(&path).unwrap();
    let mut file = stage.disk.temporary(&path.join("staging")).unwrap();
    file.write_all(b"retained typed records").unwrap();
    let records = stage.retain_temporary(file, &mut || Ok(())).unwrap();
    let manifest = FunctionManifest {
        schema: 4,
        semantics: Some(SemanticSummary {
            complete: false,
            gaps: 1,
            ..Default::default()
        }),
        recipe: FunctionRecipe {
            research: None,
            abi: RiscvAbi::Ilp32,
            address_space: CodeAddressSpace::Section,
            schema: 4,
            policy: 4,
            semantics: Some("fixture-values/1".into()),
            decoder: "fixture/1".into(),
            project: project.id().clone(),
            revision: snapshot.revision_id.clone(),
            source: FunctionSource::Input { input: 0 },
            symbol,
            payload: ArtifactId::of_bytes(b"payload"),
            section: 2,
            extent: request.extent.unwrap(),
            user_extent: true,
        },
        records,
        coverage: FunctionCoverage {
            decoding,
            control_flow: decoding,
            references: true,
        },
        instructions: 0,
        blocks: 0,
        edges: 0,
        references: 0,
        gaps: 1,
    };
    let receipt = stage.function_receipt(&manifest, &mut || Ok(())).unwrap();
    let retained = writer
        .retain_function(&run, &receipt, &mut || Ok(()))
        .unwrap();
    let connection = open_connection(&project.root, true).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_function BEFORE UPDATE ON runs BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    assert!(writer.publish_function(&mut run, retained).is_err());
    let mut count = 0;
    project
        .analyses(&mut || Ok(()), &mut |_, _| {
            count += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(count, 0);
    assert_eq!(project.run(&run.id).unwrap().state, RunState::Registered);
    connection
        .execute_batch("DROP TRIGGER fail_function")
        .unwrap();
    let retained = writer
        .retain_function(&run, &receipt, &mut || Ok(()))
        .unwrap();
    writer.publish_function(&mut run, retained).unwrap();
    drop(writer);
    let reopened = Project::open(temp.path()).unwrap();
    assert_eq!(reopened.current().unwrap(), Some(snapshot.revision_id));
    let record = reopened.run(&run.id).unwrap();
    assert_eq!(record.state, RunState::Completed);
    assert_eq!(record.complete, Some(false));
    assert_eq!(
        reopened
            .analysis(record.analysis.as_ref().unwrap(), &mut || Ok(()))
            .unwrap()
            .manifest,
        manifest
    );
}
#[test]
fn schema_three_upgrade_keeps_image_metadata_and_all_existing_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let project = Project::create(temp.path()).unwrap();
    let snapshot = project
        .writer()
        .unwrap()
        .commit(revision(&project))
        .unwrap();
    let connection = open_connection(&project.root, true).unwrap();
    connection
        .execute_batch("DROP TABLE legacy_imports; DROP TABLE knowledge_revisions; DROP TABLE publications; DROP TABLE current_publication; DROP TABLE analyses; PRAGMA user_version=3;")
        .unwrap();
    drop(connection);
    assert!(Project::open(temp.path()).unwrap().writer().is_err());
    Project::upgrade(temp.path()).unwrap();
    Project::upgrade(temp.path()).unwrap();
    let reopened = Project::open(temp.path()).unwrap();
    assert_eq!(reopened.snapshot(None).unwrap(), snapshot);
    let mut count = 0;
    reopened
        .images(&mut || Ok(()), &mut |_, _| {
            count += 1;
            Ok(())
        })
        .unwrap();
    reopened
        .analyses(&mut || Ok(()), &mut |_, _| {
            count += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(count, 0);
}

fn staged_investigation(
    writer: &mut Writer,
    revision: &RevisionId,
) -> (
    RunRecord,
    InvestigationPlan,
    PreparedInvestigationReceipt,
    FunctionAnalysisId,
) {
    use std::io::Write;
    let object = ObjectId {
        artifact: ArtifactId::of_bytes(b"object"),
        location: ObjectLocation::Standalone,
    };
    let symbol = SymbolId {
        object,
        table: SymbolTableKind::Static,
        table_section: 2,
        index: 1,
    };
    let request = FunctionRequest {
        research: None,
        revision: Some(revision.clone()),
        source: FunctionSource::Input { input: 0 },
        symbol: symbol.clone(),
        extent: None,
    };
    let entry = PlanEntry::Function {
        address_space: CodeAddressSpace::Section,
        declared_extent: CodeRange {
            start: 0,
            length: 4,
        },
        request,
        name: Some(b"entry".to_vec()),
        payload: ArtifactId::of_bytes(b"object"),
    };
    let mut digest = EntryDigest::default();
    digest.include(&entry).unwrap();
    let plan = investigation_plan(InvestigationRecipe {
        schema: 2,
        policy: 2,
        project: writer.project.id.clone(),
        request: InvestigationRequest {
            revision: Some(revision.clone()),
            ..Default::default()
        },
        producer: FunctionProducer {
            decoder: "test/1".into(),
            semantics: "values/1".into(),
        },
        entries: digest.finish().unwrap(),
        entry_count: 1,
        functions: 1,
    })
    .unwrap();
    let (mut run, path) = writer
        .register_operation(
            ResourceBudget::default(),
            owner(),
            RunOperation::Investigate {
                revision: revision.clone(),
                plan: plan.id.clone(),
            },
            |_| {},
        )
        .unwrap();
    run.state = RunState::Running;
    writer.update_run(&run).unwrap();
    run.state = RunState::Validating;
    writer.update_run(&run).unwrap();
    let disk = TemporaryBudget::open(&path).unwrap();
    let stage = Staging::with_temporary_budget(&path, disk.clone()).unwrap();
    let records = stage
        .retain_temporary(disk.temporary(&path.join("staging")).unwrap(), &mut || {
            Ok(())
        })
        .unwrap();
    let child = stage
        .function_receipt(
            &FunctionManifest {
                schema: 4,
                recipe: FunctionRecipe {
                    research: None,
                    abi: RiscvAbi::Ilp32,
                    address_space: CodeAddressSpace::Section,
                    schema: 4,
                    policy: 4,
                    project: writer.project.id.clone(),
                    revision: revision.clone(),
                    source: FunctionSource::Input { input: 0 },
                    symbol,
                    payload: ArtifactId::of_bytes(b"object"),
                    section: 1,
                    extent: CodeRange {
                        start: 0,
                        length: 2,
                    },
                    user_extent: false,
                    decoder: "test/1".into(),
                    semantics: Some("values/1".into()),
                },
                records,
                coverage: FunctionCoverage::default(),
                instructions: 1,
                blocks: 1,
                edges: 1,
                references: 0,
                gaps: 0,
                semantics: Some(SemanticSummary {
                    complete: true,
                    ..Default::default()
                }),
            },
            &mut || Ok(()),
        )
        .unwrap();
    let member = InvestigationMember {
        entry,
        outcome: InvestigationOutcome::Analyzed {
            analysis: child.analysis.clone(),
            complete: true,
        },
    };
    let mut file = disk.temporary(&path.join("staging")).unwrap();
    serde_json::to_writer(&mut file, &member).unwrap();
    file.write_all(b"\n").unwrap();
    let members = stage.retain_temporary(file, &mut || Ok(())).unwrap();
    let mut coverage = InvestigationCoverage::default();
    coverage.include(&member);
    let receipt = stage
        .investigation_receipt(
            &InvestigationManifest {
                schema: 1,
                plan: plan.clone(),
                members,
                coverage,
            },
            &mut || Ok(()),
        )
        .unwrap();
    (run, plan, receipt, child.analysis)
}
#[test]
fn investigation_commit_failure_and_work_exhaustion_rollback_children_pointer_and_run() {
    let temp = tempfile::tempdir().unwrap();
    let project = Project::create(temp.path()).unwrap();
    let revision = project
        .writer()
        .unwrap()
        .commit(revision(&project))
        .unwrap()
        .revision_id;
    let mut writer = project.writer().unwrap();
    let (mut run, plan, receipt, child) = staged_investigation(&mut writer, &revision);
    let connection = open_connection(&project.root, true).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_publication BEFORE UPDATE ON runs WHEN json_extract(NEW.record,'$.state')='completed' BEGIN SELECT RAISE(ABORT,'injected commit failure'); END;").unwrap();
    let retained = writer
        .retain_investigation(&run, &receipt, &plan, &mut || Ok(()))
        .unwrap();
    assert_eq!(
        writer
            .publish_investigation(&mut run, retained, &mut || Ok(()))
            .unwrap_err()
            .code,
        ErrorCode::Storage
    );
    assert!(project.analysis(&child, &mut || Ok(())).is_err());
    assert!(
        project
            .publication(&receipt.publication, &mut || Ok(()))
            .is_err()
    );
    assert_eq!(project.run(&run.id).unwrap().state, RunState::Validating);
    assert!(
        project
            .investigation_status(&mut || Ok(()))
            .unwrap()
            .publication
            .is_none()
    );
    connection
        .execute_batch("DROP TRIGGER fail_publication")
        .unwrap();
    // The final zero-unit checkpoint occurs after inserts and pointer update.
    struct FailCommit;
    impl RunControl for FailCommit {
        fn checkpoint(&mut self, units: u64) -> Result<()> {
            if units == 0 {
                Err(Error::new(
                    ErrorCode::ResourceLimited,
                    "injected exhausted budget",
                ))
            } else {
                Ok(())
            }
        }
    }
    let retained = writer
        .retain_investigation(&run, &receipt, &plan, &mut || Ok(()))
        .unwrap();
    assert_eq!(
        writer
            .publish_investigation(&mut run, retained, &mut FailCommit)
            .unwrap_err()
            .code,
        ErrorCode::ResourceLimited
    );
    assert!(project.analysis(&child, &mut || Ok(())).is_err());
    assert!(
        project
            .investigation_status(&mut || Ok(()))
            .unwrap()
            .publication
            .is_none()
    );
    let retained = writer
        .retain_investigation(&run, &receipt, &plan, &mut || Ok(()))
        .unwrap();
    writer
        .publish_investigation(&mut run, retained, &mut || Ok(()))
        .unwrap();
    assert_eq!(
        project.run(&run.id).unwrap().publication,
        Some(receipt.publication.clone())
    );
    assert!(
        project
            .publication(&receipt.publication, &mut || Ok(()))
            .is_ok()
    );
    assert!(project.analysis(&child, &mut || Ok(())).is_ok());
    writer.cleanup_stage(&run.id).unwrap();
    drop(writer);
}
#[test]
fn schema_four_upgrade_preserves_function_and_journal_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let project = Project::create(temp.path()).unwrap();
    let revision = project
        .writer()
        .unwrap()
        .commit(revision(&project))
        .unwrap();
    let mut writer = project.writer().unwrap();
    let (mut run, plan, receipt, child) = staged_investigation(&mut writer, &revision.revision_id);
    let retained = writer
        .retain_investigation(&run, &receipt, &plan, &mut || Ok(()))
        .unwrap();
    writer
        .publish_investigation(&mut run, retained, &mut || Ok(()))
        .unwrap();
    writer.cleanup_stage(&run.id).unwrap();
    drop(writer);
    let manifest = project.analysis(&child, &mut || Ok(())).unwrap().manifest;
    // Encode a valid schema-4 function run and remove only v5 metadata.
    run.schema = 4;
    run.publication = None;
    run.analysis = Some(child.clone());
    run.operation = RunOperation::AnalyzeFunction {
        request: FunctionRequest {
            research: None,
            revision: Some(revision.revision_id.clone()),
            source: FunctionSource::Input { input: 0 },
            symbol: manifest.recipe.symbol.clone(),
            extent: None,
        },
    };
    let raw = serde_json::to_string(&run).unwrap();
    let conn = open_connection(&project.root, true).unwrap();
    conn.execute(
        "UPDATE runs SET record=?2 WHERE id=?1",
        params![run.id.as_str(), &raw],
    )
    .unwrap();
    conn.execute_batch(
        "DROP TABLE legacy_imports; DROP TABLE knowledge_revisions; DROP TABLE publications; DROP TABLE current_publication; PRAGMA user_version=4;",
    )
    .unwrap();
    assert!(Project::open(temp.path()).unwrap().writer().is_err());
    assert_eq!(project.run(&run.id).unwrap(), run);
    assert!(
        project
            .investigation_status(&mut || Ok(()))
            .unwrap()
            .publication
            .is_none()
    );
    Project::upgrade(temp.path()).unwrap();
    Project::upgrade(temp.path()).unwrap();
    assert_eq!(project.snapshot(None).unwrap(), revision);
    assert_eq!(
        project.analysis(&child, &mut || Ok(())).unwrap().manifest,
        manifest
    );
    assert_eq!(
        conn.query_row(
            "SELECT record FROM runs WHERE id=?1",
            [run.id.as_str()],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        raw
    );
    assert_eq!(
        conn.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap(),
        6
    );
}

#[test]
fn investigation_retention_rejects_coverage_and_child_recipe_substitution() {
    let temp = tempfile::tempdir().unwrap();
    let project = Project::create(temp.path()).unwrap();
    let revision = project
        .writer()
        .unwrap()
        .commit(revision(&project))
        .unwrap()
        .revision_id;
    let mut writer = project.writer().unwrap();
    let (run, plan, receipt, _) = staged_investigation(&mut writer, &revision);
    let stage = Staging::open(&writer.stage_path(&run.id)).unwrap();
    let source = stage
        .open_payload(&receipt.publication.as_str().parse().unwrap(), &mut || {
            Ok(())
        })
        .unwrap();
    let mut bytes = vec![0; source.len() as usize];
    source.read_at(0, &mut bytes, &mut || Ok(())).unwrap();
    let mut manifest: InvestigationManifest = serde_json::from_slice(&bytes).unwrap();
    manifest.coverage.complete_functions = 0;
    let corrupt = stage
        .investigation_receipt(&manifest, &mut || Ok(()))
        .unwrap();
    assert!(matches!(
        writer.retain_investigation(&run, &corrupt, &plan, &mut || Ok(())),
        Err(Error {
            code: ErrorCode::Integrity,
            ..
        })
    ));
    // A manifest with a different producer cannot stand in for the admitted plan.
    manifest.plan.recipe.producer.semantics = "other/1".into();
    manifest.plan = investigation_plan(manifest.plan.recipe).unwrap();
    let corrupt = stage
        .investigation_receipt(&manifest, &mut || Ok(()))
        .unwrap();
    assert!(matches!(
        writer.retain_investigation(&run, &corrupt, &plan, &mut || Ok(())),
        Err(Error {
            code: ErrorCode::Integrity,
            ..
        })
    ));
}

#[test]
fn investigation_digest_rejects_encoded_expansion_without_large_buffer() {
    let mut digest = EntryDigest::default();
    let error = digest
        .include(&PlanEntry::Input {
            input: 0,
            role: "\0".repeat(16384),
        })
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert_eq!(digest.count, 0);
}

#[test]
fn knowledge_transaction_failure_and_cancellation_leave_head_and_journal_unchanged() {
    let temp = tempfile::tempdir().unwrap();
    let project = Project::create(temp.path()).unwrap();
    let mut writer = project.writer().unwrap();
    let payload = ArtifactId::of_bytes(b"source");
    let proposal = KnowledgeProposal {
        subject: "subject".to_owned().try_into().unwrap(),
        occurrence: KnowledgeOccurrence {
            revision: payload.as_str().parse().unwrap(),
            source: FunctionSource::Input { input: 0 },
            object: ObjectId {
                artifact: payload.clone(),
                location: ObjectLocation::Standalone,
            },
            symbol: None,
        },
        claim: KnowledgeClaim::Hypothesis {
            text: "possible".into(),
        },
        evidence: vec![EvidenceRef::Document { payload }],
        note: None,
    };
    let change = KnowledgeChange {
        expected_base: None,
        actor: "reviewer".into(),
        reason: "review".into(),
        action: KnowledgeAction::Propose { proposal },
    };
    let (mut run, path) = writer
        .register_operation(
            ResourceBudget::default(),
            owner(),
            RunOperation::Knowledge {
                change: change.clone(),
            },
            |_| {},
        )
        .unwrap();
    let stage = Staging::open(&path).unwrap();
    let receipt = stage
        .knowledge_receipt(
            &KnowledgeManifest {
                schema: 2,
                project: project.id().clone(),
                change,
                assertion: ArtifactId::of_bytes(b"candidate").as_str().parse().unwrap(),
                evidence_roots: vec![],
            },
            &mut || Ok(()),
        )
        .unwrap();
    run.state = RunState::Running;
    writer.update_run(&run).unwrap();
    run.state = RunState::Validating;
    writer.update_run(&run).unwrap();
    let retained = writer
        .retain_knowledge(&run, &receipt, &mut || Ok(()))
        .unwrap();
    assert_eq!(
        writer
            .publish_knowledge(&mut run, retained, &mut || Err(Error::new(
                ErrorCode::Cancelled,
                "stop"
            )))
            .unwrap_err()
            .code,
        ErrorCode::Cancelled
    );
    assert!(project.current_knowledge().unwrap().is_none());
    assert_eq!(project.run(&run.id).unwrap().state, RunState::Validating);
    let db = open_connection(&project.root, true).unwrap();
    db.execute_batch("CREATE TRIGGER fail_review BEFORE UPDATE ON runs BEGIN SELECT RAISE(ABORT,'injected failure'); END;").unwrap();
    let retained = writer
        .retain_knowledge(&run, &receipt, &mut || Ok(()))
        .unwrap();
    assert!(
        writer
            .publish_knowledge(&mut run, retained, &mut || Ok(()))
            .is_err()
    );
    assert!(project.current_knowledge().unwrap().is_none());
    assert_eq!(project.run(&run.id).unwrap().state, RunState::Validating);
    db.execute_batch("DROP TRIGGER fail_review;").unwrap();
    let retained = writer
        .retain_knowledge(&run, &receipt, &mut || Ok(()))
        .unwrap();
    writer
        .publish_knowledge(&mut run, retained, &mut || Ok(()))
        .unwrap();
    assert_eq!(project.current_knowledge().unwrap(), Some(receipt.revision));
}
#[test]
fn backup_captures_one_database_revision_while_later_publication_adds_objects() {
    let temp = tempfile::tempdir().unwrap();
    let project = Project::create(&temp.path().join("source")).unwrap();
    let before = project
        .writer()
        .unwrap()
        .commit(revision(&project))
        .unwrap();
    let mut next = revision(&project);
    next.parent = Some(before.revision_id.clone());
    let mut writer = Some(project.writer().unwrap());
    let mut calls = 0;
    let stage = temp.path().join("backup");
    fs::create_dir(&stage).unwrap();
    let disk = TemporaryBudget::new(32 * 1024 * 1024, None).unwrap();
    project
        .backup(&stage, &disk, &mut || {
            calls += 1;
            if calls == 2 {
                writer.take().unwrap().commit(next.clone())?;
            }
            Ok(())
        })
        .unwrap();
    assert!(calls >= 2);
    assert_ne!(project.current().unwrap(), Some(before.revision_id.clone()));
    let restored = temp.path().join("restored");
    restore_backup(
        &stage.join("backup.blobray"),
        &restored,
        &disk,
        &WorkingMemory::new(16 * 1024 * 1024).unwrap(),
        &mut || Ok(()),
    )
    .unwrap();
    assert_eq!(
        Project::open(&restored).unwrap().current().unwrap(),
        Some(before.revision_id)
    );
}

#[test]
fn restore_abandons_only_original_unfinished_runs_and_preserves_the_source_journal() {
    let temp = tempfile::tempdir().unwrap();
    let project = Project::create(&temp.path().join("source")).unwrap();
    let mut writer = project.writer().unwrap();
    let (run, _) = writer.register(ResourceBudget::default(), owner()).unwrap();
    let stage = temp.path().join("bundle");
    fs::create_dir(&stage).unwrap();
    let disk = TemporaryBudget::new(16 * 1024 * 1024, None).unwrap();
    project.backup(&stage, &disk, &mut || Ok(())).unwrap();
    let destination = temp.path().join("restored");
    restore_backup(
        &stage.join("backup.blobray"),
        &destination,
        &disk,
        &WorkingMemory::new(16 * 1024 * 1024).unwrap(),
        &mut || Ok(()),
    )
    .unwrap();
    let restored = Project::open(&destination).unwrap();
    let recovered = restored.run(&run.id).unwrap();
    assert_eq!(recovered.state, RunState::Abandoned);
    assert_eq!(project.run(&run.id).unwrap().state, RunState::Registered);
    let error = recovered.error.unwrap();
    let id: ArtifactId = error
        .message
        .split_whitespace()
        .last()
        .unwrap()
        .parse()
        .unwrap();
    restored.open_payload(&id, &mut || Ok(())).unwrap();
}

#[test]
fn execution_commit_failure_and_corruption_cannot_expose_valid_evidence() {
    use std::io::Write;
    let temp = tempfile::tempdir().unwrap();
    let project = Project::create(temp.path()).unwrap();
    let revision = project
        .writer()
        .unwrap()
        .commit(revision(&project))
        .unwrap()
        .revision_id;
    let request = ExecutionRequest {
        schema: 1,
        vendor: ExecutionTarget {
            revision: revision.clone(),
            source: FunctionSource::Input { input: 0 },
            entry: 4096,
            companions: vec![],
            abi: CallAbi::RiscvInteger,
            stack: MemorySeed {
                address: 8192,
                length: 4096,
                fill: None,
                bytes: vec![],
            },
        },
        replacement: None,
        binding: None,
        cases: vec![ExecutionCase {
            name: "one".into(),
            vendor: Invocation {
                arguments: [0; 8],
                memory: vec![],
                mmio: vec![],
            },
            replacement: None,
        }],
        case_execution: CaseExecution::Independent,
        max_events: 1,
        compare_return: false,
    };
    let memory = WorkingMemory::new(16 * 1024 * 1024).unwrap();
    let producer = ExecutionProducer {
        executor: "test/1".into(),
        environment: "test/1".into(),
        verifier: "test/1".into(),
    };
    let mut writer = project.writer().unwrap();
    let (mut run, path) = writer
        .register_operation(
            ResourceBudget::default(),
            owner(),
            RunOperation::Execute {
                request: request.clone(),
                producer: producer.clone(),
            },
            |_| {},
        )
        .unwrap();
    let stage = Staging::open(&path).unwrap();
    let mut file = stage.disk.temporary(&path.join("staging")).unwrap();
    serde_json::to_writer(
        &mut file,
        &ExecutionEvidence::Outcome {
            case: 0,
            replacement: false,
            stop: ExecutionStop::Returned {
                low: Some(0),
                high: None,
            },
            steps: 1,
        },
    )
    .unwrap();
    file.write_all(b"\n").unwrap();
    let records = stage.retain_temporary(file, &mut || Ok(())).unwrap();
    let mut manifest = ExecutionManifest {
        schema: 1,
        project: project.id().clone(),
        request,
        producer,
        records,
        verdict: None,
        complete: true,
    };
    manifest.complete = false;
    let invalid = stage.execution_receipt(&manifest, &mut || Ok(())).unwrap();
    assert!(
        writer
            .retain_execution(&run, &invalid, &memory, &mut || Ok(()))
            .is_err()
    );
    manifest.complete = true;
    let receipt = stage.execution_receipt(&manifest, &mut || Ok(())).unwrap();
    let retained = writer
        .retain_execution(&run, &receipt, &memory, &mut || Ok(()))
        .unwrap();
    let connection = open_connection(&project.root, true).unwrap();
    connection.execute_batch("CREATE TRIGGER reject_execution BEFORE UPDATE ON runs BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    assert!(writer.publish_execution(&mut run, retained).is_err());
    assert!(run.execution.is_none());
    assert!(
        project
            .execution(&receipt.execution, &memory, &mut || Ok(()))
            .is_err()
    );
    assert_eq!(project.current().unwrap(), Some(revision));
    connection
        .execute_batch("DROP TRIGGER reject_execution")
        .unwrap();
    let retained = writer
        .retain_execution(&run, &receipt, &memory, &mut || Ok(()))
        .unwrap();
    writer.publish_execution(&mut run, retained).unwrap();
    assert!(
        project
            .execution(&receipt.execution, &memory, &mut || Ok(()))
            .is_ok()
    );
    std::fs::write(project.object_path(&manifest.records), b"corrupt").unwrap();
    assert!(
        project
            .execution(&receipt.execution, &memory, &mut || Ok(()))
            .is_err()
    );
}
