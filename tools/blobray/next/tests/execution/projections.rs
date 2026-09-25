use super::interfaces::run;
use super::*;
fn setup() -> (Fixture, ExecutionRequest, LayoutProjection) {
    let code = [
        0x00b52023, 0x00052283, 0x00060463, 0x00158593, 0x00000513, 0x00008067,
    ];
    setup_code(&code, &[&[0x00000013][..], &code].concat())
}
fn setup_code(left: &[u32], right: &[u32]) -> (Fixture, ExecutionRequest, LayoutProjection) {
    let (a, pa) = super::goals::symbol_elf(left, 0x1000, 0x1000);
    let (b, mut pb) = super::goals::symbol_elf(right, 0x1000, 0x1000);
    pb.source = FunctionSource::Input { input: 1 };
    let f = Fixture::from_inputs(vec![a, b]);
    let endpoint = |point: ExecutionSymbol, address, length| LayoutEndpoint {
        entry: CallEndpoint {
            occurrence: KnowledgeOccurrence {
                revision: f.target.revision.clone(),
                source: point.source,
                object: point.symbol.object.clone(),
                symbol: Some(point.symbol),
            },
            boundary: ReviewedCallBoundary::Code { address: 0x1000 },
        },
        domains: vec![LayoutDomain { address, length }],
    };
    let p = LayoutProjection {
        vendor: endpoint(pa, 0x3000, 16),
        replacement: endpoint(pb, 0x5000, 24),
        fields: vec![LayoutField {
            count: 1,
            name: "counter".into(),
            vendor: FieldLocation {
                domain: 0,
                offset: 0,
            },
            replacement: FieldLocation {
                domain: 0,
                offset: 8,
            },
            width: 4,
            final_state: true,
            timeline: true,
        }],
        branches: vec![BranchPair {
            vendor: BranchLocation {
                site: 0x1008,
                target: 0x1010,
                fallthrough: 0x100c,
            },
            replacement: BranchLocation {
                site: 0x100c,
                target: 0x1014,
                fallthrough: 0x1010,
            },
        }],
        applicability: "synthetic selected entry layouts".into(),
        reason: "reviewed same field and conditional decision".into(),
    };
    let mut r = f.request();
    r.replacement.as_mut().unwrap().source = FunctionSource::Input { input: 1 };
    r.max_events = 128;
    for side in [false, true] {
        let input = if side {
            r.cases[0].replacement.as_mut().unwrap()
        } else {
            &mut r.cases[0].vendor
        };
        let domain = p.endpoint(side).domains[0];
        input.arguments = vec![Some(if side { 0x5008 } else { 0x3000 }), Some(7), Some(0)];
        input.memory = vec![ExecutionRegion {
            lifetime: RegionLifetime::Session,
            seed: MemorySeed {
                address: domain.address,
                length: domain.length,
                fill: None,
                bytes: vec![],
            },
        }];
        input.observe_memory = vec![MemorySelection {
            name: "layout including unknown padding".into(),
            address: domain.address,
            length: domain.length,
        }];
        input.observe_timeline = TimelineCapture {
            reads: true,
            writes: true,
            atomics: false,
            branches: true,
        };
    }
    let capture = r.cases[0].vendor.observe_timeline;
    let relation = r.cases[0].relation.as_mut().unwrap();
    relation.returns = ReturnWords {
        low: false,
        high: false,
    };
    relation.events = EventChannels {
        timeline: capture,
        mmio_read: false,
        mmio_write: false,
        fence: false,
        delay: false,
    };
    (f, r, p)
}
fn proposal(p: &LayoutProjection) -> KnowledgeProposal {
    KnowledgeProposal {
        subject: "fixture.layout".to_string().try_into().unwrap(),
        occurrence: p.vendor.entry.occurrence.clone(),
        claim: KnowledgeClaim::LayoutProjection {
            projection: Box::new(p.clone()),
        },
        evidence: vec![EvidenceRef::Source {
            payload: p.vendor.entry.occurrence.object.artifact.clone(),
            range: CodeRange {
                start: 0,
                length: 4,
            },
        }],
        note: None,
    }
}
fn propose(f: &Fixture, p: &LayoutProjection, specialized: bool) -> ProjectionReview {
    let base = None;
    let handle = if specialized {
        f.app.start_propose_projection(
            &f.project,
            ProjectionProposalRequest {
                subject: proposal(p).subject,
                projection: p.clone(),
                expected_base: base,
                actor: "fixture".into(),
                reason: "proposal".into(),
            },
            budget(),
        )
    } else {
        f.app.start_knowledge(
            &f.project,
            &KnowledgeChange {
                expected_base: base,
                actor: "fixture".into(),
                reason: "proposal".into(),
                action: KnowledgeAction::Propose {
                    proposal: proposal(p),
                },
            },
            budget(),
        )
    }
    .unwrap();
    let id = super::call_pairs::proposed_id(f, handle.wait());
    ProjectionReview {
        knowledge: id.knowledge,
        assertion: id.assertion,
    }
}
fn review(f: &Fixture, p: ProjectionReview, decision: ReviewDecision) -> ProjectionReview {
    let result = f
        .app
        .start_knowledge(
            &f.project,
            &KnowledgeChange {
                expected_base: Some(p.knowledge),
                actor: "fixture".into(),
                reason: "synthetic decision".into(),
                action: KnowledgeAction::Review {
                    assertion: p.assertion.clone(),
                    decision,
                    supersedes: None,
                },
            },
            budget(),
        )
        .unwrap()
        .wait();
    assert_eq!(result.state, RunState::Completed, "{result:?}");
    ProjectionReview {
        knowledge: result.knowledge.unwrap(),
        assertion: p.assertion,
    }
}
fn select(r: &mut ExecutionRequest, p: ProjectionReview) {
    r.cases[0].relation.as_mut().unwrap().projection = Some(ProjectionRef::Reviewed(p));
}
#[test]
fn reviewed_layout_rebases_fields_and_branches_without_comparing_unknown_padding() {
    for specialized in [false, true] {
        let (f, mut r, p) = setup();
        assert_eq!(run(&f, r.clone()).0.verdict, Some(ComparisonVerdict::Diff));
        let pending = propose(&f, &p, specialized);
        select(&mut r, pending.clone());
        let failed = f.run(r.clone(), budget());
        assert_eq!(failed.state, RunState::Failed);
        assert!(failed.execution.is_none());
        let accepted = review(&f, pending, ReviewDecision::Accept);
        select(&mut r, accepted.clone());
        let (m, rows) = run(&f, r.clone());
        assert_eq!(m.verdict, Some(ComparisonVerdict::Match));
        assert_eq!(
            m.projections,
            vec![ResolvedProjection {
                review: ProjectionRef::Reviewed(accepted),
                projection: p
            }]
        );
        assert!(
            rows.iter().any(
                |r| matches!(r,ExecutionEvidence::FinalMemory {chunk,..} if !chunk.complete())
            )
        );
        assert!(rows.iter().any(|r| matches!(
            r,
            ExecutionEvidence::Event {
                event: ExecutionEvent::Memory {
                    transaction: MemoryTransaction::Write {
                        address: 0x5008,
                        ..
                    },
                    ..
                },
                ..
            }
        )));
        super::comparison::check_preservation(&f, r.clone());
        r.cases[0].replacement.as_mut().unwrap().arguments[1] = Some(9);
        assert_eq!(run(&f, r).0.verdict, Some(ComparisonVerdict::Diff));
    }
}
#[test]
fn projected_final_fields_keep_unknowns_and_do_not_imply_timeline_equality() {
    let (f, mut r, mut p) = setup();
    p.fields[0].timeline = false;
    p.branches.clear();
    r.cases[0].relation.as_mut().unwrap().events.timeline = TimelineCapture::default();
    let accepted = review(&f, propose(&f, &p, true), ReviewDecision::Accept);
    select(&mut r, accepted);
    assert_eq!(run(&f, r.clone()).0.verdict, Some(ComparisonVerdict::Match));
    r.cases[0].replacement.as_mut().unwrap().arguments[1] = Some(9);
    let (m, rows) = run(&f, r.clone());
    assert_eq!(m.verdict, Some(ComparisonVerdict::Diff));
    assert!(rows.iter().any(|r| matches!(
        r,
        ExecutionEvidence::Comparison {
            result: CaseComparison {
                difference: Some(ComparisonDifference::ProjectedMemory {
                    field: 0,
                    offset: 0,
                    vendor: 7,
                    replacement: 9
                }),
                ..
            },
            ..
        }
    )));
    r.cases[0].replacement.as_mut().unwrap().arguments[1] = None;
    assert_eq!(run(&f, r).0.verdict, Some(ComparisonVerdict::Incomplete));
}
#[test]
fn unmapped_selected_timeline_and_missing_final_capture_cannot_silently_match() {
    let (f, mut r, mut p) = setup();
    p.fields[0].timeline = false;
    p.branches.clear();
    let accepted = review(&f, propose(&f, &p, false), ReviewDecision::Accept);
    select(&mut r, accepted);
    // Capture/selection still requests reads, writes and branches: every unmapped event remains unknown.
    assert_eq!(
        run(&f, r.clone()).0.verdict,
        Some(ComparisonVerdict::Incomplete)
    );
    r.cases[0].replacement.as_mut().unwrap().observe_memory[0].length = 8;
    let failed = f.run(r, budget());
    assert_eq!(failed.state, RunState::Failed);
    assert!(failed.execution.is_none());
}
#[test]
fn invalid_physical_projection_and_layout_geometry_never_publish_knowledge() {
    let (f, _, p) = setup();
    for variant in 0..10 {
        let mut bad = p.clone();
        match variant {
            0 => {
                bad.replacement
                    .entry
                    .occurrence
                    .symbol
                    .as_mut()
                    .unwrap()
                    .index = 999
            }
            1 => bad.replacement.entry.boundary = ReviewedCallBoundary::Code { address: 0x1004 },
            2 => bad.branches[0].replacement.site = 0x1000,
            3 => bad.branches[0].replacement.target = 0x1018,
            4 => bad.fields[0].replacement.offset = 24,
            5 => bad.fields[0].width = 3,
            6 => {
                let mut alias = bad.fields[0].clone();
                alias.name = "alias".into();
                bad.fields.push(alias);
            }
            7 => bad.replacement.domains[0].length = u32::MAX,
            8 => bad.fields[0].replacement.domain = 9,
            _ => bad.applicability.clear(),
        }
        let before = None;
        let result = f.app.start_knowledge(
            &f.project,
            &KnowledgeChange {
                expected_base: before.clone(),
                actor: "fixture".into(),
                reason: "invalid".into(),
                action: KnowledgeAction::Propose {
                    proposal: proposal(&bad),
                },
            },
            budget(),
        );
        if let Ok(handle) = result {
            let failed = handle.wait();
            assert_eq!(
                failed.state,
                RunState::Failed,
                "variant {variant}: {failed:?}"
            );
            assert!(failed.knowledge.is_none());
        }
    } // A valid proposal at the original empty base proves no failed attempt advanced knowledge.
    propose(&f, &p, false);
}

