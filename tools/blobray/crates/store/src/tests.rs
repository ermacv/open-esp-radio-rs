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
fn incompatible_journals_are_rejected_by_all_readers_without_mutation() {
    let temp = tempfile::tempdir().unwrap();
    let project = Project::create(temp.path()).unwrap();
    let mut writer = project.writer().unwrap();
    let (run, _) = writer.register(ResourceBudget::default(), owner()).unwrap();
    let connection = open_connection(&project.root, true).unwrap();
    for schema in (1..run.schema).chain([run.schema + 1, 999]) {
        let mut value = serde_json::to_value(&run).unwrap();
        value["schema"] = schema.into();
        let raw = value.to_string();
        connection
            .execute("UPDATE runs SET record=?1", [&raw])
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
        let after: String = connection
            .query_row("SELECT record FROM runs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(after, raw);
    }
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
        selector: (symbol.clone()).into(),
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
    serde_json::to_writer(
        &mut file,
        &FunctionRecord::SemanticGap {
            offset: 0,
            reason: SemanticGapReason::UnsupportedInstruction,
        },
    )
    .unwrap();
    file.write_all(b"\n").unwrap();
    let records = stage.retain_temporary(file, &mut || Ok(())).unwrap();
    let manifest = FunctionManifest {
        schema: FUNCTION_SCHEMA,
        semantics: Some(SemanticSummary {
            complete: false,
            gaps: 1,
            ..Default::default()
        }),
        recipe: FunctionRecipe {
            research: None,
            abi: RiscvAbi::Ilp32,
            address_space: CodeAddressSpace::Section,
            schema: FUNCTION_SCHEMA,
            policy: FUNCTION_POLICY,
            semantics: Some("fixture-values/1".into()),
            decoder: "fixture/1".into(),
            project: project.id().clone(),
            revision: snapshot.revision_id.clone(),
            source: FunctionSource::Input { input: 0 },
            selector: (symbol).into(),
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
    assert_eq!(
        record
            .assessment
            .as_ref()
            .and_then(|a| a.coverage.as_ref())
            .map(|c| c.status == CoverageStatus::Complete),
        Some(false)
    );
    assert_eq!(
        reopened
            .analysis(record.analysis.as_ref().unwrap(), &mut || Ok(()))
            .unwrap()
            .manifest,
        manifest
    );

    let memory = WorkingMemory::new(1024 * 1024).unwrap();
    let mut reader = reopened.analysis_reader(&memory);
    let id = record.analysis.as_ref().unwrap();
    reader.analysis(id, &mut || Ok(())).unwrap();
    assert!(memory.used() > 0);
    // Dependency handles belong only to this reader, never to the returned lease.
    let used = memory.used();
    struct Bytes(u64);
    impl RunControl for Bytes {
        fn checkpoint(&mut self, _: u64) -> Result<()> {
            Ok(())
        }
        fn bytes(&mut self, n: usize) -> Result<()> {
            self.0 += n as u64;
            Ok(())
        }
    }
    let mut cached = Bytes(0);
    let mut fresh = Bytes(0);
    reader.analysis(id, &mut cached).unwrap();
    reopened.analysis(id, &mut fresh).unwrap();
    let root_size = reopened
        .open_payload(
            &manifest.recipe.revision.as_str().parse().unwrap(),
            &mut || Ok(()),
        )
        .unwrap()
        .len();
    assert_eq!(
        fresh.0 - cached.0,
        2 * root_size,
        "shared revision is neither re-read nor re-hashed"
    );
    assert_eq!(memory.used(), used);
    check_ir_publication(&reopened, id, &manifest);
    // A cached revision must not hide subsequent corruption of function facts.
    std::fs::write(
        reopened
            .root
            .join("objects")
            .join(manifest.records.as_str()),
        b"corrupt",
    )
    .unwrap();
    assert!(
        matches!(reader.analysis(id, &mut || Ok(())), Err(e) if e.code == ErrorCode::Integrity)
    );
    drop(reader);
    assert_eq!(memory.used(), 0);
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
        selector: (symbol.clone()).into(),
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
        schema: 3,
        policy: 4,
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
                schema: FUNCTION_SCHEMA,
                recipe: FunctionRecipe {
                    research: None,
                    abi: RiscvAbi::Ilp32,
                    address_space: CodeAddressSpace::Section,
                    schema: FUNCTION_SCHEMA,
                    policy: FUNCTION_POLICY,
                    project: writer.project.id.clone(),
                    revision: revision.clone(),
                    source: FunctionSource::Input { input: 0 },
                    selector: (symbol).into(),
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
        schema: EXECUTION_SCHEMA,
        vendor: ExecutionTarget {
            revision: revision.clone(),
            source: FunctionSource::Input { input: 0 },
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
            relation: None,
            reset: SessionReset::Cold,
            name: "one".into(),
            vendor: Invocation {
                observe_calls: None,
                observe_timeline: TimelineCapture::default(),
                observe_memory: vec![],
                goal: ExecutionGoal::Return,
                entry: 4096,
                arguments: vec![Some(0); 8],
                memory: vec![],
                models: vec![],
                calls: vec![],
                tables: vec![],
                services: vec![],
            },
            replacement: None,
        }],
        max_events: 1,
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
        effect_contracts: vec![],
        projections: vec![],
        call_pairs: vec![],
        schema: EXECUTION_SCHEMA,
        project: project.id().clone(),
        request,
        producer,
        records,
        verdict: None,
        complete: true,
    };
    // Receipt validation must not confuse reaching a boundary with returning,
    // or accept a blocked outcome without a failed warm predecessor.
    let mut goal_manifest = manifest.clone();
    goal_manifest.complete = false;
    goal_manifest.request.cases[0].vendor.goal = ExecutionGoal::ReachSymbol {
        target: ExecutionSymbol {
            source: FunctionSource::Input { input: 0 },
            symbol: SymbolId {
                object: ObjectId {
                    artifact: ArtifactId::of_bytes(b"goal"),
                    location: ObjectLocation::Standalone,
                },
                table: SymbolTableKind::Static,
                table_section: 3,
                index: 1,
            },
        },
    };
    for stop in [
        ExecutionStop::Returned {
            low: Some(0),
            high: None,
        },
        ExecutionStop::BlockedByPriorPhase,
        ExecutionStop::ObservedCall {
            pc: 0x1000,
            target: 0x1004,
            tail: false,
        },
    ] {
        let bytes = serde_json::to_vec(&ExecutionEvidence::Outcome {
            case: 0,
            replacement: false,
            stop,
            steps: 1,
        })
        .unwrap();
        let error = validate_execution_records(
            &goal_manifest,
            &bytes.as_slice(),
            &WorkingMemory::new(1024 * 1024).unwrap(),
            &mut || Ok(()),
        )
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::Integrity);
        assert!(
            error.message.contains("goal") || error.message.contains("blocking"),
            "{error:?}"
        );
    }
    let bytes = serde_json::to_vec(&ExecutionEvidence::Outcome {
        case: 0,
        replacement: false,
        stop: ExecutionStop::GoalNotReached {
            low: Some(0),
            high: None,
        },
        steps: 1,
    })
    .unwrap();
    validate_execution_records(
        &goal_manifest,
        &bytes.as_slice(),
        &WorkingMemory::new(1024 * 1024).unwrap(),
        &mut || Ok(()),
    )
    .unwrap();
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

#[test]
fn older_project_formats_are_rejected_without_mutation() {
    for schema in 1..SCHEMA {
        let temp = tempfile::tempdir().unwrap();
        Project::create(temp.path()).unwrap();
        let db = temp.path().join(STATE).join("project.sqlite3");
        let connection = rusqlite::Connection::open(&db).unwrap();
        connection
            .pragma_update(None, "user_version", schema)
            .unwrap();
        drop(connection);
        let before = fs::read(&db).unwrap();
        assert!(matches!(
            Project::open(temp.path()),
            Err(Error {
                code: ErrorCode::Incompatible,
                ..
            })
        ));
        assert!(Writer::open(temp.path()).is_err());
        assert_eq!(fs::read(&db).unwrap(), before);
    }
}

#[test]
fn durable_assessment_cannot_describe_another_result_or_fabricate_a_verdict() {
    let temp = tempfile::tempdir().unwrap();
    let project = Project::create(temp.path()).unwrap();
    let mut writer = project.writer().unwrap();
    let (mut run, _) = writer.register(ResourceBudget::default(), owner()).unwrap();
    let revision: RevisionId = ArtifactId::of_bytes(b"result").as_str().parse().unwrap();
    run.state = RunState::Completed;
    run.revision = Some(revision.clone());
    run.assessment = Some(ResultAssessment::covered(
        CoverageSubject::Inventory(revision),
        false,
    ));
    let decode = |run: &RunRecord| jobs::decode_run(&serde_json::to_string(run).unwrap());
    assert_eq!(decode(&run).unwrap(), run);
    let mut forged = run.clone();
    forged
        .assessment
        .as_mut()
        .unwrap()
        .coverage
        .as_mut()
        .unwrap()
        .subject =
        CoverageSubject::Inventory(ArtifactId::of_bytes(b"other").as_str().parse().unwrap());
    assert_eq!(decode(&forged).unwrap_err().code, ErrorCode::Integrity);
    forged = run.clone();
    forged.assessment.as_mut().unwrap().comparison = Some(ComparisonVerdict::Match);
    assert_eq!(decode(&forged).unwrap_err().code, ErrorCode::Integrity);
    forged = run.clone();
    forged.state = RunState::ResourceLimited;
    assert_eq!(decode(&forged).unwrap_err().code, ErrorCode::Integrity);
    forged = run;
    forged.execution = Some(ArtifactId::of_bytes(b"unexpected"));
    assert_eq!(decode(&forged).unwrap_err().code, ErrorCode::Integrity);
}

#[test]
fn frozen_knowledge_snapshot_preserves_review_history_and_checks_superseded_events() {
    let temp = tempfile::tempdir().unwrap();
    let project = Project::create(temp.path()).unwrap();
    let mut writer = project.writer().unwrap();
    let payload = ArtifactId::of_bytes(b"source");
    let proposal = KnowledgeProposal {
        subject: "register".to_owned().try_into().unwrap(),
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
            text: "reviewed proposal".into(),
        },
        evidence: vec![],
        note: None,
    };
    let first: AssertionId = ArtifactId::of_bytes(b"first").as_str().parse().unwrap();
    let second: AssertionId = ArtifactId::of_bytes(b"second").as_str().parse().unwrap();
    let mut revisions = Vec::new();
    for (assertion, action) in [
        (
            first.clone(),
            KnowledgeAction::Propose {
                proposal: proposal.clone(),
            },
        ),
        (
            first.clone(),
            KnowledgeAction::Review {
                assertion: first.clone(),
                decision: ReviewDecision::Accept,
                supersedes: None,
            },
        ),
        (second.clone(), KnowledgeAction::Propose { proposal }),
        (
            second.clone(),
            KnowledgeAction::Review {
                assertion: second.clone(),
                decision: ReviewDecision::Reject,
                supersedes: None,
            },
        ),
        (
            second.clone(),
            KnowledgeAction::Review {
                assertion: second.clone(),
                decision: ReviewDecision::Accept,
                supersedes: Some(first.clone()),
            },
        ),
    ] {
        let change = KnowledgeChange {
            expected_base: revisions.last().cloned(),
            actor: "fixture".into(),
            reason: "review".into(),
            action,
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
                    assertion,
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
        writer
            .publish_knowledge(&mut run, retained, &mut || Ok(()))
            .unwrap();
        revisions.push(receipt.revision);
    }
    struct Control(u64);
    impl RunControl for Control {
        fn checkpoint(&mut self, _: u64) -> Result<()> {
            Ok(())
        }
        fn measure(&mut self, metric: WorkMetric, amount: u64) {
            if matches!(metric, WorkMetric::KnowledgeHistoryPasses) {
                self.0 += amount;
            }
        }
    }
    let memory = WorkingMemory::new(2 * 1024 * 1024).unwrap();
    for at in &revisions {
        let mut control = Control(0);
        let snapshot = project
            .knowledge_snapshot(Some(at), &memory, &mut control)
            .unwrap();
        assert_eq!(control.0, 1);
        for entry in snapshot.entries() {
            assert_eq!(
                *entry,
                project
                    .knowledge_entry(at, &entry.id, &mut || Ok(()))
                    .unwrap()
            );
        }
        for entry in snapshot.entries() {
            assert_eq!(snapshot.get(&entry.id, &mut control).unwrap(), Some(entry));
        }
        assert!(
            snapshot
                .get(
                    &ArtifactId::of_bytes(b"absent").as_str().parse().unwrap(),
                    &mut control
                )
                .unwrap()
                .is_none()
        );
        assert_eq!(control.0, 1, "indexed lookups do not replay history");
        drop(snapshot);
        assert_eq!(memory.used(), 0);
    }
    let snapshot = project
        .knowledge_snapshot(revisions.last(), &memory, &mut Control(0))
        .unwrap();
    assert_eq!(
        snapshot.entries().find(|e| e.id == first).unwrap().state,
        AssertionState::Superseded
    );
    assert_eq!(
        snapshot.entries().find(|e| e.id == second).unwrap().state,
        AssertionState::Accepted
    );
    drop(snapshot);
    fs::write(
        project.root.join("objects").join(revisions[0].as_str()),
        b"corrupt old event",
    )
    .unwrap();
    assert!(matches!(
        project.knowledge_snapshot(revisions.last(), &memory, &mut Control(0)),
        Err(Error {
            code: ErrorCode::Integrity,
            ..
        })
    ));
    assert_eq!(memory.used(), 0);
}

fn check_ir_publication(
    project: &Project,
    analysis: &FunctionAnalysisId,
    function: &FunctionManifest,
) {
    use std::io::Write;
    let memory = WorkingMemory::new(16 * 1024 * 1024).unwrap();
    let request = IrBuildRequest {
        scope: NavigationScope {
            revision: function.recipe.revision.clone(),
            publications: vec![],
            analyses: vec![analysis.clone()],
            knowledge: None,
        },
        profiles: vec![IrProfile {
            name: "fixture".into(),
            roots: IrRoots::All,
            include_reachable: false,
        }],
    };
    let mut writer = project.writer().unwrap();
    let (mut run, path) = writer
        .register_operation(
            ResourceBudget::default(),
            owner(),
            RunOperation::BuildIr {
                request: request.clone(),
            },
            |_| {},
        )
        .unwrap();
    let stage = Staging::open(&path).unwrap();
    let row = SemanticIrRecord::Function {
        function: NavigationFunction {
            analysis: analysis.clone(),
            location: FunctionLocation {
                source: function.recipe.source.clone(),
                selector: function.recipe.selector.clone(),
            },
        },
        manifest: Box::new(function.clone()),
        name: None,
        profiles: vec![0],
        roots: vec![0],
        provenance_only: false,
    };
    let mut file = stage.disk.temporary(&path.join("staging")).unwrap();
    serde_json::to_writer(&mut file, &row).unwrap();
    file.write_all(b"\n").unwrap();
    let records = stage.retain_temporary(file, &mut || Ok(())).unwrap();
    let manifest = SemanticIrManifest {
        schema: 1,
        policy: 1,
        project: project.id().clone(),
        request,
        records,
        record_count: 1,
        functions: 1,
        provenance_functions: 0,
        unavailable_entries: 0,
        profiles: vec![IrProfileSummary {
            name: "fixture".into(),
            roots: 1,
            functions: 1,
            partial_functions: 1,
            unresolved_links: 0,
        }],
    };
    for case in 0..3 {
        let mut bad = manifest.clone();
        match case {
            0 => bad.profiles[0].partial_functions = 0,
            1 => bad.profiles[0].unresolved_links = 1,
            _ => bad.request.profiles[0].include_reachable = true,
        }
        let receipt = stage.ir_receipt(&bad, &mut || Ok(())).unwrap();
        assert!(
            matches!(writer.retain_ir(&run, &receipt, &memory, &mut || Ok(())), Err(e) if e.code == ErrorCode::Integrity)
        );
        assert!(run.semantic_ir.is_none());
    }
    let receipt = stage.ir_receipt(&manifest, &mut || Ok(())).unwrap();
    let retained = writer
        .retain_ir(&run, &receipt, &memory, &mut || Ok(()))
        .unwrap();
    let connection = open_connection(&project.root, true).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_ir BEFORE UPDATE ON runs BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    assert!(writer.publish_ir(&mut run, retained).is_err());
    assert!(run.semantic_ir.is_none());
    let count: i64 = connection
        .query_row("SELECT count(*) FROM semantic_ir", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
    connection.execute_batch("DROP TRIGGER fail_ir").unwrap();
    let retained = writer
        .retain_ir(&run, &receipt, &memory, &mut || Ok(()))
        .unwrap();
    writer.publish_ir(&mut run, retained).unwrap();
    assert_eq!(
        project
            .semantic_ir(&receipt.semantic_ir, &memory, &mut || Ok(()))
            .unwrap()
            .manifest,
        manifest
    );
    assert_eq!(
        project.current().unwrap(),
        Some(function.recipe.revision.clone())
    );
    // Valid JSON and a completed state cannot substitute another admitted build.
    let original = serde_json::to_string(&run).unwrap();
    if let RunOperation::BuildIr { request } = &mut run.operation {
        request.profiles[0].name = "changed".into();
    }
    connection
        .execute(
            "UPDATE runs SET record=?2 WHERE id=?1",
            params![run.id.as_str(), serde_json::to_string(&run).unwrap()],
        )
        .unwrap();
    assert!(
        matches!(project.semantic_ir(&receipt.semantic_ir, &memory, &mut || Ok(())), Err(e) if e.code == ErrorCode::Integrity)
    );
    connection
        .execute(
            "UPDATE runs SET record=?2 WHERE id=?1",
            params![run.id.as_str(), original],
        )
        .unwrap();
    assert_eq!(memory.used(), 0);
}

#[test]
fn retained_models_reject_missing_forged_identity_closure_and_match() {
    let temp = tempfile::tempdir().unwrap();
    let project = Project::create(temp.path()).unwrap();
    let revision = project
        .writer()
        .unwrap()
        .commit(revision(&project))
        .unwrap()
        .revision_id;
    let target = ExecutionTarget {
        revision,
        source: FunctionSource::Input { input: 0 },
        companions: vec![],
        abi: CallAbi::RiscvInteger,
        stack: MemorySeed {
            address: 8192,
            length: 4096,
            fill: None,
            bytes: vec![],
        },
    };
    let declaration = DeviceDeclaration {
        id: "sequence".into(),
        applicability: "fixture".into(),
        lifetime: RegionLifetime::Phase,
        behavior: DeviceBehavior::SequenceRead {
            address: 0x3000,
            width: 4,
            values: vec![7, 9],
        },
    };
    let input = Invocation {
        observe_calls: None,
        observe_timeline: TimelineCapture::default(),
        observe_memory: vec![],
        entry: 4096,
        goal: ExecutionGoal::Return,
        arguments: vec![],
        memory: vec![],
        models: vec![declaration.clone()],
        calls: vec![],
        tables: vec![],
        services: vec![],
    };
    let mut rows = Vec::new();
    for replacement in [false, true] {
        rows.push(ExecutionEvidence::Event {
            case: 0,
            replacement,
            event: ExecutionEvent::Read {
                address: 0x3000,
                width: 4,
                value: 7,
            },
        });
        rows.push(ExecutionEvidence::Model {
            case: 0,
            replacement,
            observation: ModelObservation {
                id: declaration.id.clone(),
                definition: declaration.identity(&mut || Ok(())).unwrap(),
                lifetime: RegionLifetime::Phase,
                reads: 1,
                writes: 0,
                remaining_reads: 1,
                remaining_writes: 0,
                closed: true,
                issue: None,
                status: ModelStatus::Incomplete,
            },
        });
        rows.push(ExecutionEvidence::Outcome {
            case: 0,
            replacement,
            steps: 2,
            stop: ExecutionStop::Returned {
                low: Some(7),
                high: None,
            },
        });
    }
    rows.push(ExecutionEvidence::Comparison {
        case: 0,
        result: CaseComparison {
            effect_claim: None,
            effect_gap: None,
            verdict: ComparisonVerdict::Incomplete,
            difference: None,
        },
    });
    let manifest = ExecutionManifest {
        effect_contracts: vec![],
        projections: vec![],
        call_pairs: vec![],
        schema: EXECUTION_SCHEMA,
        project: project.id().clone(),
        request: ExecutionRequest {
            schema: EXECUTION_SCHEMA,
            vendor: target.clone(),
            replacement: Some(target),
            binding: Some(CompiledBinding::ProductionEntry),
            cases: vec![ExecutionCase {
                relation: Some(ComparisonRelation {
                    effects: None,
                    projection: None,
                    calls: false,
                    reviewed_calls: None,
                    returns: ReturnWords {
                        low: true,
                        high: false,
                    },
                    events: EventChannels {
                        timeline: TimelineCapture::default(),
                        mmio_read: true,
                        mmio_write: true,
                        fence: true,
                        delay: true,
                    },
                    memory: vec![],
                }),
                name: "one".into(),
                reset: SessionReset::Cold,
                vendor: input.clone(),
                replacement: Some(input),
            }],
            max_events: 4,
        },
        producer: ExecutionProducer {
            executor: "test/1".into(),
            environment: "test/1".into(),
            verifier: "test/1".into(),
        },
        records: ArtifactId::of_bytes(b"unused"),
        verdict: Some(ComparisonVerdict::Incomplete),
        complete: false,
    };
    let validate = |m: &ExecutionManifest, rows: &[ExecutionEvidence]| {
        let mut bytes = Vec::new();
        for row in rows {
            serde_json::to_writer(&mut bytes, row).unwrap();
            bytes.push(b'\n');
        }
        validate_execution_records(
            m,
            &bytes.as_slice(),
            &WorkingMemory::new(1024 * 1024).unwrap(),
            &mut || Ok(()),
        )
    };
    validate(&manifest, &rows).unwrap();
    let model_index = rows
        .iter()
        .position(|r| matches!(r, ExecutionEvidence::Model { .. }))
        .unwrap();
    for variant in 0..5 {
        let mut forged = rows.clone();
        if variant == 0 {
            forged.remove(model_index);
        } else if let ExecutionEvidence::Model { observation, .. } = &mut forged[model_index] {
            match variant {
                1 => observation.definition = ArtifactId::of_bytes(b"other assumption"),
                2 => {
                    observation.closed = false;
                    observation.status = ModelStatus::Open;
                }
                3 => observation.status = ModelStatus::Complete,
                4 => {
                    observation.remaining_reads = 0;
                    observation.status = ModelStatus::Complete;
                }
                _ => unreachable!(),
            }
        }
        assert_eq!(
            validate(&manifest, &forged).unwrap_err().code,
            ErrorCode::Integrity
        );
    }
    // Code completion alone cannot admit MATCH with unknown selected call words.
    {
        let mut m = manifest.clone();
        m.complete = true;
        {
            let i = &mut m.request.cases[0].vendor;
            i.models.clear();
            i.observe_calls = Some(CallCapture {
                include_tail: false,
                argument_words: 1,
                overrides: vec![],
            });
        }
        m.request.cases[0].replacement = Some(m.request.cases[0].vendor.clone());
        m.request.cases[0].relation.as_mut().unwrap().calls = true;
        let mut captured = Vec::new();
        for replacement in [false, true] {
            for event in [
                ExecutionEvent::CallTransfer {
                    site: 4096,
                    target: 4100,
                    tail: false,
                    indirect: false,
                    stack: Some(12288),
                    target_kind: ObservedCallTarget::CapturedCode,
                    words: 1,
                },
                ExecutionEvent::TransferArgument {
                    word: 0,
                    value: ObservedWord::Unknown,
                },
            ] {
                captured.push(ExecutionEvidence::Event {
                    case: 0,
                    replacement,
                    event,
                });
            }
            captured.push(ExecutionEvidence::Outcome {
                case: 0,
                replacement,
                steps: 3,
                stop: ExecutionStop::Returned {
                    low: Some(7),
                    high: None,
                },
            });
        }
        captured.push(ExecutionEvidence::Comparison {
            case: 0,
            result: CaseComparison {
                effect_claim: None,
                effect_gap: None,
                verdict: ComparisonVerdict::Incomplete,
                difference: None,
            },
        });
        validate(&m, &captured).unwrap();
        m.verdict = Some(ComparisonVerdict::Match);
        if let ExecutionEvidence::Comparison { result, .. } = captured.last_mut().unwrap() {
            result.verdict = ComparisonVerdict::Match;
        }
        assert_eq!(
            validate(&m, &captured).unwrap_err().code,
            ErrorCode::Integrity
        );
        m.request.cases[0].relation.as_mut().unwrap().calls = false;
        validate(&m, &captured).unwrap(); // excluded unknown evidence stays retained
    }
    let mut forged = rows;
    for row in &mut forged {
        if let ExecutionEvidence::Comparison { result, .. } = row {
            result.verdict = ComparisonVerdict::Match;
        }
    }
    let mut forged_manifest = manifest;
    forged_manifest.verdict = Some(ComparisonVerdict::Match);
    assert_eq!(
        validate(&forged_manifest, &forged).unwrap_err().code,
        ErrorCode::Integrity
    );
}

#[test]
fn retained_call_pairs_require_exact_review_content_and_release_admitted_owners() {
    let temp = tempfile::tempdir().unwrap();
    let project = Project::create(temp.path()).unwrap();
    let id = ArtifactId::of_bytes(b"fixture");
    let object = ObjectId {
        artifact: id.clone(),
        location: ObjectLocation::Standalone,
    };
    let occurrence = KnowledgeOccurrence {
        revision: id.as_str().parse().unwrap(),
        source: FunctionSource::Input { input: 0 },
        object: object.clone(),
        symbol: Some(SymbolId {
            object,
            table: SymbolTableKind::Static,
            table_section: 3,
            index: 1,
        }),
    };
    let pair = CallCorrespondence {
        vendor: CallEndpoint {
            occurrence: occurrence.clone(),
            boundary: ReviewedCallBoundary::Code { address: 0x1000 },
        },
        replacement: CallEndpoint {
            occurrence: occurrence.clone(),
            boundary: ReviewedCallBoundary::Code { address: 0x2000 },
        },
        arguments: CallArguments::Exact { words: 0 },
        applicability: "fixture".into(),
        reason: "reviewed fixture".into(),
    };
    let assertion: AssertionId = ArtifactId::of_bytes(b"assertion").as_str().parse().unwrap();
    let mut base = None;
    for action in [
        KnowledgeAction::Propose {
            proposal: KnowledgeProposal {
                subject: "fixture".to_owned().try_into().unwrap(),
                occurrence,
                claim: KnowledgeClaim::CallPair {
                    correspondence: Box::new(pair.clone()),
                },
                evidence: vec![EvidenceRef::Document {
                    payload: id.clone(),
                }],
                note: None,
            },
        },
        KnowledgeAction::Review {
            assertion: assertion.clone(),
            decision: ReviewDecision::Accept,
            supersedes: None,
        },
    ] {
        let mut writer = project.writer().unwrap();
        let change = KnowledgeChange {
            expected_base: base,
            actor: "fixture".into(),
            reason: "fixture".into(),
            action,
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
        let receipt = Staging::open(&path)
            .unwrap()
            .knowledge_receipt(
                &KnowledgeManifest {
                    schema: 2,
                    project: project.id().clone(),
                    change,
                    assertion: assertion.clone(),
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
        writer
            .publish_knowledge(&mut run, retained, &mut || Ok(()))
            .unwrap();
        base = Some(receipt.revision);
    }
    let review = CallPairReview {
        knowledge: base.unwrap(),
        assertion,
    };
    let target = ExecutionTarget {
        revision: id.as_str().parse().unwrap(),
        source: FunctionSource::Input { input: 0 },
        companions: vec![],
        abi: CallAbi::RiscvInteger,
        stack: MemorySeed {
            address: 0x8000,
            length: 4096,
            fill: None,
            bytes: vec![],
        },
    };
    let input = Invocation {
        entry: 0x1000,
        goal: ExecutionGoal::Return,
        arguments: vec![],
        memory: vec![],
        models: vec![],
        calls: vec![],
        tables: vec![],
        services: vec![],
        observe_memory: vec![],
        observe_timeline: TimelineCapture::default(),
        observe_calls: Some(CallCapture {
            include_tail: false,
            argument_words: 0,
            overrides: vec![],
        }),
    };
    let request = ExecutionRequest {
        schema: EXECUTION_SCHEMA,
        vendor: target.clone(),
        replacement: Some(target),
        binding: Some(CompiledBinding::SharedCore),
        max_events: 4,
        cases: vec![ExecutionCase {
            name: "fixture".into(),
            reset: SessionReset::Cold,
            vendor: input.clone(),
            replacement: Some(input),
            relation: Some(ComparisonRelation {
                effects: None,
                projection: None,
                returns: ReturnWords {
                    low: false,
                    high: false,
                },
                events: EventChannels {
                    timeline: TimelineCapture::default(),
                    mmio_read: false,
                    mmio_write: false,
                    fence: false,
                    delay: false,
                },
                memory: vec![],
                calls: false,
                reviewed_calls: Some(ReviewedCalls {
                    pairs: vec![review.clone()],
                    unlisted: UnlistedCalls::Exclude,
                }),
            }),
        }],
    };
    let memory = WorkingMemory::new(4 * 1024 * 1024).unwrap();
    for _ in 0..3 {
        let selected = project
            .execution_call_pairs(&request, &memory, &mut || Ok(()))
            .unwrap();
        assert_eq!(
            selected.pairs,
            vec![ResolvedCallPair {
                review: review.clone(),
                correspondence: pair.clone()
            }]
        );
        assert!(memory.used() > 0);
        drop(selected);
        assert_eq!(memory.used(), 0);
    }
    let small = WorkingMemory::new(1).unwrap();
    assert!(matches!(
        project.execution_call_pairs(&request, &small, &mut || Ok(())),
        Err(Error {
            code: ErrorCode::ResourceLimited,
            ..
        })
    ));
    assert_eq!(small.used(), 0);
    let mut manifest = ExecutionManifest {
        effect_contracts: vec![],
        projections: vec![],
        schema: EXECUTION_SCHEMA,
        project: project.id().clone(),
        request,
        producer: ExecutionProducer {
            executor: "test".into(),
            environment: "test".into(),
            verifier: "test".into(),
        },
        records: id,
        verdict: Some(ComparisonVerdict::Match),
        complete: true,
        call_pairs: vec![ResolvedCallPair {
            review,
            correspondence: pair,
        }],
    };
    project
        .validate_execution_call_pairs(&manifest, &memory, &mut || Ok(()))
        .unwrap();
    manifest.call_pairs[0].correspondence.arguments = CallArguments::Ignore;
    assert_eq!(
        project
            .validate_execution_call_pairs(&manifest, &memory, &mut || Ok(()))
            .unwrap_err()
            .code,
        ErrorCode::Integrity
    );
    manifest.call_pairs.clear();
    assert_eq!(
        project
            .validate_execution_call_pairs(&manifest, &memory, &mut || Ok(()))
            .unwrap_err()
            .code,
        ErrorCode::Integrity
    );
}
