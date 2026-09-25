use super::interfaces::run;
use super::*;
fn setup() -> (Fixture, ExecutionRequest, CallCorrespondence) {
    setup_code(
        &[0x00008413, 0x00c000ef, 0x00040067, 0x00000013, 0x00008067],
        0x1010,
        &[
            0x00008413, 0x010000ef, 0x00040067, 0x00000013, 0x00000013, 0x00008067,
        ],
        0x1014,
    )
}
fn setup_code(
    left: &[u32],
    la: u32,
    right: &[u32],
    ra: u32,
) -> (Fixture, ExecutionRequest, CallCorrespondence) {
    let (a, pa) = super::goals::symbol_elf(left, 0x1000, la);
    let (b, mut pb) = super::goals::symbol_elf(right, 0x1000, ra);
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
    let pair = CallCorrespondence {
        vendor: endpoint(pa, la),
        replacement: endpoint(pb, ra),
        arguments: CallArguments::Exact { words: 8 },
        applicability: "synthetic captured pair".into(),
        reason: "same chosen operation".into(),
    };
    let mut r = f.request();
    r.replacement.as_mut().unwrap().source = FunctionSource::Input { input: 1 };
    r.max_events = 128;
    r.cases[0].vendor.observe_calls = Some(CallCapture {
        include_tail: false,
        argument_words: 8,
        overrides: vec![],
    });
    r.cases[0].replacement = Some(r.cases[0].vendor.clone());
    (f, r, pair)
}
fn proposal(pair: &CallCorrespondence) -> KnowledgeProposal {
    KnowledgeProposal {
        subject: "fixture.call-pair".to_owned().try_into().unwrap(),
        occurrence: pair.vendor.occurrence.clone(),
        claim: KnowledgeClaim::CallPair {
            correspondence: Box::new(pair.clone()),
        },
        evidence: vec![EvidenceRef::Source {
            payload: pair.vendor.occurrence.object.artifact.clone(),
            range: CodeRange {
                start: 0,
                length: 4,
            },
        }],
        note: None,
    }
}
pub(super) fn proposed_id(f: &Fixture, run: app::RunRecord) -> CallPairReview {
    assert_eq!(run.state, RunState::Completed, "{run:?}");
    let revision = run.knowledge.unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args(["--format", "json", "knowledge", "--project"])
        .arg(&f.project)
        .args([
            "--limit-mode",
            "watchdog",
            "show",
            "--revision",
            revision.as_str(),
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let shown: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let assertion = shown["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["value"]["proposed_in"].as_str() == Some(revision.as_str()))
        .unwrap()["value"]["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    CallPairReview {
        knowledge: revision,
        assertion,
    }
}
fn propose(
    f: &Fixture,
    pair: &CallCorrespondence,
    base: Option<KnowledgeRevisionId>,
    specialized: bool,
) -> CallPairReview {
    let h = if specialized {
        f.app.start_propose_call_pair(
            &f.project,
            CallPairProposalRequest {
                subject: proposal(pair).subject,
                correspondence: pair.clone(),
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
                    proposal: proposal(pair),
                },
            },
            budget(),
        )
    }
    .unwrap();
    proposed_id(f, h.wait())
}
fn review(f: &Fixture, selected: CallPairReview, decision: ReviewDecision) -> CallPairReview {
    let run = f
        .app
        .start_knowledge(
            &f.project,
            &KnowledgeChange {
                expected_base: Some(selected.knowledge),
                actor: "fixture".into(),
                reason: "synthetic decision".into(),
                action: KnowledgeAction::Review {
                    assertion: selected.assertion.clone(),
                    decision,
                    supersedes: None,
                },
            },
            budget(),
        )
        .unwrap()
        .wait();
    assert_eq!(run.state, RunState::Completed, "{run:?}");
    CallPairReview {
        knowledge: run.knowledge.unwrap(),
        assertion: selected.assertion,
    }
}
fn select(r: &mut ExecutionRequest, review: CallPairReview) {
    let relation = r.cases[0].relation.as_mut().unwrap();
    relation.returns.low = false;
    relation.reviewed_calls = Some(ReviewedCalls {
        pairs: vec![review],
        unlisted: UnlistedCalls::Exact,
    });
}
#[test]
fn accepted_exact_call_pairs_map_distinct_physical_targets_and_replay_without_sources() {
    let (f, mut r, pair) = setup();
    let p = propose(&f, &pair, None, true);
    select(&mut r, p.clone());
    let failed = f.run(r.clone(), budget());
    assert!(failed.execution.is_none());
    assert_eq!(failed.error.unwrap().code, ErrorCode::InvalidRequest);
    let accepted = review(&f, p, ReviewDecision::Accept);
    select(&mut r, accepted.clone());
    let (m, rows) = run(&f, r.clone());
    assert_eq!(m.verdict, Some(ComparisonVerdict::Match));
    assert_eq!(m.call_pairs.len(), 1);
    assert_eq!(m.call_pairs[0].review, accepted);
    assert_eq!(m.call_pairs[0].correspondence, pair);
    for target in [0x1010, 0x1014] {
        assert!(rows.iter().any(|r| matches!(r, ExecutionEvidence::Event { event: ExecutionEvent::CallTransfer { target: t, .. }, .. } if *t==target)));
    }
    super::comparison::check_preservation(&f, r.clone());
    r.cases[0].replacement.as_mut().unwrap().arguments[7] = Some(42);
    let (m, rows) = run(&f, r);
    assert_eq!(m.verdict, Some(ComparisonVerdict::Diff));
    assert!(rows.iter().any(|r| matches!(
        r,
        ExecutionEvidence::Comparison {
            result: CaseComparison {
                difference: Some(ComparisonDifference::CallArgument {
                    word: 7,
                    replacement: 42,
                    ..
                }),
                ..
            },
            ..
        }
    )));
}
#[test]
fn selected_and_ignored_policies_do_not_discard_raw_unknown_words() {
    for arguments in [
        CallArguments::Selected { words: vec![1, 7] },
        CallArguments::Ignore,
    ] {
        let (f, mut r, mut pair) = setup();
        pair.arguments = arguments;
        let accepted = review(&f, propose(&f, &pair, None, false), ReviewDecision::Accept);
        select(&mut r, accepted);
        r.cases[0].vendor.arguments[2] = None;
        r.cases[0].replacement.as_mut().unwrap().arguments[2] = Some(123);
        let (m, rows) = run(&f, r.clone());
        assert_eq!(m.verdict, Some(ComparisonVerdict::Match));
        assert!(rows.iter().any(|r| matches!(
            r,
            ExecutionEvidence::Event {
                replacement: false,
                event: ExecutionEvent::TransferArgument {
                    word: 2,
                    value: ObservedWord::Unknown
                },
                ..
            }
        )));
        r.cases[0].replacement.as_mut().unwrap().arguments[7] = None;
        assert_eq!(
            run(&f, r).0.verdict,
            Some(if matches!(pair.arguments, CallArguments::Ignore) {
                ComparisonVerdict::Match
            } else {
                ComparisonVerdict::Incomplete
            })
        );
    }
}
#[test]
fn generic_call_pair_proposals_validate_both_exact_physical_occurrences() {
    let (f, _, pair) = setup();
    for side in [false, true] {
        for variant in 0..4 {
            let mut bad = pair.clone();
            let endpoint = if side {
                &mut bad.replacement
            } else {
                &mut bad.vendor
            };
            match variant {
                0 => endpoint.occurrence.symbol.as_mut().unwrap().index = 99999,
                1 => endpoint.occurrence.symbol.as_mut().unwrap().table = SymbolTableKind::Dynamic,
                2 => endpoint.boundary = ReviewedCallBoundary::Code { address: 0x1000 },
                _ => endpoint.occurrence.object.artifact = ArtifactId::of_bytes(b"wrong object"),
            }
            let result = f.app.start_knowledge(
                &f.project,
                &KnowledgeChange {
                    expected_base: None,
                    actor: "fixture".into(),
                    reason: "bad physical endpoint".into(),
                    action: KnowledgeAction::Propose {
                        proposal: proposal(&bad),
                    },
                },
                budget(),
            );
            match result {
                Err(_) => (),
                Ok(h) => {
                    let run = h.wait();
                    assert_eq!(run.state, RunState::Failed);
                    assert!(run.knowledge.is_none());
                }
            }
        }
    }
    let accepted = review(&f, propose(&f, &pair, None, true), ReviewDecision::Accept);
    assert!(!accepted.knowledge.as_str().is_empty()); // failed attempts left the empty base unchanged
}
#[test]
fn unlisted_calls_require_explicit_exclusion_and_remain_in_retained_evidence() {
    let (f, mut r, pair) = setup_code(
        &[
            0x00008413, 0x00c000ef, 0x00c000ef, 0x00040067, 0x00008067, 0x00008067,
        ],
        0x1010,
        &[
            0x00008413, 0x014000ef, 0x00000013, 0x00040067, 0x00000013, 0x00000013, 0x00008067,
        ],
        0x1018,
    );
    let accepted = review(&f, propose(&f, &pair, None, false), ReviewDecision::Accept);
    select(&mut r, accepted);
    assert_eq!(run(&f, r.clone()).0.verdict, Some(ComparisonVerdict::Diff));
    r.cases[0]
        .relation
        .as_mut()
        .unwrap()
        .reviewed_calls
        .as_mut()
        .unwrap()
        .unlisted = UnlistedCalls::Exclude;
    let (m, rows) = run(&f, r);
    assert_eq!(m.verdict, Some(ComparisonVerdict::Match));
    assert!(rows.iter().any(|r| matches!(
        r,
        ExecutionEvidence::Event {
            replacement: false,
            event: ExecutionEvent::CallTransfer { target: 0x1014, .. },
            ..
        }
    )));
}
#[test]
fn specialized_cli_proposal_and_rejected_or_mismatched_selections_fail_closed() {
    let (f, mut r, pair) = setup();
    let request = CallPairProposalRequest {
        subject: proposal(&pair).subject,
        correspondence: pair.clone(),
        expected_base: None,
        actor: "fixture".into(),
        reason: "CLI proposal".into(),
    };
    let path = f._dir.path().join("pair.json");
    fs::write(&path, serde_json::to_vec(&request).unwrap()).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args(["--format", "json", "knowledge", "--project"])
        .arg(&f.project)
        .args(["--limit-mode", "watchdog", "propose-call-pair", "--request"])
        .arg(path)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let p = proposed_id(&f, serde_json::from_value(out["run"].clone()).unwrap());
    let rejected = review(&f, p, ReviewDecision::Reject);
    select(&mut r, rejected.clone());
    assert_eq!(
        f.run(r.clone(), budget()).error.unwrap().code,
        ErrorCode::InvalidRequest
    );
    let accepted = review(
        &f,
        propose(&f, &pair, Some(rejected.knowledge), false),
        ReviewDecision::Accept,
    );
    select(&mut r, accepted);
    for variant in 0..3 {
        let mut bad = r.clone();
        match variant {
            0 => bad.replacement.as_mut().unwrap().source = FunctionSource::Input { input: 0 },
            1 => {
                bad.cases[0]
                    .vendor
                    .observe_calls
                    .as_mut()
                    .unwrap()
                    .argument_words = 7;
                bad.cases[0].replacement = Some(bad.cases[0].vendor.clone());
            }
            _ => {
                bad.cases[0]
                    .relation
                    .as_mut()
                    .unwrap()
                    .reviewed_calls
                    .as_mut()
                    .unwrap()
                    .pairs[0]
                    .assertion = ArtifactId::of_bytes(b"missing").as_str().parse().unwrap()
            }
        }
        let result = f.run(bad, budget());
        assert_eq!(result.state, RunState::Failed);
        assert!(result.execution.is_none());
    }
    assert_eq!(run(&f, r).0.verdict, Some(ComparisonVerdict::Match));
}
#[test]
fn modeled_pair_identity_and_phase_ownership_are_part_of_applicability() {
    let (f, mut r, mut pair) = setup();
    {
        let (endpoint, input) = (&mut pair.vendor, &mut r.cases[0].vendor);
        endpoint.occurrence.symbol = None;
        let binding = CallBinding {
            address: endpoint.address(),
            boundary: CallBoundary::CapturedCode,
            allow_tail: false,
        };
        let model = CallDeclaration {
            repetition: blobray_domain::CallRepetition::Finite,
            id: "callee".into(),
            applicability: "test model".into(),
            lifetime: RegionLifetime::Session,
            binding,
            argument_words: 1,
            responses: vec![
                CallResponse {
                    return_words: [Some(0), None],
                    outputs: vec![],
                    allocation: None,
                    delay_micros: None
                };
                2
            ],
        };
        endpoint.boundary = ReviewedCallBoundary::Model {
            binding,
            definition: model.identity(&mut || Ok(())).unwrap(),
        };
        input.calls = vec![model];
    }
    pair.replacement.occurrence.symbol = None;
    let calls = r.cases[0].vendor.calls.clone();
    let other = r.cases[0].replacement.as_mut().unwrap();
    other.calls = calls;
    other.calls[0].binding.address = pair.replacement.address();
    pair.replacement.boundary = ReviewedCallBoundary::Model {
        binding: other.calls[0].binding,
        definition: other.calls[0].identity(&mut || Ok(())).unwrap(),
    };
    // Unknown caller-saved arguments after a response are excluded by this reviewed policy.
    pair.arguments = CallArguments::Projected {
        words: vec![CallWordPair {
            vendor: 0,
            replacement: 0,
        }],
    };
    let accepted = review(&f, propose(&f, &pair, None, true), ReviewDecision::Accept);
    select(&mut r, accepted);
    let mut warm = r.cases[0].clone();
    warm.name = "warm".into();
    warm.reset = SessionReset::Warm;
    warm.vendor.calls.clear();
    warm.replacement.as_mut().unwrap().calls.clear();
    r.cases.push(warm);
    assert_eq!(run(&f, r.clone()).0.verdict, Some(ComparisonVerdict::Match));
    r.cases[0].vendor.calls[0].responses[0].return_words[0] = Some(9);
    let failed = f.run(r, budget());
    assert_eq!(failed.error.unwrap().code, ErrorCode::InvalidRequest);
    assert!(failed.execution.is_none());
}
#[test]
fn fifo_correspondence_pins_definition_slot_and_selected_reviewed_table() {
    let (f, mut r) = super::services::setup(vec![11]);
    let table_review = &r.cases[0].vendor.tables[0].review;
    let out = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args(["--format", "json", "knowledge", "--project"])
        .arg(&f.project)
        .args([
            "--limit-mode",
            "watchdog",
            "show",
            "--revision",
            table_review.knowledge.as_str(),
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let shown: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let occurrence: KnowledgeOccurrence = serde_json::from_value(
        shown["records"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["value"]["id"].as_str() == Some(table_review.assertion.as_str()))
            .unwrap()["value"]["proposal"]["occurrence"]
            .clone(),
    )
    .unwrap();
    let service = &r.cases[0].vendor.services[0];
    let endpoint = CallEndpoint {
        occurrence,
        boundary: ReviewedCallBoundary::Service {
            binding: service.bindings[0].call,
            definition: service.identity(&mut || Ok(())).unwrap(),
            binding_index: 0,
        },
    };
    let pair = CallCorrespondence {
        vendor: endpoint.clone(),
        replacement: endpoint,
        arguments: CallArguments::Ignore,
        applicability: "queue service fixture".into(),
        reason: "same explicit FIFO boundary".into(),
    };
    let accepted = review(
        &f,
        propose(&f, &pair, Some(table_review.knowledge.clone()), true),
        ReviewDecision::Accept,
    );
    r.cases[0].vendor.observe_calls = Some(CallCapture {
        include_tail: false,
        argument_words: 8,
        overrides: vec![],
    });
    r.cases[0].replacement = Some(r.cases[0].vendor.clone());
    select(&mut r, accepted);
    let (m, rows) = run(&f, r.clone());
    assert_eq!(m.verdict, Some(ComparisonVerdict::Match));
    assert!(rows.iter().any(|r| matches!(
        r,
        ExecutionEvidence::Event {
            event: ExecutionEvent::CallTransfer {
                target_kind: ObservedCallTarget::FifoService,
                ..
            },
            ..
        }
    )));
    r.cases[0].replacement.as_mut().unwrap().services[0].items = vec![12];
    let failed = f.run(r, budget());
    assert_eq!(failed.error.unwrap().code, ErrorCode::InvalidRequest);
    assert!(failed.execution.is_none());
}
#[test]
fn duplicate_accepted_endpoint_selections_are_ambiguous_without_changing_frozen_review() {
    let (f, mut r, pair) = setup();
    let first = review(&f, propose(&f, &pair, None, false), ReviewDecision::Accept);
    let mut second = proposal(&pair);
    second.note = Some("independent identical review".into());
    let p = f
        .app
        .start_knowledge(
            &f.project,
            &KnowledgeChange {
                expected_base: Some(first.knowledge.clone()),
                actor: "fixture".into(),
                reason: "second proposal".into(),
                action: KnowledgeAction::Propose { proposal: second },
            },
            budget(),
        )
        .unwrap()
        .wait();
    let second = review(&f, proposed_id(&f, p), ReviewDecision::Accept);
    select(&mut r, first);
    assert_eq!(run(&f, r.clone()).0.verdict, Some(ComparisonVerdict::Match));
    r.cases[0]
        .relation
        .as_mut()
        .unwrap()
        .reviewed_calls
        .as_mut()
        .unwrap()
        .pairs
        .push(second);
    let failed = f.run(r, budget());
    assert_eq!(failed.error.unwrap().code, ErrorCode::Conflict);
    assert!(failed.execution.is_none());
}

#[test]
fn reviewed_word_projection_maps_register_and_stack_positions_without_dropping_unknowns() {
    let (f, mut r, mut pair) = setup();
    pair.arguments = CallArguments::Projected {
        words: vec![
            CallWordPair {
                vendor: 0,
                replacement: 1,
            },
            CallWordPair {
                vendor: 7,
                replacement: 8,
            },
        ],
    };
    r.cases[0].vendor.arguments = vec![None; 8];
    r.cases[0].vendor.arguments[0] = Some(7);
    r.cases[0].vendor.arguments[7] = Some(42);
    let right = r.cases[0].replacement.as_mut().unwrap();
    right.arguments = vec![None; 9];
    right.arguments[1] = Some(7);
    right.arguments[8] = Some(42);
    right.observe_calls.as_mut().unwrap().argument_words = 9;
    let accepted = review(&f, propose(&f, &pair, None, true), ReviewDecision::Accept);
    select(&mut r, accepted);
    r.cases[0]
        .relation
        .as_mut()
        .unwrap()
        .reviewed_calls
        .as_mut()
        .unwrap()
        .unlisted = UnlistedCalls::Exclude;
    assert_eq!(run(&f, r.clone()).0.verdict, Some(ComparisonVerdict::Match));
    super::comparison::check_preservation(&f, r.clone());
    r.cases[0].replacement.as_mut().unwrap().arguments[8] = Some(43);
    assert_eq!(run(&f, r.clone()).0.verdict, Some(ComparisonVerdict::Diff));
    r.cases[0].replacement.as_mut().unwrap().arguments[8] = None;
    assert_eq!(
        run(&f, r.clone()).0.verdict,
        Some(ComparisonVerdict::Incomplete)
    );
    r.cases[0]
        .replacement
        .as_mut()
        .unwrap()
        .observe_calls
        .as_mut()
        .unwrap()
        .argument_words = 8;
    let failed = f.run(r, budget());
    assert_eq!(failed.state, RunState::Failed);
    assert!(failed.execution.is_none());
    for words in [
        vec![
            CallWordPair {
                vendor: 0,
                replacement: 1,
            },
            CallWordPair {
                vendor: 0,
                replacement: 2,
            },
        ],
        vec![
            CallWordPair {
                vendor: 0,
                replacement: 1,
            },
            CallWordPair {
                vendor: 2,
                replacement: 1,
            },
        ],
        vec![CallWordPair {
            vendor: 256,
            replacement: 1,
        }],
        vec![],
    ] {
        pair.arguments = CallArguments::Projected { words };
        assert!(pair.validate().is_err());
    }
}