#[test]
fn projection_cli_review_freezes_applicability_and_conflicts_without_changing_old_results() {
    let (f, mut r, p) = setup();
    let request = ProjectionProposalRequest {
        subject: proposal(&p).subject,
        projection: p.clone(),
        expected_base: None,
        actor: "fixture".into(),
        reason: "CLI proposal".into(),
    };
    let path = f._dir.path().join("projection.json");
    fs::write(&path, serde_json::to_vec(&request).unwrap()).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args(["--format", "json", "knowledge", "--project"])
        .arg(&f.project)
        .args([
            "--limit-mode",
            "watchdog",
            "propose-projection",
            "--request",
        ])
        .arg(path)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let id =
        super::call_pairs::proposed_id(&f, serde_json::from_value(out["run"].clone()).unwrap());
    let accepted = review(
        &f,
        ProjectionReview {
            knowledge: id.knowledge,
            assertion: id.assertion,
        },
        ReviewDecision::Accept,
    );
    select(&mut r, accepted.clone());
    for variant in 0..4 {
        let mut bad = r.clone();
        match variant {
            0 => bad.replacement.as_mut().unwrap().source = FunctionSource::Input { input: 0 },
            1 => bad.cases[0].replacement.as_mut().unwrap().entry += 4,
            2 => {
                bad.cases[0]
                    .relation
                    .as_mut()
                    .unwrap()
                    .projection
                    .as_mut()
                    .and_then(ProjectionRef::review_mut)
                    .unwrap()
                    .assertion = ArtifactId::of_bytes(b"missing assertion")
                    .as_str()
                    .parse()
                    .unwrap()
            }
            _ => {
                bad.cases[0]
                    .relation
                    .as_mut()
                    .unwrap()
                    .projection
                    .as_mut()
                    .and_then(ProjectionRef::review_mut)
                    .unwrap()
                    .knowledge = ArtifactId::of_bytes(b"missing review")
                    .as_str()
                    .parse()
                    .unwrap()
            }
        }
        let failed = f.run(bad, budget());
        assert_eq!(failed.state, RunState::Failed, "{failed:?}");
        assert!(failed.execution.is_none());
    }
    let mut changed = p;
    changed.fields[0].name = "revised interpretation".into();
    let pending = f
        .app
        .start_knowledge(
            &f.project,
            &KnowledgeChange {
                expected_base: Some(accepted.knowledge.clone()),
                actor: "fixture".into(),
                reason: "second interpretation".into(),
                action: KnowledgeAction::Propose {
                    proposal: proposal(&changed),
                },
            },
            budget(),
        )
        .unwrap()
        .wait();
    let pending = super::call_pairs::proposed_id(&f, pending);
    let mut change = KnowledgeChange {
        expected_base: Some(pending.knowledge),
        actor: "fixture".into(),
        reason: "review".into(),
        action: KnowledgeAction::Review {
            assertion: pending.assertion,
            supersedes: None,
            decision: ReviewDecision::Accept,
        },
    };
    let conflict = f
        .app
        .start_knowledge(&f.project, &change, budget())
        .unwrap()
        .wait();
    assert_eq!(conflict.error.unwrap().code, ErrorCode::Conflict);
    assert!(conflict.knowledge.is_none());
    if let KnowledgeAction::Review { supersedes, .. } = &mut change.action {
        *supersedes = Some(accepted.assertion.clone());
    }
    let newer = f
        .app
        .start_knowledge(&f.project, &change, budget())
        .unwrap()
        .wait();
    assert_eq!(newer.state, RunState::Completed, "{newer:?}");
    assert_eq!(run(&f, r.clone()).0.verdict, Some(ComparisonVerdict::Match));
    super::comparison::check_preservation(&f, r.clone());
    r.cases[0]
        .relation
        .as_mut()
        .unwrap()
        .projection
        .as_mut()
        .and_then(ProjectionRef::review_mut)
        .unwrap()
        .knowledge = newer.knowledge.unwrap();
    let failed = f.run(r, budget());
    assert_eq!(failed.error.unwrap().code, ErrorCode::InvalidRequest);
    assert!(failed.execution.is_none());
}
#[test]
fn completed_execution_with_unknown_selected_final_fields_is_incomplete() {
    let (f, mut r, mut p) = setup();
    p.fields[0].vendor.offset = 4;
    p.fields[0].replacement.offset = 12;
    p.fields[0].timeline = false;
    p.branches.clear();
    r.cases[0].relation.as_mut().unwrap().events.timeline = TimelineCapture::default();
    select(
        &mut r,
        review(&f, propose(&f, &p, false), ReviewDecision::Accept),
    );
    let (m, rows) = run(&f, r);
    assert!(m.complete);
    assert_eq!(m.verdict, Some(ComparisonVerdict::Incomplete));
    assert_eq!(
        rows.iter()
            .filter(|r| matches!(
                r,
                ExecutionEvidence::Outcome {
                    stop: ExecutionStop::Returned { .. },
                    ..
                }
            ))
            .count(),
        2
    );
}

