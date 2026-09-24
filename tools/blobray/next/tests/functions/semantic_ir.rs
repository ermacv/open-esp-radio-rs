use super::*;
#[derive(Default)]
struct Sink(Vec<SemanticIrRecord>);
impl ElfSink for Sink {
    fn section(&mut self, _: &SectionRecord, _: &mut dyn RunControl) -> Result<()> {
        unreachable!()
    }
    fn symbol(&mut self, _: &SymbolRecord, _: &mut dyn RunControl) -> Result<()> {
        unreachable!()
    }
    fn relocation(&mut self, _: &RelocationRecord, _: &mut dyn RunControl) -> Result<()> {
        unreachable!()
    }
    fn diagnostic(&mut self, _: &Diagnostic, _: &mut dyn RunControl) -> Result<()> {
        unreachable!()
    }
}
impl InventorySink for Sink {}
impl app::DoctorSink for Sink {
    fn error(&mut self, _: &Error, _: &mut dyn RunControl) -> Result<()> {
        unreachable!()
    }
    fn unfinished(&mut self, _: &RunId, _: &mut dyn RunControl) -> Result<()> {
        unreachable!()
    }
}
impl app::QuerySink for Sink {
    fn summary(&mut self, _: &app::QuerySummary, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
    fn semantic_ir(&mut self, r: &SemanticIrRecord, _: &mut dyn RunControl) -> Result<()> {
        self.0.push(r.clone());
        Ok(())
    }
}
fn read(
    f: &Fixture,
    project: &Path,
    id: &ArtifactId,
) -> (SemanticIrManifest, Vec<SemanticIrRecord>) {
    let mut out = f
        .app
        .query(
            project,
            app::ReadQuery::SemanticIr { id: id.clone() },
            budget(),
        )
        .unwrap();
    let app::QuerySummary::SemanticIr { manifest, .. } = out.summary() else {
        panic!("IR")
    };
    let manifest = (**manifest).clone();
    assert_eq!(out.assessment(), &ResultAssessment::default());
    let mut sink = Sink::default();
    out.records(&|| false, &mut sink).unwrap();
    (manifest, sink.0)
}
#[test]
fn semantic_ir_build_prefix_export_restore_and_failure_keep_original_facts() {
    let f = fixture(object(BRANCH, BRANCH.len() as u64, false), true);
    let plan = investigations::plan(&f, InvestigationRequest::default());
    let published = investigations::publish(&f, &plan);
    assert_eq!(published.state, RunState::Completed, "{published:?}");
    let mut request = IrBuildRequest {
        scope: NavigationScope {
            revision: f.revision.clone(),
            publications: vec![published.publication.unwrap()],
            analyses: vec![],
            knowledge: None,
        },
        profiles: vec![IrProfile {
            name: "entry".into(),
            roots: IrRoots::NamePrefix {
                prefix: b"entry".to_vec(),
            },
            include_reachable: true,
        }],
    };
    fs::remove_file(f.dir.path().join("entry.a")).unwrap();
    fs::remove_file(f.dir.path().join("entry.o")).unwrap();
    let run = f
        .app
        .start_build_ir(&f.project, request.clone(), budget())
        .unwrap()
        .wait();
    assert_eq!(run.state, RunState::Completed, "{run:?}");
    assert_eq!(run.assessment, Some(ResultAssessment::default()));
    let id = run.semantic_ir.unwrap();
    let original = read(&f, &f.project, &id);
    assert_eq!(original.0.functions, 2);
    assert_eq!(original.0.profiles[0].roots, 2);
    assert_eq!(original.0.provenance_functions, 0);
    assert!(original.1.iter().any(|r| matches!(r, SemanticIrRecord::Fact { fact, .. } if matches!(&**fact, FunctionRecord::Condition { .. }))));
    for row in &original.1 {
        if let SemanticIrRecord::Function {
            function,
            manifest,
            name,
            ..
        } = row
        {
            assert_eq!(name.as_deref(), Some(b"entry".as_slice()));
            let (saved, expected) = export(&f, function.analysis.clone());
            assert_eq!(saved, **manifest);
            fs::remove_dir_all(f.dir.path().join("export")).unwrap();
            let actual: Vec<_> = original
                .1
                .iter()
                .filter_map(|r| match r {
                    SemanticIrRecord::Fact {
                        analysis,
                        record,
                        fact,
                    } if *analysis == function.analysis => Some((*record, *fact.clone())),
                    _ => None,
                })
                .collect();
            assert_eq!(
                actual,
                expected
                    .into_iter()
                    .enumerate()
                    .map(|(i, r)| (i as u64, r))
                    .collect::<Vec<_>>()
            );
        }
    }
    // Repeated builds have one content identity, with independently completed runs.
    let again = f
        .app
        .start_build_ir(&f.project, request.clone(), budget())
        .unwrap()
        .wait();
    assert_eq!(again.semantic_ir, Some(id.clone()), "{again:?}");
    for case in 0..3 {
        let mut bad = request.clone();
        let mut limits = budget();
        match case {
            0 => {
                bad.profiles[0].roots = IrRoots::NamePrefix {
                    prefix: b"missing".to_vec(),
                }
            }
            1 => limits.working_memory_bytes = Some(1024 * 1024),
            _ => limits.max_work_units = Some(1),
        }
        let failed = f
            .app
            .start_build_ir(&f.project, bad, limits)
            .unwrap()
            .wait();
        assert_ne!(failed.state, RunState::Completed, "{failed:?}");
        assert!(failed.semantic_ir.is_none() && failed.assessment.is_none());
        assert_eq!(read(&f, &f.project, &id), original);
    }
    let limited = app::Application::with_temporary_storage(
        Arc::new(LinuxHost::new(env!("CARGO_BIN_EXE_blobray").into(), None)),
        app::ApplicationLimits::default(),
        app::TemporaryStoragePolicy {
            root: Some(f.dir.path().join("limited")),
            operation_bytes: 1024 * 1024,
            total_bytes: 1024 * 1024,
        },
    )
    .unwrap();
    let failed = limited
        .start_build_ir(&f.project, request.clone(), budget())
        .unwrap()
        .wait();
    assert_eq!(failed.error.unwrap().code, ErrorCode::ResourceLimited);
    assert!(failed.semantic_ir.is_none());
    let handle = f
        .app
        .start_build_ir(&f.project, request.clone(), budget())
        .unwrap();
    assert!(handle.cancel());
    let cancelled = handle.wait();
    assert_eq!(cancelled.state, RunState::Cancelled, "{cancelled:?}");
    assert!(cancelled.semantic_ir.is_none());
    drop(handle);
    let analysis = original
        .1
        .iter()
        .find_map(|r| {
            if let SemanticIrRecord::Function { function, .. } = r {
                Some(function.analysis.clone())
            } else {
                None
            }
        })
        .unwrap();
    request.scope.publications.clear();
    request.scope.analyses = vec![analysis.clone()];
    let failed = f
        .app
        .start_build_ir(&f.project, request.clone(), budget())
        .unwrap()
        .wait();
    assert_eq!(failed.error.unwrap().code, ErrorCode::InvalidRequest); // names are not guessed
    request.profiles[0].roots = IrRoots::Analyses {
        analyses: vec![analysis],
    };
    assert_eq!(
        f.app
            .start_build_ir(&f.project, request.clone(), budget())
            .unwrap()
            .wait()
            .state,
        RunState::Completed
    );
    request.profiles.push(request.profiles[0].clone());
    assert!(
        matches!(f.app.start_build_ir(&f.project, request, budget()), Err(e) if e.code == ErrorCode::InvalidRequest)
    );
    let backup = f
        .app
        .query(&f.project, app::ReadQuery::Backup, budget())
        .unwrap();
    let path = f.dir.path().join("ir-backup.blobray");
    let mut backup = backup;
    backup.export_backup(&path, &|| false).unwrap();
    drop(backup);
    let restored = f.dir.path().join("restored");
    let mut restore = f
        .app
        .query(
            &restored,
            app::ReadQuery::Restore {
                bundle: OriginPath::from_path(&path),
            },
            budget(),
        )
        .unwrap();
    restore.publish_restore(&restored, &|| false).unwrap();
    drop(restore);
    assert_eq!(read(&f, &restored, &id), original);
    let cli = interfaces::cli(&f, &["ir", "show", id.as_str()]);
    assert_eq!(cli["summary"]["manifest"]["functions"], 2);
    assert_eq!(cli["records"].as_array().unwrap().len(), original.1.len());
    let destination = f.dir.path().join("ir-export.json");
    interfaces::cli(
        &f,
        &[
            "ir",
            "show",
            id.as_str(),
            "--output",
            destination.to_str().unwrap(),
        ],
    );
    let exported: serde_json::Value =
        serde_json::from_slice(&fs::read(&destination).unwrap()).unwrap();
    assert_eq!(exported, cli);
    let record_path = f
        .project
        .join(".blobray-next/objects")
        .join(original.0.records.as_str());
    fs::write(&record_path, b"corrupt").unwrap();
    let error = f
        .app
        .query(
            &f.project,
            app::ReadQuery::SemanticIr { id: id.clone() },
            budget(),
        )
        .err()
        .unwrap();
    assert_eq!(error.code, ErrorCode::Integrity);
    assert_eq!(read(&f, &restored, &id), original);
}
