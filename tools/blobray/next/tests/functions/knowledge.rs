use super::*;
#[derive(Default)]
struct KnowledgeRecords {
    entries: Vec<KnowledgeEntry>,
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
    fn summary(&mut self, _: &app::QuerySummary, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
}
fn records(f: &Fixture, project: &Path, at: Option<KnowledgeRevisionId>) -> KnowledgeRecords {
    let handle = f
        .app
        .start_query(
            project,
            app::ReadQuery::Knowledge { revision: at },
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
fn missing_evidence_wrong_occurrence_and_exhausted_budget_cannot_publish() {
    let f = fixture(object(BRANCH, BRANCH.len() as u64, false), false);
    let mut request = change(&f, None, "name");
    if let KnowledgeAction::Propose { proposal } = &mut request.action {
        proposal.occurrence.source = FunctionSource::Input { input: 99 };
    }
    assert_eq!(apply(&f, &request).error.unwrap().code, ErrorCode::NotFound);
    assert!(records(&f, &f.project, None).entries.is_empty());
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
    assert!(records(&f, &f.project, None).entries.is_empty());
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
    let id = records(&f, &f.project, None).entries[0].id.clone();
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
                        decoder: blobray_backend_riscv::RiscvDecoder.identity().into(),
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
