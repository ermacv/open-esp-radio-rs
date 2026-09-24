use super::interfaces::run;
use super::*;
fn setup(omit: bool) -> (Fixture, ExecutionRequest, EffectContract) {
    let vendor = [0x00b52023, 0x00000513, 0x00008067];
    let replacement = if omit {
        [0x00000013, 0x00000513, 0x00008067]
    } else {
        vendor
    };
    let (a, pa) = super::goals::symbol_elf(&vendor, 0x1000, 0x1000);
    let (b, mut pb) = super::goals::symbol_elf(&replacement, 0x1000, 0x1000);
    pb.source = FunctionSource::Input { input: 1 };
    let f = Fixture::from_inputs(vec![a, b]);
    let endpoint = |point: ExecutionSymbol| CallEndpoint {
        occurrence: KnowledgeOccurrence {
            revision: f.target.revision.clone(),
            source: point.source,
            object: point.symbol.object.clone(),
            symbol: Some(point.symbol),
        },
        boundary: ReviewedCallBoundary::Code { address: 0x1000 },
    };
    let pattern = EffectPattern {
        selector: EffectSelector::MmioWrite {
            address: 0x3000,
            width: 4,
        },
        value: EffectValue::Any,
    };
    let p = EffectContract {
        vendor: endpoint(pa),
        replacement: endpoint(pb),
        rules: vec![EffectRule {
            name: "initial register write".into(),
            vendor: Some(pattern),
            replacement: Some(pattern),
            disposition: if omit {
                EffectDisposition::Omitted
            } else {
                EffectDisposition::Required
            },
            min_occurrences: 1,
            max_occurrences: 1,
            reason: "explicit synthetic refinement".into(),
        }],
        claim_ceiling: EffectClaimCeiling::ReviewedEffectRefinement,
        applicability: "exact fixture root inputs".into(),
        reason: "synthetic effect policy".into(),
    };
    let mut r = f.request();
    r.replacement.as_mut().unwrap().source = FunctionSource::Input { input: 1 };
    let v = &mut r.cases[0].vendor;
    v.arguments[0] = Some(0x3000);
    v.arguments[1] = Some(7);
    v.models = vec![register_bank(vec![RegisterCell {
        address: 0x3000,
        width: 4,
        value: 0,
    }])];
    r.cases[0].replacement = Some(v.clone());
    (f, r, p)
}
fn proposal(p: &EffectContract) -> KnowledgeProposal {
    KnowledgeProposal {
        subject: "fixture.effects".to_owned().try_into().unwrap(),
        occurrence: p.vendor.occurrence.clone(),
        claim: KnowledgeClaim::EffectContract {
            contract: Box::new(p.clone()),
        },
        evidence: vec![EvidenceRef::Source {
            payload: p.vendor.occurrence.object.artifact.clone(),
            range: CodeRange {
                start: 0,
                length: 4,
            },
        }],
        note: None,
    }
}
fn propose(
    f: &Fixture,
    p: &EffectContract,
    base: Option<KnowledgeRevisionId>,
    specialized: bool,
) -> EffectReview {
    let h = if specialized {
        f.app.start_propose_effect_contract(
            &f.project,
            EffectProposalRequest {
                subject: proposal(p).subject,
                contract: p.clone(),
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
    let id = super::call_pairs::proposed_id(f, h.wait());
    EffectReview {
        knowledge: id.knowledge,
        assertion: id.assertion,
    }
}
fn review(f: &Fixture, p: EffectReview, decision: ReviewDecision) -> EffectReview {
    let run = f
        .app
        .start_knowledge(
            &f.project,
            &KnowledgeChange {
                expected_base: Some(p.knowledge),
                actor: "fixture".into(),
                reason: "synthetic review".into(),
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
    assert_eq!(run.state, RunState::Completed, "{run:?}");
    EffectReview {
        knowledge: run.knowledge.unwrap(),
        assertion: p.assertion,
    }
}
fn select(r: &mut ExecutionRequest, p: EffectReview) {
    r.cases[0].relation.as_mut().unwrap().effects = Some(p);
}
#[test]
fn effect_review_api_preserves_raw_omissions_and_source_free_replay() {
    for specialized in [false, true] {
        let (f, mut r, p) = setup(true);
        assert_eq!(run(&f, r.clone()).0.verdict, Some(ComparisonVerdict::Diff));
        let pending = propose(&f, &p, None, specialized);
        select(&mut r, pending.clone());
        let failed = f.run(r.clone(), budget());
        assert_eq!(failed.state, RunState::Failed);
        assert!(failed.execution.is_none());
        let accepted = review(&f, pending, ReviewDecision::Accept);
        select(&mut r, accepted.clone());
        let (manifest, rows) = run(&f, r.clone());
        assert_eq!(manifest.verdict, Some(ComparisonVerdict::Match));
        assert_eq!(
            manifest.effect_contracts,
            vec![ResolvedEffectContract {
                review: accepted,
                contract: p
            }]
        );
        assert!(rows.iter().any(|r| matches!(
            r,
            ExecutionEvidence::Event {
                replacement: false,
                event: ExecutionEvent::Write {
                    address: 0x3000,
                    value: 7,
                    ..
                },
                ..
            }
        )));
        assert!(rows.iter().any(|r| matches!(
            r,
            ExecutionEvidence::Comparison {
                result: CaseComparison {
                    effect_claim: Some(EffectClaimCeiling::ReviewedEffectRefinement),
                    effect_gap: None,
                    verdict: ComparisonVerdict::Match,
                    ..
                },
                ..
            }
        )));
        if specialized {
            super::comparison::check_preservation(&f, r);
        }
    }
}
#[test]
fn generic_effect_proposal_rejects_invalid_physical_endpoints_without_publishing() {
    let (f, _, p) = setup(true);
    for side in [false, true] {
        for variant in 0..5 {
            let mut bad = p.clone();
            let e = if side {
                &mut bad.replacement
            } else {
                &mut bad.vendor
            };
            match variant {
                0 => e.occurrence.symbol.as_mut().unwrap().index = 9999,
                1 => e.boundary = ReviewedCallBoundary::Code { address: 0x1002 },
                2 => e.occurrence.symbol.as_mut().unwrap().table = SymbolTableKind::Dynamic,
                3 => e.occurrence.symbol.as_mut().unwrap().table_section = 99,
                _ => e.occurrence.source = FunctionSource::Input { input: 9 },
            }
            let result = f.app.start_knowledge(
                &f.project,
                &KnowledgeChange {
                    expected_base: None,
                    actor: "fixture".into(),
                    reason: "invalid physical effect endpoint".into(),
                    action: KnowledgeAction::Propose {
                        proposal: proposal(&bad),
                    },
                },
                budget(),
            );
            if let Ok(h) = result {
                let run = h.wait();
                assert_eq!(
                    run.state,
                    RunState::Failed,
                    "side {side} variant {variant}: {run:?}"
                );
                assert!(run.knowledge.is_none());
            }
        }
    }
    propose(&f, &p, None, false);
}
#[test]
fn reviewed_effect_replacement_requires_exact_values_and_case_applicability() {
    let (f, mut r, mut p) = setup(false);
    r.cases[0].replacement.as_mut().unwrap().arguments[1] = Some(9);
    p.rules[0].disposition = EffectDisposition::Replaced;
    p.rules[0].vendor.as_mut().unwrap().value = EffectValue::Exact { value: 7 };
    p.rules[0].replacement.as_mut().unwrap().value = EffectValue::Exact { value: 9 };
    let accepted = review(&f, propose(&f, &p, None, true), ReviewDecision::Accept);
    select(&mut r, accepted);
    assert_eq!(run(&f, r.clone()).0.verdict, Some(ComparisonVerdict::Match));
    let mut wrong = r.clone();
    wrong.cases[0].replacement.as_mut().unwrap().arguments[1] = Some(8);
    let (m, rows) = run(&f, wrong);
    assert_eq!(m.verdict, Some(ComparisonVerdict::Diff));
    assert!(rows.iter().any(|r| matches!(
        r,
        ExecutionEvidence::Comparison {
            result: CaseComparison {
                difference: Some(ComparisonDifference::EffectViolation {
                    violation: EffectViolation {
                        replacement: true,
                        kind: EffectViolationKind::Value,
                        ..
                    }
                }),
                ..
            },
            ..
        }
    )));
    for variant in 0..4 {
        let mut wrong = r.clone();
        match variant {
            0 => wrong.cases[0].replacement.as_mut().unwrap().entry = 0x1004,
            1 => wrong.replacement.as_mut().unwrap().source = FunctionSource::Input { input: 0 },
            2 => {
                wrong.cases[0]
                    .relation
                    .as_mut()
                    .unwrap()
                    .effects
                    .as_mut()
                    .unwrap()
                    .assertion = ArtifactId::of_bytes(b"missing").as_str().parse().unwrap()
            }
            _ => wrong.cases[0].relation.as_mut().unwrap().events.delay = false,
        }
        if let Ok(h) = f.app.start_execution(
            &f.project,
            wrong,
            &blobray_backend_riscv::RiscvExecutor,
            budget(),
        ) {
            let failed = h.wait();
            assert_eq!(failed.state, RunState::Failed, "{failed:?}");
            assert!(failed.execution.is_none());
        }
    }
    assert_eq!(run(&f, r).0.verdict, Some(ComparisonVerdict::Match));
}
#[test]
fn effect_cli_review_freezes_policy_and_rejects_conflicting_acceptance() {
    let (f, mut r, p) = setup(true);
    let request = EffectProposalRequest {
        subject: proposal(&p).subject,
        contract: p.clone(),
        expected_base: None,
        actor: "fixture".into(),
        reason: "CLI proposal".into(),
    };
    let path = f._dir.path().join("effect.json");
    fs::write(&path, serde_json::to_vec(&request).unwrap()).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args(["--format", "json", "knowledge", "--project"])
        .arg(&f.project)
        .args([
            "--limit-mode",
            "watchdog",
            "propose-effect-contract",
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
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let record: app::RunRecord = serde_json::from_value(value["run"].clone()).unwrap();
    let id = super::call_pairs::proposed_id(&f, record);
    let accepted = review(
        &f,
        EffectReview {
            knowledge: id.knowledge,
            assertion: id.assertion,
        },
        ReviewDecision::Accept,
    );
    select(&mut r, accepted.clone());
    let original = run(&f, r.clone());
    let mut conflict = p.clone();
    conflict.rules[0].disposition = EffectDisposition::Required;
    let pending = propose(&f, &conflict, Some(accepted.knowledge.clone()), false);
    let failed = f
        .app
        .start_knowledge(
            &f.project,
            &KnowledgeChange {
                expected_base: Some(pending.knowledge.clone()),
                actor: "fixture".into(),
                reason: "conflict".into(),
                action: KnowledgeAction::Review {
                    assertion: pending.assertion.clone(),
                    decision: ReviewDecision::Accept,
                    supersedes: None,
                },
            },
            budget(),
        )
        .unwrap()
        .wait();
    assert_eq!(failed.state, RunState::Failed);
    assert!(failed.knowledge.is_none());
    let rejected = review(&f, pending, ReviewDecision::Reject);
    let mut wrong = r.clone();
    select(&mut wrong, rejected);
    assert_eq!(f.run(wrong, budget()).state, RunState::Failed);
    assert_eq!(run(&f, r.clone()), original);
    super::comparison::check_preservation(&f, r);
}

#[test]
fn effect_policy_composes_with_reviewed_abi_layout_timeline_returns_and_final_ram() {
    // RAM write/read, conditional branch, omittable MMIO, captured call and explicit root return.
    let code = [
        0x00008413, 0x00b52023, 0x00052283, 0x00060463, 0x00158593, 0x00e6a023, 0x018000ef,
        0x00000513, 0x00040067, 0x00000013, 0x00000013, 0x00000013, 0x00008067,
    ];
    let mut right = vec![0x00000013];
    right.extend(code);
    right[6] = 0x00000013;
    let (a, pa) = super::goals::symbol_elf(&code, 0x1000, 0x1030);
    let (b, mut pb) = super::goals::symbol_elf(&right, 0x1000, 0x1034);
    pb.source = FunctionSource::Input { input: 1 };
    let f = Fixture::from_inputs(vec![a, b]);
    let endpoint = |point: ExecutionSymbol, address| CallEndpoint {
        occurrence: KnowledgeOccurrence {
            revision: f.target.revision.clone(),
            source: point.source,
            object: point.symbol.object.clone(),
            symbol: Some(point.symbol),
        },
        boundary: ReviewedCallBoundary::Code { address },
    };
    let callee_a = endpoint(pa, 0x1030);
    let callee_b = endpoint(pb, 0x1034);
    let mut root_a = callee_a.clone();
    root_a.boundary = ReviewedCallBoundary::Code { address: 0x1000 };
    root_a.occurrence.symbol.as_mut().unwrap().index = 1;
    let mut root_b = callee_b.clone();
    root_b.boundary = ReviewedCallBoundary::Code { address: 0x1000 };
    root_b.occurrence.symbol.as_mut().unwrap().index = 1;
    let layout = LayoutProjection {
        vendor: LayoutEndpoint {
            entry: root_a.clone(),
            domains: vec![LayoutDomain {
                address: 0x3000,
                length: 4,
            }],
        },
        replacement: LayoutEndpoint {
            entry: root_b.clone(),
            domains: vec![LayoutDomain {
                address: 0x5000,
                length: 8,
            }],
        },
        fields: vec![LayoutField {
            name: "counter".into(),
            vendor: FieldLocation {
                domain: 0,
                offset: 0,
            },
            replacement: FieldLocation {
                domain: 0,
                offset: 4,
            },
            width: 4,
            count: 1,
            final_state: true,
            timeline: true,
        }],
        branches: vec![BranchPair {
            vendor: BranchLocation {
                site: 0x100c,
                target: 0x1014,
                fallthrough: 0x1010,
            },
            replacement: BranchLocation {
                site: 0x1010,
                target: 0x1018,
                fallthrough: 0x1014,
            },
        }],
        applicability: "synthetic root entries".into(),
        reason: "same counter and branch".into(),
    };
    let pair = CallCorrespondence {
        vendor: callee_a,
        replacement: callee_b,
        arguments: CallArguments::Projected {
            words: vec![CallWordPair {
                vendor: 1,
                replacement: 4,
            }],
        },
        applicability: "synthetic captured callee".into(),
        reason: "explicit corresponding input word".into(),
    };
    let accept = |subject: &str, occurrence: KnowledgeOccurrence, claim, base| {
        let proposed = f
            .app
            .start_knowledge(
                &f.project,
                &KnowledgeChange {
                    expected_base: base,
                    actor: "fixture".into(),
                    reason: "composed policy".into(),
                    action: KnowledgeAction::Propose {
                        proposal: KnowledgeProposal {
                            subject: subject.to_owned().try_into().unwrap(),
                            evidence: vec![EvidenceRef::Source {
                                payload: occurrence.object.artifact.clone(),
                                range: CodeRange {
                                    start: 0,
                                    length: 4,
                                },
                            }],
                            occurrence,
                            claim,
                            note: None,
                        },
                    },
                },
                budget(),
            )
            .unwrap()
            .wait();
        let id = super::call_pairs::proposed_id(&f, proposed);
        review(
            &f,
            EffectReview {
                knowledge: id.knowledge,
                assertion: id.assertion,
            },
            ReviewDecision::Accept,
        )
    };
    let projection = accept(
        "fixture.layout",
        root_a.occurrence.clone(),
        KnowledgeClaim::LayoutProjection {
            projection: Box::new(layout),
        },
        None,
    );
    let calls = accept(
        "fixture.call",
        pair.vendor.occurrence.clone(),
        KnowledgeClaim::CallPair {
            correspondence: Box::new(pair),
        },
        Some(projection.knowledge.clone()),
    );
    let pattern = EffectPattern {
        selector: EffectSelector::MmioWrite {
            address: 0x6000,
            width: 4,
        },
        value: EffectValue::Exact { value: 11 },
    };
    let contract = EffectContract {
        vendor: root_a,
        replacement: root_b,
        rules: vec![EffectRule {
            name: "optional synthetic write".into(),
            vendor: Some(pattern),
            replacement: Some(pattern),
            disposition: EffectDisposition::Omitted,
            min_occurrences: 1,
            max_occurrences: 1,
            reason: "fixture relaxation".into(),
        }],
        claim_ceiling: EffectClaimCeiling::ReviewedEffectRefinement,
        applicability: "exact synthetic entries".into(),
        reason: "composition regression".into(),
    };
    let effects = review(
        &f,
        propose(&f, &contract, Some(calls.knowledge.clone()), true),
        ReviewDecision::Accept,
    );
    let mut r = f.request();
    r.max_events = 128;
    r.replacement.as_mut().unwrap().source = FunctionSource::Input { input: 1 };
    let capture = TimelineCapture {
        reads: true,
        writes: true,
        branches: true,
        atomics: false,
    };
    for side in [false, true] {
        let input = if side {
            r.cases[0].replacement.as_mut().unwrap()
        } else {
            &mut r.cases[0].vendor
        };
        let address = if side { 0x5000 } else { 0x3000 };
        let length = if side { 8 } else { 4 };
        input.arguments = vec![
            Some(if side { 0x5004 } else { 0x3000 }),
            Some(7),
            Some(0),
            Some(0x6000),
            Some(if side { 7 } else { 11 }),
            Some(0),
            Some(0),
            Some(0),
        ];
        input.memory = vec![
            ExecutionRegion {
                lifetime: RegionLifetime::Session,
                seed: MemorySeed {
                    address,
                    length,
                    fill: None,
                    bytes: vec![],
                },
            },
            ExecutionRegion {
                lifetime: RegionLifetime::Phase,
                seed: MemorySeed {
                    address: 0x7000,
                    length: 4,
                    fill: Some(9),
                    bytes: vec![],
                },
            },
        ];
        input.observe_memory = vec![
            MemorySelection {
                name: "layout".into(),
                address,
                length,
            },
            MemorySelection {
                name: "physical".into(),
                address: 0x7000,
                length: 4,
            },
        ];
        input.observe_timeline = capture;
        input.observe_calls = Some(CallCapture {
            include_tail: false,
            argument_words: 8,
            overrides: vec![],
        });
        input.models = vec![register_bank(vec![RegisterCell {
            address: 0x6000,
            width: 4,
            value: 0,
        }])];
    }
    let relation = r.cases[0].relation.as_mut().unwrap();
    relation.returns.high = true;
    relation.memory = vec![MemoryPair {
        vendor: 1,
        replacement: 1,
    }];
    relation.events.timeline = capture;
    relation.projection = Some(ProjectionReview {
        knowledge: projection.knowledge,
        assertion: projection.assertion,
    });
    relation.reviewed_calls = Some(ReviewedCalls {
        pairs: vec![CallPairReview {
            knowledge: calls.knowledge,
            assertion: calls.assertion,
        }],
        unlisted: UnlistedCalls::Exact,
    });
    relation.effects = Some(effects);
    let (m, rows) = run(&f, r.clone());
    assert_eq!(m.verdict, Some(ComparisonVerdict::Match));
    assert_eq!(
        (
            m.projections.len(),
            m.call_pairs.len(),
            m.effect_contracts.len()
        ),
        (1, 1, 1)
    );
    assert!(rows.iter().any(|r| matches!(
        r,
        ExecutionEvidence::Event {
            event: ExecutionEvent::CallTransfer { target: 0x1034, .. },
            ..
        }
    )));
    for variant in 0..5 {
        let mut changed = r.clone();
        match variant {
            0 => changed.cases[0].relation.as_mut().unwrap().effects = None,
            1 => changed.cases[0].relation.as_mut().unwrap().projection = None,
            2 => changed.cases[0].replacement.as_mut().unwrap().arguments[4] = Some(8),
            3 => changed.cases[0].replacement.as_mut().unwrap().arguments[2] = Some(1),
            _ => {
                changed.cases[0].replacement.as_mut().unwrap().memory[1]
                    .seed
                    .fill = Some(8)
            }
        }
        let (manifest, rows) = run(&f, changed);
        assert_eq!(
            manifest.verdict,
            Some(ComparisonVerdict::Diff),
            "variant {variant}: {rows:?}"
        );
    }
    super::comparison::check_preservation(&f, r);
}
