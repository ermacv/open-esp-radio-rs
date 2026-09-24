use super::*;
fn declaration(root: InterfaceRoot, payload: ArtifactId) -> InterfaceContract {
    InterfaceContract {
        root,
        path: vec![],
        layout_version: "fixture/1".into(),
        layout_bytes: 16,
        pointer_bytes: 4,
        abi: CallAbi::RiscvInteger,
        index_domains: vec![],
        guards: vec![
            InterfaceGuard::CapturedPayload { payload },
            InterfaceGuard::RuntimeValue {
                offset: 0,
                width: 1,
                mask: 1,
                value: 0,
                purpose: "conditional runtime tag".into(),
            },
        ],
        slots: vec![InterfaceSlot {
            offset: 4,
            name: "callback".into(),
            semantic: Some("fixture.callback".to_owned().try_into().unwrap()),
            signature: Some(InterfaceSignature {
                arguments: vec![InterfaceArgument {
                    role: "fixture.input".to_owned().try_into().unwrap(),
                    value_type: InterfaceValueType::Integer {
                        bits: 32,
                        signed: false,
                    },
                }],
                result: InterfaceValueType::Integer {
                    bits: 32,
                    signed: false,
                },
                variadic: false,
            }),
        }],
        purpose: "fixture declaration".into(),
        applicability: "only this captured occurrence and declared guards".into(),
    }
}
fn cli(f: &Fixture, args: &[&str]) -> serde_json::Value {
    let out = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args(["--format", "json", args[0], "--project"])
        .arg(&f.project)
        .args(["--limit-mode", "watchdog"])
        .args(&args[1..])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    if out.stdout.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&out.stdout).unwrap()
    }
}
#[test]
fn native_interface_roots_validate_review_and_export_without_sources() {
    for thin in [false, true] {
        for kind in 0..4 {
            let bytes = data::data_object_bytes(false, &[0; 16]);
            let payload = ArtifactId::of_bytes(&bytes);
            let f = fixture(bytes.clone(), thin);
            let request = data::request(&f, &[b"table"]);
            let DataSelector::Symbol { symbol, .. } = &request.ranges[0] else {
                panic!()
            };
            let root = match kind {
                0 => InterfaceRoot::Symbol {
                    symbol: symbol.clone(),
                    addend: 0,
                },
                1 => InterfaceRoot::FunctionArgument {
                    function: f.request.selector.clone(),
                    argument: 0,
                },
                2 => InterfaceRoot::Address { address: 0x1000 },
                _ => {
                    use object::{Object as _, ObjectSection as _};
                    let elf = object::File::parse(bytes.as_slice()).unwrap();
                    InterfaceRoot::FunctionArgument {
                        function: FunctionSelector::Range {
                            object: f.request.selector.object().clone(),
                            section: elf.section_by_name(".text.entry").unwrap().index().0 as u32,
                            extent: CodeRange {
                                start: 0,
                                length: 8,
                            },
                        },
                        argument: 0,
                    }
                }
            };
            let mut occurrence = request.occurrence;
            if kind == 0 {
                occurrence.symbol = Some(symbol.clone());
            }
            let proposal = KnowledgeProposal {
                subject: "fixture.interface".to_owned().try_into().unwrap(),
                occurrence,
                claim: KnowledgeClaim::Interface {
                    contract: Box::new(declaration(root, payload.clone())),
                },
                evidence: vec![EvidenceRef::Source {
                    payload,
                    range: CodeRange {
                        start: 0,
                        length: bytes.len() as u64,
                    },
                }],
                note: None,
            };
            // Invalid optional data identity and root identity cannot slip through the generic API.
            let mut bad = proposal.clone();
            if kind == 0 {
                let KnowledgeClaim::Interface { contract } = &mut bad.claim else {
                    panic!()
                };
                let InterfaceRoot::Symbol { symbol, .. } = &mut contract.root else {
                    panic!()
                };
                symbol.index = u64::MAX;
                bad.occurrence.symbol = None;
            } else if kind == 1 {
                let KnowledgeClaim::Interface { contract } = &mut bad.claim else {
                    panic!()
                };
                let InterfaceRoot::FunctionArgument {
                    function: FunctionSelector::Symbol { symbol },
                    ..
                } = &mut contract.root
                else {
                    panic!()
                };
                symbol.index = u64::MAX;
            } else {
                let KnowledgeClaim::Interface { contract } = &mut bad.claim else {
                    panic!()
                };
                contract.guards[0] = InterfaceGuard::CapturedPayload {
                    payload: ArtifactId::of_bytes(b"wrong capture"),
                };
            }
            let change = |proposal| KnowledgeChange {
                expected_base: None,
                actor: "fixture-reviewer".into(),
                reason: "exact physical declaration".into(),
                action: KnowledgeAction::Propose { proposal },
            };
            let failed = f
                .app
                .start_knowledge(&f.project, &change(bad), budget())
                .unwrap()
                .wait();
            assert_eq!(failed.state, RunState::Failed, "{failed:?}");
            assert!(failed.knowledge.is_none());
            assert!(
                cli(&f, &["knowledge", "show"])["records"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
            fs::remove_file(f.dir.path().join("entry.a")).unwrap();
            fs::remove_file(f.dir.path().join("entry.o")).unwrap();
            let document = f.dir.path().join("interface.json");
            fs::write(
                &document,
                serde_json::to_vec(&change(proposal.clone())).unwrap(),
            )
            .unwrap();
            let proposed = cli(
                &f,
                &["knowledge", "apply", "--change", document.to_str().unwrap()],
            );
            let revision = proposed["run"]["knowledge"].as_str().unwrap();
            let claims = cli(&f, &["knowledge", "show"]);
            let assertion = claims["records"][0]["value"]["id"].as_str().unwrap();
            let accepted = cli(
                &f,
                &[
                    "knowledge",
                    "accept",
                    "--base",
                    revision,
                    "--assertion",
                    assertion,
                    "--actor",
                    "fixture-reviewer",
                    "--reason",
                    "conditional contract only",
                ],
            );
            let revision = accepted["run"]["knowledge"].as_str().unwrap();
            let mut limited = budget();
            limited.max_work_units = Some(1);
            let mut stopped = change(proposal.clone());
            stopped.expected_base = Some(revision.parse().unwrap());
            let failed = f
                .app
                .start_knowledge(&f.project, &stopped, limited)
                .unwrap()
                .wait();
            assert_eq!(failed.state, RunState::ResourceLimited);
            assert!(failed.knowledge.is_none());
            assert_eq!(
                cli(&f, &["knowledge", "show"])["summary"]["status"]["revision"],
                revision
            );
            let before = cli(&f, &["knowledge", "show", "--revision", revision]);
            assert_eq!(before["records"][0]["value"]["state"], "accepted");
            assert_eq!(
                before["records"][0]["value"]["proposal"],
                serde_json::to_value(&proposal).unwrap()
            );
            let output = f.dir.path().join("interface-export.json");
            cli(
                &f,
                &[
                    "knowledge",
                    "export",
                    "--revision",
                    revision,
                    "--output",
                    output.to_str().unwrap(),
                ],
            );
            let exported: serde_json::Value =
                serde_json::from_slice(&fs::read(&output).unwrap()).unwrap();
            assert!(exported["records"].as_array().unwrap().len() >= 2);
            assert_eq!(
                cli(&f, &["knowledge", "show", "--revision", revision])["records"],
                before["records"]
            );
        }
    }
}

fn observe(f: &Fixture, request: &InterfaceQuery) -> serde_json::Value {
    let file = f.dir.path().join("interface-query.json");
    fs::write(&file, serde_json::to_vec(request).unwrap()).unwrap();
    cli(f, &["interfaces", "--request", file.to_str().unwrap()])
}
fn propose(
    f: &Fixture,
    proposal: KnowledgeProposal,
    base: Option<KnowledgeRevisionId>,
) -> app::RunRecord {
    f.app
        .start_knowledge(
            &f.project,
            &KnowledgeChange {
                expected_base: base,
                actor: "fixture".into(),
                reason: "captured structural interface".into(),
                action: KnowledgeAction::Propose { proposal },
            },
            budget(),
        )
        .unwrap()
        .wait()
}
fn review(
    f: &Fixture,
    base: KnowledgeRevisionId,
    id: AssertionId,
    decision: ReviewDecision,
) -> app::RunRecord {
    f.app
        .start_knowledge(
            &f.project,
            &KnowledgeChange {
                expected_base: Some(base),
                actor: "fixture".into(),
                reason: "conditional fixture contract".into(),
                action: KnowledgeAction::Review {
                    assertion: id,
                    decision,
                    supersedes: None,
                },
            },
            budget(),
        )
        .unwrap()
        .wait()
}
#[test]
fn saved_callback_discovery_review_states_guards_and_export_share_one_query_contract() {
    for thin in [false, true] {
        let words = [0x00052283u32, 0x0082a303, 0x000300e7, 0x00008067];
        let code: Vec<_> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let bytes = object(&code, code.len() as u64, false);
        let payload = ArtifactId::of_bytes(&bytes);
        let f = fixture(bytes, thin);
        let run = analyze(&f);
        assert_eq!(run.state, RunState::Completed, "{run:?}");
        let analysis = run.analysis.unwrap();
        let mut request = InterfaceQuery {
            input: InterfaceInput::Analysis {
                analysis: analysis.clone(),
                abi: Some(CallAbi::RiscvInteger),
            },
            knowledge: None,
        };
        let discovered = observe(&f, &request);
        assert_eq!(
            discovered["records"].as_array().unwrap().len(),
            1,
            "{discovered}"
        );
        let observed = &discovered["records"][0]["value"];
        assert_eq!(observed["issue"], serde_json::Value::Null);
        let path: InterfaceAccessPath =
            serde_json::from_value(observed["paths"][0].clone()).unwrap();
        assert_eq!(
            path.root,
            InterfaceRoot::FunctionArgument {
                function: f.request.selector.clone(),
                argument: 0
            }
        );
        assert_eq!(path.path, [InterfaceStep::LoadPointer { offset: 0 }]);
        assert_eq!(path.slot, 8);
        let mut contract = declaration(path.root, payload);
        contract.path = path.path;
        contract.slots[0].offset = path.slot;
        let mut proposal = KnowledgeProposal {
            subject: "fixture.observed-interface".to_owned().try_into().unwrap(),
            occurrence: KnowledgeOccurrence {
                revision: f.revision.clone(),
                source: f.request.source.clone(),
                object: f.request.selector.object().clone(),
                symbol: f.request.selector.symbol().cloned(),
            },
            claim: KnowledgeClaim::Interface {
                contract: Box::new(contract),
            },
            evidence: vec![EvidenceRef::Analysis {
                analysis: analysis.clone(),
                record: Some(observed["record"].as_u64().unwrap()),
            }],
            note: None,
        };
        let proposed = propose(&f, proposal.clone(), None);
        assert_eq!(proposed.state, RunState::Completed, "{proposed:?}");
        assert!(
            observe(&f, &request)["records"][0]["value"]["bindings"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        request.knowledge = proposed.knowledge.clone();
        let candidate = observe(&f, &request);
        assert_eq!(
            candidate["records"][0]["value"]["bindings"][0]["state"],
            "proposed"
        );
        let id = candidate["records"][0]["value"]["bindings"][0]["assertion"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let rejected = review(&f, proposed.knowledge.unwrap(), id, ReviewDecision::Reject);
        assert_eq!(rejected.state, RunState::Completed);
        request.knowledge = rejected.knowledge.clone();
        assert_eq!(
            observe(&f, &request)["records"][0]["value"]["bindings"][0]["state"],
            "rejected"
        );
        proposal.note = Some("second explicit review".into());
        let proposed = propose(&f, proposal.clone(), rejected.knowledge);
        assert_eq!(proposed.state, RunState::Completed, "{proposed:?}");
        request.knowledge = proposed.knowledge.clone();
        let candidates = observe(&f, &request);
        let id = candidates["records"][0]["value"]["bindings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["state"] == "proposed")
            .unwrap()["assertion"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let accepted = review(&f, proposed.knowledge.unwrap(), id, ReviewDecision::Accept);
        assert_eq!(accepted.state, RunState::Completed);
        request.knowledge = accepted.knowledge.clone();
        let matched = observe(&f, &request);
        assert_eq!(matched["summary"]["summary"]["matched_accepted"], 1);
        assert_eq!(matched["summary"]["summary"]["conditional"], 1);
        assert!(
            matched["records"][0]["value"]["bindings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|b| b["state"] == "accepted"
                    && b["conditions"] == "runtime-conditions-unverified")
        );
        let mut limited = budget();
        limited.max_work_units = Some(1);
        assert!(
            f.app
                .query(
                    &f.project,
                    app::ReadQuery::Interfaces {
                        request: request.clone()
                    },
                    limited
                )
                .is_err()
        );
        fs::remove_file(f.dir.path().join("entry.a")).unwrap();
        fs::remove_file(f.dir.path().join("entry.o")).unwrap();
        assert_eq!(observe(&f, &request)["records"], matched["records"]);
        let input = f.dir.path().join("interface-query.json");
        let output = f.dir.path().join("interfaces.json");
        cli(
            &f,
            &[
                "interfaces",
                "--request",
                input.to_str().unwrap(),
                "--output",
                output.to_str().unwrap(),
            ],
        );
        let exported: serde_json::Value =
            serde_json::from_slice(&fs::read(&output).unwrap()).unwrap();
        assert_eq!(exported["records"], matched["records"]);
        // A conflicting accepted interpretation cannot replace this contract implicitly.
        let KnowledgeClaim::Interface { contract } = &mut proposal.claim else {
            panic!()
        };
        contract.layout_version = "different/2".into();
        let proposed = propose(&f, proposal, accepted.knowledge);
        assert_eq!(proposed.state, RunState::Completed);
        let entries = cli(&f, &["knowledge", "show"]);
        let id = entries["records"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["value"]["state"] == "proposed")
            .unwrap()["value"]["id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let failed = review(&f, proposed.knowledge.unwrap(), id, ReviewDecision::Accept);
        assert_eq!(failed.state, RunState::Failed);
        assert!(failed.knowledge.is_none());
        assert_eq!(observe(&f, &request)["records"], matched["records"]);
    }
}
#[test]
fn captured_symbol_less_pointer_slots_match_only_selected_structural_declarations() {
    use object::{Object as _, ObjectSection as _};
    let bytes = data::data_object_bytes(false, &[0; 16]);
    let payload = ArtifactId::of_bytes(&bytes);
    let elf = object::File::parse(bytes.as_slice()).unwrap();
    let section = elf.section_by_name(".rodata.table").unwrap().index().0 as u32;
    let f = fixture(bytes.clone(), true);
    let occurrence = data::request(&f, &[b"table"]).occurrence;
    let mut request = InterfaceQuery {
        input: InterfaceInput::Data {
            occurrence: Box::new(occurrence.clone()),
            selector: DataSelector::Section {
                section,
                offset: 0,
                length: 16,
            },
            layout: PointerTable {
                count: 4,
                stride: 4,
            },
        },
        knowledge: None,
    };
    let raw = observe(&f, &request);
    assert_eq!(raw["records"].as_array().unwrap().len(), 4);
    let mut contract = declaration(
        InterfaceRoot::Section { section, offset: 0 },
        payload.clone(),
    );
    contract.slots[0].signature = None;
    contract.slots[0].semantic = None;
    let proposal = KnowledgeProposal {
        subject: "fixture.raw-slots".to_owned().try_into().unwrap(),
        occurrence,
        claim: KnowledgeClaim::Interface {
            contract: Box::new(contract),
        },
        evidence: vec![EvidenceRef::Source {
            payload,
            range: CodeRange {
                start: 0,
                length: bytes.len() as u64,
            },
        }],
        note: None,
    };
    let run = propose(&f, proposal, None);
    assert_eq!(run.state, RunState::Completed, "{run:?}");
    let entries = cli(&f, &["knowledge", "show"]);
    let id = entries["records"][0]["value"]["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    request.knowledge = review(&f, run.knowledge.unwrap(), id, ReviewDecision::Accept).knowledge;
    let observed = observe(&f, &request);
    assert_eq!(observed["summary"]["summary"]["matched_accepted"], 1);
    assert_eq!(observed["records"][1]["value"]["pointer"]["kind"], "null");
    assert!(observed["records"][1]["value"]["bindings"][0]["signature"].is_null());
    assert_eq!(
        observed["summary"]["summary"]["captured_span"]["writable"],
        false
    );
}

#[test]
fn finite_table_roots_keep_both_accepted_bindings_without_selecting_a_callee() {
    let words = [
        0x00050663u32,
        0x000012b7,
        0x0080006f,
        0x000022b7,
        0x0082a303,
        0x000300e7,
        0x00008067,
    ];
    let code: Vec<_> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let bytes = object(&code, code.len() as u64, false);
    let payload = ArtifactId::of_bytes(&bytes);
    let f = fixture(bytes, false);
    let result = analyze(&f);
    assert_eq!(result.state, RunState::Completed, "{result:?}");
    let analysis = result.analysis.unwrap();
    let mut query = InterfaceQuery {
        input: InterfaceInput::Analysis {
            analysis: analysis.clone(),
            abi: None,
        },
        knowledge: None,
    };
    let initial = observe(&f, &query);
    let observation = &initial["records"][0]["value"];
    let paths: Vec<InterfaceAccessPath> =
        serde_json::from_value(observation["paths"].clone()).unwrap();
    assert_eq!(paths.len(), 2, "{initial}");
    assert_eq!(paths[0].root, InterfaceRoot::Address { address: 0x1008 });
    assert_eq!(paths[1].root, InterfaceRoot::Address { address: 0x2008 });
    for (i, path) in paths.into_iter().enumerate() {
        let mut contract = declaration(path.root, payload.clone());
        contract.path = path.path;
        contract.slots[0].offset = path.slot;
        contract
            .guards
            .retain(|g| matches!(g, InterfaceGuard::CapturedPayload { .. }));
        let proposal = KnowledgeProposal {
            subject: format!("fixture.finite-interface-{i}").try_into().unwrap(),
            occurrence: KnowledgeOccurrence {
                revision: f.revision.clone(),
                source: f.request.source.clone(),
                object: f.request.selector.object().clone(),
                symbol: f.request.selector.symbol().cloned(),
            },
            claim: KnowledgeClaim::Interface {
                contract: Box::new(contract),
            },
            evidence: vec![EvidenceRef::Analysis {
                analysis: analysis.clone(),
                record: Some(observation["record"].as_u64().unwrap()),
            }],
            note: None,
        };
        let proposed = propose(&f, proposal, query.knowledge.take());
        assert_eq!(proposed.state, RunState::Completed, "{proposed:?}");
        query.knowledge = proposed.knowledge.clone();
        let candidates = observe(&f, &query);
        let id = candidates["records"][0]["value"]["bindings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["state"] == "proposed")
            .unwrap()["assertion"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let accepted = review(&f, proposed.knowledge.unwrap(), id, ReviewDecision::Accept);
        assert_eq!(accepted.state, RunState::Completed, "{accepted:?}");
        query.knowledge = accepted.knowledge;
    }
    let matched = observe(&f, &query);
    assert_eq!(matched["summary"]["summary"]["matched_accepted"], 2);
    assert_eq!(matched["summary"]["summary"]["ambiguous_bindings"], 1);
    assert_eq!(
        matched["records"][0]["value"]["paths"],
        observation["paths"]
    );
    assert_eq!(
        matched["records"][0]["value"]["target"],
        observation["target"]
    );
}