#[test]
fn projected_arrays_compare_cross_chunk_snapshots_and_bulk_initialization_once() {
    let code = [0x00008413, 0x000022b7, 0x000280e7, 0x00040067];
    let (f, mut r, mut p) = setup_code(&code, &code);
    p.branches.clear();
    p.fields[0].count = 4;
    p.fields[0].replacement.offset = 16;
    p.replacement.domains[0].length = 32;
    r.cases[0].replacement.as_mut().unwrap().observe_memory[0].address = 0x5008;
    for side in [false, true] {
        let input = if side {
            r.cases[0].replacement.as_mut().unwrap()
        } else {
            &mut r.cases[0].vendor
        };
        input.memory.clear();
        input.arguments = vec![Some(16)];
        input.observe_timeline = TimelineCapture {
            writes: true,
            ..Default::default()
        };
        input.calls = vec![CallDeclaration {
            repetition: blobray_domain::CallRepetition::Finite,
            id: "allocate".into(),
            applicability: "test".into(),
            lifetime: RegionLifetime::Phase,
            binding: CallBinding {
                address: 0x2000,
                boundary: CallBoundary::Unmapped,
                allow_tail: false,
            },
            argument_words: 1,
            responses: vec![CallResponse {
                return_words: [Some(if side { 0x5010 } else { 0x3000 }), None],
                outputs: vec![],
                delay_micros: None,
                allocation: Some(CallAllocation {
                    address: if side { 0x5010 } else { 0x3000 },
                    size_argument: 0,
                    capacity: 16,
                    lifetime: RegionLifetime::Session,
                }),
            }],
        }];
    }
    r.cases[0].relation.as_mut().unwrap().events.timeline = TimelineCapture {
        writes: true,
        ..Default::default()
    };
    select(
        &mut r,
        review(&f, propose(&f, &p, true), ReviewDecision::Accept),
    );
    let (m, rows) = run(&f, r.clone());
    assert_eq!(m.verdict, Some(ComparisonVerdict::Match));
    assert_eq!(
        rows.iter()
            .filter(|r| matches!(
                r,
                ExecutionEvidence::Event {
                    event: ExecutionEvent::Allocation { requested: 16, .. },
                    ..
                }
            ))
            .count(),
        2
    );
    super::comparison::check_preservation(&f, r.clone());
    r.cases[0].replacement.as_mut().unwrap().arguments[0] = Some(12);
    assert_eq!(run(&f, r).0.verdict, Some(ComparisonVerdict::Diff));
    p.fields[0].count = 0;
    assert!(p.validate().is_err());
    p.fields[0].count = u32::MAX;
    assert!(p.validate().is_err());
}
