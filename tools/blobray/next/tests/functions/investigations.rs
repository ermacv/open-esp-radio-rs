use super::*;
fn plan(f: &Fixture, request: InvestigationRequest) -> InvestigationPlan {
    let decoder = blobray_backend_riscv::RiscvDecoder;
    let output = f
        .app
        .query(
            &f.project,
            app::ReadQuery::PlanInvestigation {
                request,
                producer: FunctionProducer {
                    decoder: decoder.identity().into(),
                    semantics: decoder.semantic_identity().into(),
                },
            },
            budget(),
        )
        .unwrap();
    let app::QuerySummary::InvestigationPlan { plan } = output.summary() else {
        panic!("expected plan");
    };
    (**plan).clone()
}
fn publish(f: &Fixture, plan: &InvestigationPlan) -> app::RunRecord {
    f.app
        .start_analyze_project(&f.project, plan, budget())
        .unwrap()
        .wait()
}
#[derive(Default)]
struct Results {
    members: Vec<InvestigationMember>,
    findings: Vec<InvestigationFinding>,
}
impl ElfSink for Results {
    fn section(&mut self, _: &SectionRecord, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
    fn symbol(&mut self, _: &SymbolRecord, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
    fn relocation(&mut self, _: &RelocationRecord, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
    fn diagnostic(&mut self, _: &Diagnostic, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
}
impl InventorySink for Results {}
impl app::DoctorSink for Results {
    fn error(&mut self, e: &Error, _: &mut dyn RunControl) -> Result<()> {
        Err(e.clone())
    }
    fn unfinished(&mut self, _: &RunId, _: &mut dyn RunControl) -> Result<()> {
        panic!("unfinished")
    }
}
impl app::QuerySink for Results {
    fn investigation_member(
        &mut self,
        r: &InvestigationMember,
        _: &mut dyn RunControl,
    ) -> Result<()> {
        self.members.push(r.clone());
        Ok(())
    }
    fn finding(&mut self, r: &InvestigationFinding, _: &mut dyn RunControl) -> Result<()> {
        self.findings.push(r.clone());
        Ok(())
    }
    fn summary(&mut self, _: &app::QuerySummary, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
}
fn read(
    f: &Fixture,
    id: &PublicationId,
    filter: InvestigationFilter,
) -> (InvestigationManifest, Results) {
    let mut output = f
        .app
        .query(
            &f.project,
            app::ReadQuery::Publication {
                id: id.clone(),
                filter,
            },
            budget(),
        )
        .unwrap();
    let app::QuerySummary::Publication { manifest, .. } = output.summary() else {
        panic!("expected publication");
    };
    let manifest = (**manifest).clone();
    let mut results = Results::default();
    output.records(&|| false, &mut results).unwrap();
    (manifest, results)
}
#[test]
fn library_results_equal_single_function_and_survive_move_and_deleted_origins() {
    let code = words(&[
        0x60000537, 0x12050513, 0x02a00593, 0x00b52223, 0x00852603, 0x00072683, 0x00008067,
    ]);
    let mut f = fixture(object(&code, code.len() as u64, false), true);
    let plan = plan(&f, InvestigationRequest::default());
    assert_eq!(plan.recipe.functions, 2);
    fs::remove_file(f.dir.path().join("entry.a")).unwrap();
    fs::remove_file(f.dir.path().join("entry.o")).unwrap();
    let single = analyze(&f);
    assert_eq!(single.state, RunState::Completed);
    let run = publish(&f, &plan);
    assert_eq!(run.state, RunState::Completed, "{:?}", run.error);
    let id = run.publication.unwrap();
    let (manifest, records) = read(&f, &id, InvestigationFilter::Members);
    assert_eq!(manifest.coverage.functions, 2);
    assert_eq!(manifest.coverage.analyzed, 2);
    let exact = records
        .members
        .iter()
        .find(|m| matches!(&m.entry,PlanEntry::Function{request,..} if request==&f.request))
        .unwrap();
    assert!(
        matches!(&exact.outcome,InvestigationOutcome::Analyzed{analysis,..} if Some(analysis)==single.analysis.as_ref())
    );
    let (_, found) = read(
        &f,
        &id,
        InvestigationFilter::Accesses {
            address: Some(0x60000124),
            symbol: None,
            unknown_only: false,
        },
    );
    assert_eq!(found.findings.len(), 2);
    assert_ne!(
        found.findings[0].request.symbol,
        found.findings[1].request.symbol
    );
    assert!(
        found
            .findings
            .iter()
            .all(|f| matches!(f.record, FunctionRecord::MemoryAccess { offset: 12, .. }))
    );
    let (_, unknown) = read(
        &f,
        &id,
        InvestigationFilter::Accesses {
            address: None,
            symbol: None,
            unknown_only: true,
        },
    );
    assert_eq!(unknown.findings.len(), 2);
    assert!(matches!(
        f.app
            .query(&f.project, app::ReadQuery::InvestigationStatus, budget())
            .unwrap()
            .summary(),
        app::QuerySummary::InvestigationStatus {
            status: InvestigationStatus { current: true, .. }
        }
    ));
    f.app.shutdown();
    let moved = f.dir.path().join("moved");
    fs::rename(&f.project, &moved).unwrap();
    f.project = moved;
    f.app = application(&f.dir.path().join("runtime2"));
    assert_eq!(read(&f, &id, InvestigationFilter::Members).0, manifest);
    assert!(matches!(
        f.app
            .query(&f.project, app::ReadQuery::Doctor, budget())
            .unwrap()
            .summary(),
        app::QuerySummary::Doctor {
            checked_publications: 1,
            errors: 0,
            ..
        }
    ));
}
#[test]
fn zero_size_functions_remain_blocked_unless_exact_occurrence_has_extent() {
    let f = fixture(object(BRANCH, 0, false), false);
    let p = plan(
        &f,
        InvestigationRequest {
            extents: vec![FunctionExtent {
                source: FunctionSource::Input { input: 0 },
                symbol: f.request.symbol.clone(),
                extent: CodeRange {
                    start: 0,
                    length: BRANCH.len() as u64,
                },
            }],
            ..Default::default()
        },
    );
    let run = publish(&f, &p);
    assert_eq!(run.state, RunState::Completed, "{:?}", run.error);
    assert_eq!(run.complete, Some(false));
    let (m, r) = read(&f, &run.publication.unwrap(), InvestigationFilter::Members);
    assert_eq!(
        (
            m.coverage.functions,
            m.coverage.analyzed,
            m.coverage.blocked
        ),
        (2, 1, 1)
    );
    assert!(r.members.iter().any(|m|matches!(&m.outcome,InvestigationOutcome::Blocked{error} if error.code==ErrorCode::NeedsExtent)));
}
#[test]
fn stale_publication_is_preserved_and_old_plan_cannot_replace_current_publication() {
    let f = fixture(object(BRANCH, BRANCH.len() as u64, false), false);
    let old = plan(&f, InvestigationRequest::default());
    let first = publish(&f, &old).publication.unwrap();
    let source = f.dir.path().join("new.o");
    fs::write(&source, object(&[1, 0, 0x82, 0x80], 4, false)).unwrap();
    f.app
        .import(
            &f.project,
            vec![app::ImportInput {
                role: "new".into(),
                path: source,
                expected: None,
            }],
            Target::Riscv32Ilp32,
            budget(),
        )
        .unwrap();
    let status = f
        .app
        .query(&f.project, app::ReadQuery::InvestigationStatus, budget())
        .unwrap();
    assert!(
        matches!(status.summary(),app::QuerySummary::InvestigationStatus{status:InvestigationStatus{current:false,publication:Some(id),..}} if id==&first)
    );
    drop(status);
    let new = plan(&f, InvestigationRequest::default());
    let second = publish(&f, &new).publication.unwrap();
    assert_eq!(publish(&f, &old).publication, Some(first.clone()));
    let status = f
        .app
        .query(&f.project, app::ReadQuery::InvestigationStatus, budget())
        .unwrap();
    assert!(
        matches!(status.summary(),app::QuerySummary::InvestigationStatus{status:InvestigationStatus{current:true,publication:Some(id),..}} if id==&second)
    );
    read(&f, &first, InvestigationFilter::Members);
}
#[test]
fn exhaustion_cancel_and_changed_plan_never_publish_children() {
    let f = fixture(object(BRANCH, BRANCH.len() as u64, false), false);
    let p = plan(&f, InvestigationRequest::default());
    let mut low = budget();
    low.working_memory_bytes = Some(1024 * 1024);
    let run = f
        .app
        .start_analyze_project(&f.project, &p, low)
        .unwrap()
        .wait();
    assert_eq!(run.state, RunState::ResourceLimited);
    let mut low = budget();
    low.max_work_units = Some(1);
    let run = f
        .app
        .start_analyze_project(&f.project, &p, low)
        .unwrap()
        .wait();
    assert_eq!(run.state, RunState::ResourceLimited);
    let handle = f
        .app
        .start_analyze_project(&f.project, &p, budget())
        .unwrap();
    handle.cancel();
    assert_eq!(handle.wait().state, RunState::Cancelled);
    let mut changed = p.clone();
    changed.recipe.functions += 1;
    assert!(
        f.app
            .start_analyze_project(&f.project, &changed, budget())
            .is_err()
    );
    assert!(matches!(
        f.app
            .query(&f.project, app::ReadQuery::Publications, budget())
            .unwrap()
            .summary(),
        app::QuerySummary::Publications { count: 0 }
    ));
    assert!(matches!(
        f.app
            .query(&f.project, app::ReadQuery::Analyses, budget())
            .unwrap()
            .summary(),
        app::QuerySummary::Analyses { count: 0 }
    ));
    assert_eq!(publish(&f, &p).state, RunState::Completed);
}
#[test]
fn gaps_for_missing_thin_members_unsupported_objects_and_stripped_code_are_explicit() {
    let f = fixture(object(BRANCH, BRANCH.len() as u64, false), false);
    let thin = f.dir.path().join("missing.a");
    fs::write(&thin, support::archive(&[(b"absent.o", &[0u8; 10])], true)).unwrap();
    let bad = f.dir.path().join("bad.o");
    fs::write(&bad, b"not an ELF").unwrap();
    let mut stripped = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let section = stripped.add_section(Vec::new(), b".text".to_vec(), SectionKind::Text);
    stripped.append_section_data(section, BRANCH, 2);
    let path = f.dir.path().join("stripped.o");
    fs::write(&path, stripped.write().unwrap()).unwrap();
    f.app
        .import(
            &f.project,
            vec![thin, bad, path]
                .into_iter()
                .map(|path| app::ImportInput {
                    role: "input".into(),
                    path,
                    expected: None,
                })
                .collect(),
            Target::Riscv32Ilp32,
            budget(),
        )
        .unwrap();
    let p = plan(&f, InvestigationRequest::default());
    let run = publish(&f, &p);
    assert_eq!(run.state, RunState::Completed, "{:?}", run.error);
    assert_eq!(run.complete, Some(false));
    let (m, r) = read(&f, &run.publication.unwrap(), InvestigationFilter::Members);
    assert_eq!(m.coverage.inputs, 3);
    assert!(m.coverage.gaps >= 3);
    assert!(r.members.iter().any(
        |m| matches!(&m.entry,PlanEntry::Gap{reason,..} if reason.contains("no defined static"))
    ));
}
#[test]
fn aliases_are_separate_and_reference_lookup_retains_external_identity() {
    let mut obj = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let section = obj.add_section(Vec::new(), b".text".to_vec(), SectionKind::Text);
    obj.append_section_data(section, BRANCH, 2);
    for name in [b"entry".as_slice(), b"alias"] {
        obj.add_symbol(symbol(
            name,
            SymbolSection::Section(section),
            BRANCH.len() as u64,
            SymbolKind::Text,
        ));
    }
    let f = fixture(obj.write().unwrap(), false);
    let p = plan(&f, InvestigationRequest::default());
    assert_eq!(p.recipe.functions, 4);
    let run = publish(&f, &p);
    assert_eq!(run.state, RunState::Completed, "{:?}", run.error);
    let code = words(&[0x00000097, 0x000080e7, 0x00000537, 0x00050513, 0x00008067]);
    let f = fixture(object(&code, code.len() as u64, true), false);
    let p = plan(&f, InvestigationRequest::default());
    let id = publish(&f, &p).publication.unwrap();
    let (_, all) = read(
        &f,
        &id,
        InvestigationFilter::References {
            symbol: None,
            address: None,
        },
    );
    assert_eq!(all.findings.len(), 6);
    let symbol = match &all.findings[0].record {
        FunctionRecord::Reference { target, .. } => {
            assert_eq!(target.definition, SymbolDefinition::Undefined);
            target.symbol.clone()
        }
        _ => panic!("reference"),
    };
    let (_, selected) = read(
        &f,
        &id,
        InvestigationFilter::References {
            address: None,
            symbol: Some(symbol),
        },
    );
    assert_eq!(selected.findings.len(), 1);
}
#[test]
fn plan_rejects_unused_selections_and_extent_overrides() {
    let f = fixture(object(BRANCH, BRANCH.len() as u64, false), false);
    let decoder = blobray_backend_riscv::RiscvDecoder;
    for request in [
        InvestigationRequest {
            inputs: Some(vec![]),
            ..Default::default()
        },
        InvestigationRequest {
            inputs: Some(vec![9]),
            ..Default::default()
        },
        InvestigationRequest {
            inputs: Some(vec![0, 0]),
            ..Default::default()
        },
        InvestigationRequest {
            extents: vec![FunctionExtent {
                source: FunctionSource::Input { input: 9 },
                symbol: f.request.symbol.clone(),
                extent: CodeRange {
                    start: 0,
                    length: 2,
                },
            }],
            ..Default::default()
        },
    ] {
        let error = f
            .app
            .query(
                &f.project,
                app::ReadQuery::PlanInvestigation {
                    request,
                    producer: FunctionProducer {
                        decoder: decoder.identity().into(),
                        semantics: decoder.semantic_identity().into(),
                    },
                },
                budget(),
            )
            .err()
            .unwrap();
        assert_eq!(error.code, ErrorCode::InvalidRequest);
    }
}

#[test]
fn killed_library_coordinator_leaves_no_publication_and_recovery_never_promotes_staging() {
    use std::{
        process::Stdio,
        time::{Duration, Instant},
    };
    let code = [1, 0].repeat(262144);
    let f = fixture(object(&code, code.len() as u64, false), false);
    let plan = plan(&f, InvestigationRequest::default());
    let request = f.dir.path().join("plan.json");
    fs::write(&request, serde_json::to_vec(&plan).unwrap()).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_blobray-next"))
        .args(["analyze-project", "--project"])
        .arg(&f.project)
        .arg("--plan")
        .arg(request)
        .args(["--limit-mode", "watchdog"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(runs) = app::runs(&f.project)
            && runs.iter().any(|r| {
                matches!(r.operation, app::RunOperation::Investigate { .. })
                    && r.state == RunState::Running
            })
        {
            break;
        }
        assert!(Instant::now() < deadline, "worker did not start");
        std::thread::sleep(Duration::from_millis(5));
    }
    child.kill().unwrap();
    child.wait().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let recovered = loop {
        match f.app.recover(&f.project) {
            Ok(r) => break r,
            Err(e) if e.code == ErrorCode::Busy && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10))
            }
            Err(e) => panic!("{e}"),
        }
    };
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].state, RunState::Abandoned);
    assert!(recovered[0].publication.is_none());
    assert!(matches!(
        f.app
            .query(&f.project, app::ReadQuery::Publications, budget())
            .unwrap()
            .summary(),
        app::QuerySummary::Publications { count: 0 }
    ));
    assert!(matches!(
        f.app
            .query(&f.project, app::ReadQuery::Analyses, budget())
            .unwrap()
            .summary(),
        app::QuerySummary::Analyses { count: 0 }
    ));
    assert!(f.app.recover(&f.project).unwrap().is_empty());
    assert_eq!(
        app::inventory(&f.project, None).unwrap().revision_id,
        f.revision
    );
}

#[test]
fn whole_run_budget_failure_after_analysis_keeps_previous_publication() {
    let f = fixture(object(BRANCH, BRANCH.len() as u64, false), false);
    let p = plan(&f, InvestigationRequest::default());
    let first = publish(&f, &p);
    assert_eq!(first.state, RunState::Completed);
    // Repeating the identical recipe exercises all functions and retention with
    // a deterministic total work budget, not a fresh per-function allowance.
    let full = publish(&f, &p);
    let used = full.diagnostics.unwrap().progress.unwrap().work_used;
    let mut limited = budget();
    limited.max_work_units = Some(used - 1);
    let failed = f
        .app
        .start_analyze_project(&f.project, &p, limited)
        .unwrap()
        .wait();
    assert_eq!(
        failed.state,
        RunState::ResourceLimited,
        "{:?}",
        failed.error
    );
    assert_eq!(
        failed.diagnostics.unwrap().progress.unwrap().position.phase,
        RunPhase::Publish
    );
    assert!(failed.publication.is_none());
    let status = f
        .app
        .query(&f.project, app::ReadQuery::InvestigationStatus, budget())
        .unwrap();
    assert!(
        matches!(status.summary(),app::QuerySummary::InvestigationStatus{status:InvestigationStatus{publication,..}} if publication==&first.publication)
    );
    read(
        &f,
        &first.publication.unwrap(),
        InvestigationFilter::Members,
    );
}
#[test]
fn library_staging_quota_and_deadline_fail_without_visible_children() {
    let code = [1, 0].repeat(8192);
    let f = fixture(object(&code, code.len() as u64, false), false);
    let p = plan(&f, InvestigationRequest::default());
    let limited = app::Application::with_temporary_storage(
        Arc::new(LinuxHost::new(
            env!("CARGO_BIN_EXE_blobray-next").into(),
            None,
        )),
        app::ApplicationLimits::default(),
        app::TemporaryStoragePolicy {
            root: Some(f.dir.path().join("limited")),
            operation_bytes: 2 * TEMPORARY_CONTROL_BYTES,
            total_bytes: 4 * TEMPORARY_CONTROL_BYTES,
        },
    )
    .unwrap();
    let run = limited
        .start_analyze_project(&f.project, &p, budget())
        .unwrap()
        .wait();
    assert_eq!(run.state, RunState::ResourceLimited, "{:?}", run.error);
    assert!(run.error.unwrap().storage.is_some());
    let mut short = budget();
    short.timeout_ms = 1;
    let run = f
        .app
        .start_analyze_project(&f.project, &p, short)
        .unwrap()
        .wait();
    assert_eq!(run.state, RunState::TimedOut, "{:?}", run.error);
    assert!(matches!(
        f.app
            .query(&f.project, app::ReadQuery::Analyses, budget())
            .unwrap()
            .summary(),
        app::QuerySummary::Analyses { count: 0 }
    ));
    assert!(matches!(
        f.app
            .query(&f.project, app::ReadQuery::Publications, budget())
            .unwrap()
            .summary(),
        app::QuerySummary::Publications { count: 0 }
    ));
}

#[test]
fn selected_inputs_define_coverage_without_hiding_unselected_scope() {
    let f = fixture(object(BRANCH, BRANCH.len() as u64, false), false);
    let valid = f.dir.path().join("entry.o");
    let missing = f.dir.path().join("absent.o");
    f.app
        .import(
            &f.project,
            vec![missing, valid]
                .into_iter()
                .map(|path| app::ImportInput {
                    role: "code".into(),
                    path,
                    expected: None,
                })
                .collect(),
            Target::Riscv32Ilp32,
            budget(),
        )
        .unwrap();
    let p = plan(
        &f,
        InvestigationRequest {
            inputs: Some(vec![1]),
            ..Default::default()
        },
    );
    let run = publish(&f, &p);
    assert_eq!(run.state, RunState::Completed, "{:?}", run.error);
    assert_eq!(run.complete, Some(true));
    let (manifest, records) = read(&f, &run.publication.unwrap(), InvestigationFilter::Members);
    assert_eq!(manifest.plan.recipe.request.inputs, Some(vec![1]));
    assert_eq!(manifest.coverage.inputs, 1);
    assert!(records.members.iter().all(|m| match &m.entry {
        PlanEntry::Function { request, .. } => request.source.input() == Some(1),
        PlanEntry::Image { .. } | PlanEntry::ImageGap { .. } => false,
        PlanEntry::Input { input, .. }
        | PlanEntry::Object { input, .. }
        | PlanEntry::Gap { input, .. } => *input == 1,
    }));
}
#[test]
fn missing_child_is_integrity_failure_and_reads_never_recompute() {
    let f = fixture(object(BRANCH, BRANCH.len() as u64, false), false);
    let p = plan(&f, InvestigationRequest::default());
    let id = publish(&f, &p).publication.unwrap();
    let (_, members) = read(&f, &id, InvestigationFilter::Members);
    let child = members
        .members
        .iter()
        .find_map(|m| {
            if let InvestigationOutcome::Analyzed { analysis, .. } = &m.outcome {
                Some(analysis.clone())
            } else {
                None
            }
        })
        .unwrap();
    let output = f
        .app
        .query(
            &f.project,
            app::ReadQuery::Analysis {
                id: child,
                export: false,
            },
            budget(),
        )
        .unwrap();
    let app::QuerySummary::Analysis { manifest, .. } = output.summary() else {
        panic!("analysis");
    };
    let records = f
        .project
        .join(".blobray-next/objects")
        .join(manifest.records.as_str());
    drop(output);
    fs::remove_file(&records).unwrap();
    let before = app::runs(&f.project).unwrap();
    assert!(
        f.app
            .query(
                &f.project,
                app::ReadQuery::Publication {
                    id,
                    filter: InvestigationFilter::Members
                },
                budget()
            )
            .is_err()
    );
    assert!(!records.exists());
    assert_eq!(app::runs(&f.project).unwrap(), before);
    assert!(
        matches!(f.app.query(&f.project,app::ReadQuery::Doctor,budget()).unwrap().summary(),app::QuerySummary::Doctor{errors,..} if *errors>0)
    );
}
