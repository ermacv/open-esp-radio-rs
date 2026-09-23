use super::*;
#[derive(Default)]
struct KnowledgeRecords {
    entries: Vec<KnowledgeEntry>,
    events: Vec<KnowledgeEvent>,
}
impl ElfSink for KnowledgeRecords {
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
impl InventorySink for KnowledgeRecords {}
impl app::DoctorSink for KnowledgeRecords {
    fn error(&mut self, e: &Error, _: &mut dyn RunControl) -> Result<()> {
        Err(e.clone())
    }
    fn unfinished(&mut self, _: &RunId, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
}
impl app::QuerySink for KnowledgeRecords {
    fn knowledge_entry(&mut self, e: &KnowledgeEntry, _: &mut dyn RunControl) -> Result<()> {
        self.entries.push(e.clone());
        Ok(())
    }
    fn knowledge_event(&mut self, e: &KnowledgeEvent, _: &mut dyn RunControl) -> Result<()> {
        self.events.push(e.clone());
        Ok(())
    }
    fn summary(&mut self, _: &app::QuerySummary, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
}
fn records(
    f: &Fixture,
    project: &Path,
    at: Option<KnowledgeRevisionId>,
    history: bool,
) -> KnowledgeRecords {
    let handle = f
        .app
        .start_query(
            project,
            app::ReadQuery::Knowledge {
                revision: at,
                history,
            },
            budget(),
        )
        .unwrap();
    let record = handle.wait();
    assert_eq!(record.state, RunState::Completed, "{record:?}");
    let mut output = KnowledgeRecords::default();
    handle
        .take_output()
        .unwrap()
        .records(&|| false, &mut output)
        .unwrap();
    output
}
fn change(f: &Fixture, base: Option<KnowledgeRevisionId>, name: &str) -> KnowledgeChange {
    let analysis = f
        .app
        .start_analyze_function(&f.project, f.request.clone(), budget())
        .unwrap()
        .wait();
    assert_eq!(analysis.state, RunState::Completed, "{analysis:?}");
    KnowledgeChange {
        expected_base: base,
        actor: "reviewer".into(),
        reason: "reviewed captured function".into(),
        action: KnowledgeAction::Propose {
            proposal: KnowledgeProposal {
                subject: "function.entry".to_owned().try_into().unwrap(),
                occurrence: KnowledgeOccurrence {
                    revision: f.revision.clone(),
                    source: FunctionSource::Input {
                        input: f.request.source.input().unwrap(),
                    },
                    object: f.request.selector.object().clone(),
                    symbol: Some(f.request.selector.symbol().unwrap().clone()),
                },
                claim: KnowledgeClaim::Name { name: name.into() },
                evidence: vec![EvidenceRef::Analysis {
                    analysis: analysis.analysis.unwrap(),
                    record: Some(0),
                }],
                note: None,
            },
        },
    }
}
fn apply(f: &Fixture, change: &KnowledgeChange) -> app::RunRecord {
    f.app
        .start_knowledge(&f.project, change, budget())
        .unwrap()
        .wait()
}
#[test]
fn review_history_conflicts_supersession_and_backup_survive_source_removal() {
    let f = fixture(object(BRANCH, BRANCH.len() as u64, false), false);
    let first = change(&f, None, "old");
    let validated = f
        .app
        .start_query(
            &f.project,
            app::ReadQuery::ValidateKnowledge {
                change: first.clone(),
            },
            budget(),
        )
        .unwrap();
    let result = validated.wait();
    assert_eq!(result.state, RunState::Completed, "{result:?}");
    assert!(matches!(
        validated.take_output().unwrap().summary(),
        app::QuerySummary::KnowledgeValidation {
            expected_base: None
        }
    ));
    assert!(records(&f, &f.project, None, false).entries.is_empty());
    let proposed = apply(&f, &first);
    assert_eq!(proposed.state, RunState::Completed, "{proposed:?}");
    let base = proposed.knowledge.unwrap();
    let entries = records(&f, &f.project, None, false).entries;
    let assertion = entries[0].id.clone();
    let accept = KnowledgeChange {
        expected_base: Some(base.clone()),
        actor: "reviewer".into(),
        reason: "checked byte evidence".into(),
        action: KnowledgeAction::Review {
            assertion: assertion.clone(),
            decision: ReviewDecision::Accept,
            supersedes: None,
        },
    };
    let accepted = apply(&f, &accept);
    assert_eq!(accepted.state, RunState::Completed, "{accepted:?}");
    assert_eq!(
        records(&f, &f.project, Some(base), false).entries[0].state,
        AssertionState::Proposed
    );
    assert_eq!(
        records(&f, &f.project, None, false).entries[0].state,
        AssertionState::Accepted
    );
    assert_eq!(
        f.app
            .start_knowledge(&f.project, &accept, budget())
            .err()
            .unwrap()
            .code,
        ErrorCode::Conflict
    );
    let next = apply(&f, &change(&f, accepted.knowledge, "new"));
    let pending = records(&f, &f.project, None, false).entries[1].id.clone();
    let mut review = KnowledgeChange {
        expected_base: next.knowledge,
        actor: "reviewer".into(),
        reason: "corrected name".into(),
        action: KnowledgeAction::Review {
            assertion: pending.clone(),
            decision: ReviewDecision::Accept,
            supersedes: None,
        },
    };
    assert_eq!(apply(&f, &review).error.unwrap().code, ErrorCode::Conflict);
    review.action = KnowledgeAction::Review {
        assertion: pending,
        decision: ReviewDecision::Accept,
        supersedes: Some(assertion),
    };
    let final_run = apply(&f, &review);
    assert_eq!(final_run.state, RunState::Completed, "{final_run:?}");
    fs::remove_file(f.dir.path().join("entry.a")).unwrap();
    fs::remove_file(f.dir.path().join("entry.o")).unwrap();
    let backup = f
        .app
        .start_query(&f.project, app::ReadQuery::Backup, budget())
        .unwrap();
    assert_eq!(backup.wait().state, RunState::Completed);
    let path = f.dir.path().join("backup.blobray");
    backup
        .take_output()
        .unwrap()
        .export_backup(&path, &|| false)
        .unwrap();
    let restored = f.dir.path().join("restored");
    let restore = f
        .app
        .start_query(
            &restored,
            app::ReadQuery::Restore {
                bundle: OriginPath::from_path(&path),
            },
            budget(),
        )
        .unwrap();
    let run = restore.wait();
    assert_eq!(run.state, RunState::Completed, "{run:?}");
    restore
        .take_output()
        .unwrap()
        .publish_restore(&restored, &|| false)
        .unwrap();
    assert_eq!(
        records(&f, &f.project, None, false).entries,
        records(&f, &restored, None, false).entries
    );
    assert_eq!(records(&f, &restored, None, true).events.len(), 4);
    let mut damaged = fs::read(&path).unwrap();
    let n = damaged.len() / 2;
    damaged[n] ^= 1;
    fs::write(&path, damaged).unwrap();
    let failed = f
        .app
        .start_query(
            &f.dir.path().join("corrupt"),
            app::ReadQuery::Restore {
                bundle: OriginPath::from_path(&path),
            },
            budget(),
        )
        .unwrap();
    assert_ne!(failed.wait().state, RunState::Completed);
    assert!(!f.dir.path().join("corrupt").exists());
}
#[test]
fn missing_evidence_wrong_occurrence_and_exhausted_budget_cannot_publish() {
    let f = fixture(object(BRANCH, BRANCH.len() as u64, false), false);
    let mut request = change(&f, None, "name");
    if let KnowledgeAction::Propose { proposal } = &mut request.action {
        proposal.occurrence.source = FunctionSource::Input { input: 99 };
    }
    assert_eq!(apply(&f, &request).error.unwrap().code, ErrorCode::NotFound);
    assert!(records(&f, &f.project, None, false).entries.is_empty());
    if let KnowledgeAction::Propose { proposal } = &mut request.action {
        proposal.occurrence.source = f.request.source.clone();
        proposal.evidence = vec![EvidenceRef::Analysis {
            analysis: ArtifactId::of_bytes(b"absent").as_str().parse().unwrap(),
            record: None,
        }];
    }
    assert_eq!(apply(&f, &request).error.unwrap().code, ErrorCode::NotFound);
    let request = change(&f, None, "name");
    let mut tiny = budget();
    tiny.working_memory_bytes = Some(1024 * 1024);
    let run = f
        .app
        .start_knowledge(&f.project, &request, tiny)
        .unwrap()
        .wait();
    assert_eq!(run.state, RunState::ResourceLimited);
    assert!(records(&f, &f.project, None, false).entries.is_empty());
}
#[derive(Default)]
struct LegacyRecords(Vec<LegacyRecord>);
impl ElfSink for LegacyRecords {
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
impl InventorySink for LegacyRecords {}
impl app::DoctorSink for LegacyRecords {
    fn error(&mut self, e: &Error, _: &mut dyn RunControl) -> Result<()> {
        Err(e.clone())
    }
    fn unfinished(&mut self, _: &RunId, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
}
impl app::QuerySink for LegacyRecords {
    fn legacy(&mut self, r: &LegacyRecord, _: &mut dyn RunControl) -> Result<()> {
        self.0.push(r.clone());
        Ok(())
    }
    fn summary(&mut self, _: &app::QuerySummary, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
}
#[test]
fn legacy_preserves_unsupported_and_missing_data_and_converts_only_exact_boundaries() {
    let f = fixture(object(BRANCH, BRANCH.len() as u64, false), false);
    let legacy = f.dir.path().join("legacy");
    fs::create_dir(&legacy).unwrap();
    let payload = object(BRANCH, BRANCH.len() as u64, false);
    let digest = ArtifactId::of_bytes(&payload);
    fs::write(legacy.join("source.o"), &payload).unwrap();
    fs::write(legacy.join("vendor-project.toml"),"schema = 4\nid = 'fixture'\ntarget-spec = 'missing-target.toml'\n[code]\npack = 'boundaries.toml'\n").unwrap();
    fs::write(legacy.join("boundaries.toml"),format!("schema=1\nid='fixture'\n[[boundaries]]\nsource='vendor'\nartifact-sha256='{digest}'\nsection='.text.entry'\nentry-offset=0\nend-exclusive-offset={}\nstatus='accepted'\nreason='reviewed exact bytes'\n",BRANCH.len())).unwrap();
    fs::write(legacy.join("opaque.bin"), b"retained historical evidence").unwrap();
    fs::write(
        legacy.join("disposition.toml"),
        "schema=3\n[[functions]]\nbinding='v2'\n",
    )
    .unwrap();
    std::os::unix::fs::symlink(".", legacy.join("loop")).unwrap();
    let request = app::LegacyRequest {
        manifest: OriginPath::from_path(&legacy.join("vendor-project.toml")),
        run_spec: None,
        roots: vec![],
        inputs: vec![app::ImportBinding {
            role: "vendor".into(),
            origin: OriginPath::from_path(&legacy.join("source.o")),
            expected: Some(digest),
        }],
        target: Target::Riscv32Ilp32,
    };
    let destination = f.dir.path().join("migrated");
    let handle = f
        .app
        .start_query(
            &destination,
            app::ReadQuery::ImportLegacy { request },
            budget(),
        )
        .unwrap();
    let run = handle.wait();
    assert_eq!(run.state, RunState::Completed, "{run:?}");
    handle
        .take_output()
        .unwrap()
        .publish_restore(&destination, &|| false)
        .unwrap();
    let query = f
        .app
        .start_query(&destination, app::ReadQuery::Legacy, budget())
        .unwrap();
    let run = query.wait();
    assert_eq!(run.state, RunState::Completed, "{run:?}");
    let mut data = LegacyRecords::default();
    query
        .take_output()
        .unwrap()
        .records(&|| false, &mut data)
        .unwrap();
    assert!(
        data.0
            .iter()
            .any(|r| matches!(r.outcome, LegacyOutcome::MissingPayload { .. }))
    );
    assert!(
        data.0
            .iter()
            .any(|r| matches!(r.outcome, LegacyOutcome::Unsupported { .. })
                && matches!(r.capture, Capture::Captured { .. }))
    );
    assert!(
        data.0
            .iter()
            .any(|r| matches!(r.outcome, LegacyOutcome::Converted { .. })),
        "{:?}",
        data.0
    );
    assert!(
        !data
            .0
            .iter()
            .any(|r| r.origin.to_path().unwrap().ends_with("v2")),
        "binding version is not a filesystem dependency"
    );
    assert!(data.0.iter().any(|r| r.selector == "symlink-target"));
    let entries = records(&f, &destination, None, false).entries;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].state, AssertionState::Accepted);
    let payload = data
        .0
        .iter()
        .find(|r| r.selector == "file" && r.origin.to_path().unwrap().ends_with("opaque.bin"))
        .unwrap()
        .capture
        .artifact()
        .unwrap()
        .clone();
    let query = f
        .app
        .start_query(
            &destination,
            app::ReadQuery::RetainedPayload { id: payload },
            budget(),
        )
        .unwrap();
    let result = query.wait();
    assert_eq!(result.state, RunState::Completed, "{result:?}");
    let exported = f.dir.path().join("opaque-export");
    query
        .take_output()
        .unwrap()
        .export_payload(&exported, &|| false)
        .unwrap();
    assert_eq!(fs::read(exported).unwrap(), b"retained historical evidence");
    fs::remove_dir_all(legacy).unwrap();
    assert_eq!(records(&f, &destination, None, false).entries, entries);
}

#[test]
fn reviewed_extent_is_selected_explicitly_and_names_do_not_change_function_computation() {
    let mut f = fixture(object(BRANCH, 0, false), false);
    f.request.extent = Some(CodeRange {
        start: 0,
        length: BRANCH.len() as u64,
    });
    let mut proposal = change(&f, None, "unused-name");
    if let KnowledgeAction::Propose { proposal } = &mut proposal.action {
        proposal.claim = KnowledgeClaim::FunctionExtent {
            extent: f.request.extent.unwrap(),
        };
    }
    let proposed = apply(&f, &proposal);
    assert_eq!(proposed.state, RunState::Completed, "{proposed:?}");
    let id = records(&f, &f.project, None, false).entries[0].id.clone();
    let query = |revision: KnowledgeRevisionId| {
        f.app
            .start_query(
                &f.project,
                app::ReadQuery::PlanInvestigation {
                    request: InvestigationRequest {
                        reviewed_extents: vec![ReviewedExtent {
                            revision,
                            assertion: id.clone(),
                        }],
                        ..Default::default()
                    },
                    producer: FunctionProducer {
                        decoder: "rv32imac/rv-asm-0.2.1/policy-1".into(),
                        semantics: blobray_backend_riscv::RiscvDecoder
                            .semantic_identity()
                            .into(),
                    },
                },
                budget(),
            )
            .unwrap()
    };
    let pending = query(proposed.knowledge.clone().unwrap());
    assert_eq!(
        pending.wait().error.unwrap().code,
        ErrorCode::InvalidRequest
    );
    let accepted = apply(
        &f,
        &KnowledgeChange {
            expected_base: proposed.knowledge,
            actor: "reviewer".into(),
            reason: "extent checked".into(),
            action: KnowledgeAction::Review {
                assertion: id.clone(),
                decision: ReviewDecision::Accept,
                supersedes: None,
            },
        },
    );
    assert_eq!(accepted.state, RunState::Completed, "{accepted:?}");
    let handle = query(accepted.knowledge.clone().unwrap());
    let run = handle.wait();
    assert_eq!(run.state, RunState::Completed, "{run:?}");
    let output = handle.take_output().unwrap();
    let app::QuerySummary::InvestigationPlan { plan } = output.summary() else {
        panic!("wrong result")
    };
    let plan = plan.clone();
    drop(output);
    let name = change(&f, accepted.knowledge, "new-display-name");
    let renamed = apply(&f, &name);
    assert_eq!(renamed.state, RunState::Completed);
    let handle = query(plan.recipe.request.reviewed_extents[0].revision.clone());
    assert_eq!(handle.wait().state, RunState::Completed);
    let output = handle.take_output().unwrap();
    let app::QuerySummary::InvestigationPlan { plan: again } = output.summary() else {
        panic!("wrong result")
    };
    assert_eq!(again, &plan);
    let analyzed = f
        .app
        .start_analyze_project(
            &f.project,
            blobray_domain::InvestigationInput::Plan {
                plan: (*plan).clone(),
            },
            budget(),
        )
        .unwrap()
        .wait();
    assert_eq!(analyzed.state, RunState::Completed, "{analyzed:?}");
}
