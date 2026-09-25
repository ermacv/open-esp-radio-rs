use super::*;
fn fixture() -> (Fixture, KnowledgeOccurrence) {
    fixture_code(&[
        0x00008413, 0x00452303, 0x000300e7, 0x00040067, 0x00000013, 0x00700513, 0x00008067,
    ])
}
pub(super) fn fixture_code(code: &[u32]) -> (Fixture, KnowledgeOccurrence) {
    let (mut bytes, _) = super::goals::symbol_elf(code, 0x1000, 0x1014);
    let sections = u32::from_le_bytes(bytes[32..36].try_into().unwrap()) as usize;
    let symbols = u32::from_le_bytes(
        bytes[sections + 3 * 40 + 16..sections + 3 * 40 + 20]
            .try_into()
            .unwrap(),
    ) as usize;
    bytes[symbols + 16 + 8..symbols + 16 + 12]
        .copy_from_slice(&(code.len() as u32 * 4).to_le_bytes());
    let object = ObjectId {
        artifact: ArtifactId::of_bytes(&bytes),
        location: ObjectLocation::Standalone,
    };
    let f = Fixture::from_inputs(vec![bytes]);
    let occurrence = KnowledgeOccurrence {
        revision: f.target.revision.clone(),
        source: f.target.source.clone(),
        object,
        symbol: None,
    };
    (f, occurrence)
}
pub(super) fn contract(root: AccessRoot, payload: ArtifactId) -> InterfaceContract {
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
                value: 1,
                purpose: "fixture ready tag".into(),
            },
        ],
        slots: vec![InterfaceSlot {
            offset: 4,
            name: "callback".into(),
            semantic: Some("fixture.callback".to_owned().try_into().unwrap()),
            signature: Some(CallSignature {
                arguments: vec![CallArgument {
                    role: "fixture.context".to_owned().try_into().unwrap(),
                    value_type: AbiValueType::Pointer { nullable: false },
                }],
                result: AbiValueType::Integer {
                    bits: 32,
                    signed: false,
                },
                variadic: false,
            }),
        }],
        purpose: "synthetic callback table".into(),
        applicability: "selected occurrence and explicit runtime conditions".into(),
    }
}
pub(super) fn review(
    f: &Fixture,
    occurrence: KnowledgeOccurrence,
    contract: InterfaceContract,
) -> InterfaceReview {
    review_as(f, occurrence, contract, None, ReviewDecision::Accept)
}
fn review_as(
    f: &Fixture,
    occurrence: KnowledgeOccurrence,
    contract: InterfaceContract,
    base: Option<KnowledgeRevisionId>,
    decision: ReviewDecision,
) -> InterfaceReview {
    let proposal = KnowledgeProposal {
        subject: "fixture.interface".to_owned().try_into().unwrap(),
        evidence: vec![EvidenceRef::Source {
            payload: occurrence.object.artifact.clone(),
            range: CodeRange {
                start: 0,
                length: 4,
            },
        }],
        occurrence,
        claim: KnowledgeClaim::Interface {
            contract: Box::new(contract),
        },
        note: None,
    };
    let proposed = f
        .app
        .start_knowledge(
            &f.project,
            &KnowledgeChange {
                expected_base: base,
                actor: "fixture".into(),
                reason: "synthetic contract".into(),
                action: KnowledgeAction::Propose { proposal },
            },
            budget(),
        )
        .unwrap()
        .wait();
    assert_eq!(proposed.state, RunState::Completed, "{proposed:?}");
    let shown = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args(["--format", "json", "knowledge", "--project"])
        .arg(&f.project)
        .args([
            "--limit-mode",
            "watchdog",
            "show",
            "--revision",
            proposed.knowledge.as_ref().unwrap().as_str(),
        ])
        .output()
        .unwrap();
    assert!(
        shown.status.success(),
        "{}",
        String::from_utf8_lossy(&shown.stderr)
    );
    let shown: serde_json::Value = serde_json::from_slice(&shown.stdout).unwrap();
    let assertion: AssertionId = shown["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| {
            r["value"]["proposed_in"].as_str()
                == Some(proposed.knowledge.as_ref().unwrap().as_str())
        })
        .expect("new proposal in selected snapshot")["value"]["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let accepted = f
        .app
        .start_knowledge(
            &f.project,
            &KnowledgeChange {
                expected_base: proposed.knowledge,
                actor: "fixture".into(),
                reason: "accept synthetic contract".into(),
                action: KnowledgeAction::Review {
                    assertion: assertion.clone(),
                    decision,
                    supersedes: None,
                },
            },
            budget(),
        )
        .unwrap()
        .wait();
    assert_eq!(accepted.state, RunState::Completed, "{accepted:?}");
    InterfaceReview {
        knowledge: accepted.knowledge.unwrap(),
        assertion,
    }
}
pub(super) fn request(f: &Fixture, review: InterfaceReview) -> ExecutionRequest {
    let mut r = f.request();
    r.max_events = 128;
    r.cases[0].vendor.arguments[0] = Some(0x3000);
    r.cases[0].vendor.tables = vec![RuntimeTable {
        id: "callbacks".into(),
        review,
        lifetime: RegionLifetime::Phase,
        seed: MemorySeed {
            address: 0x3000,
            length: 16,
            fill: None,
            bytes: vec![1],
        },
        slots: vec![RuntimeSlot {
            offset: 4,
            target: RuntimeSlotTarget::Code { address: 0x1014 },
        }],
        pointer_cells: vec![],
    }];
    r.cases[0].replacement = Some(r.cases[0].vendor.clone());
    r
}
pub(super) fn run(f: &Fixture, r: ExecutionRequest) -> (ExecutionManifest, Vec<ExecutionEvidence>) {
    let record = f.run(r, budget());
    assert_eq!(record.state, RunState::Completed, "{record:?}");
    let data = f.read(&record.execution.unwrap());
    (
        serde_json::from_value(data["summary"]["manifest"].clone()).unwrap(),
        data["records"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| serde_json::from_value(r["value"].clone()).unwrap())
            .collect(),
    )
}
fn table(rows: &[ExecutionEvidence], case: u32) -> &RuntimeTableObservation {
    rows.iter()
        .find_map(|r| match r {
            ExecutionEvidence::RuntimeTable {
                case: i,
                replacement: false,
                observation,
            } if *i == case => Some(observation),
            _ => None,
        })
        .unwrap()
}
#[test]
fn all_reviewed_root_forms_execute_captured_callbacks_with_explicit_conditions() {
    for kind in 0..5 {
        let (f, occurrence) = fixture();
        let symbol = SymbolId {
            object: occurrence.object.clone(),
            table: SymbolTableKind::Static,
            table_section: 3,
            index: 1,
        };
        let root = match kind {
            0 => AccessRoot::Address { address: 0x3000 },
            1 => AccessRoot::Section {
                section: 1,
                offset: 0,
            },
            2 => AccessRoot::Symbol {
                symbol: symbol.clone(),
                addend: 0,
            },
            3 => AccessRoot::EntryWord {
                function: FunctionSelector::Symbol { symbol },
                word: 0,
            },
            _ => AccessRoot::EntryWord {
                function: FunctionSelector::Range {
                    object: occurrence.object.clone(),
                    section: 1,
                    extent: CodeRange {
                        start: 0x1000,
                        length: 28,
                    },
                },
                word: 0,
            },
        };
        let mut c = contract(root, occurrence.object.artifact.clone());
        if matches!(kind, 1 | 2) {
            c.path.push(AccessStep::Offset { bytes: 0x2000 });
        }
        let selection = review(&f, occurrence, c);
        let (m, rows) = run(&f, request(&f, selection));
        assert!(m.complete, "{rows:?}");
        assert_eq!(m.verdict, Some(ComparisonVerdict::Match));
        assert_eq!(table(&rows, 0).initialized, 1);
        assert_eq!(table(&rows, 0).calls, 1);
        assert!(table(&rows, 0).conditions_checked);
        assert!(rows.iter().any(|r| matches!(
            r,
            ExecutionEvidence::Outcome {
                stop: ExecutionStop::Returned { low: Some(7), .. },
                ..
            }
        )));
    }
}
#[test]
fn pointer_installation_and_bounded_index_paths_use_owned_memory() {
    let (f, occurrence) = fixture();
    let mut c = contract(
        AccessRoot::Address { address: 0x4000 },
        occurrence.object.artifact.clone(),
    );
    c.path = vec![
        AccessStep::Index { word: 1, stride: 4 },
        AccessStep::LoadPointer { offset: 0 },
    ];
    c.index_domains = vec![InterfaceIndexDomain {
        word: 1,
        min: 0,
        max: 1,
        reason: "one of two pointer cells".into(),
    }];
    let selection = review(&f, occurrence, c);
    let mut r = request(&f, selection);
    r.cases[0].vendor.arguments[1] = Some(1);
    r.cases[0].vendor.memory = vec![ExecutionRegion {
        lifetime: RegionLifetime::Phase,
        seed: MemorySeed {
            address: 0x4000,
            length: 8,
            fill: None,
            bytes: vec![],
        },
    }];
    r.cases[0].vendor.tables[0].pointer_cells = vec![0x4004];
    r.cases[0].replacement = Some(r.cases[0].vendor.clone());
    let (m, rows) = run(&f, r.clone());
    assert!(m.complete);
    assert_eq!(table(&rows, 0).pointer_installs, 1);
    r.cases[0].vendor.arguments[1] = Some(2);
    r.cases[0].replacement = Some(r.cases[0].vendor.clone());
    let (m, rows) = run(&f, r);
    assert!(!m.complete);
    assert_eq!(
        table(&rows, 0).issue,
        Some(RuntimeTableIssue::IndexPrecondition { word: 1 })
    );
}

#[test]
fn partial_slot_writes_update_target_associations_and_unknowns_do_not_become_callbacks() {
    for (byte, issue, expected) in [
        (0x20, None, Some(42)),
        (
            0x80,
            Some(RuntimeTableIssue::UnavailableTarget { target: 0x1080 }),
            None,
        ),
    ] {
        let code = [
            0x00008413,
            (byte << 20) | 0x00000593,
            0x00b50223,
            0x00452303,
            0x000300e7,
            0x00040067,
            0x00000013,
            0x00000013,
            0x02a00513,
            0x00008067,
        ];
        let (f, occurrence) = fixture_code(&code);
        let c = contract(
            AccessRoot::Address { address: 0x3000 },
            occurrence.object.artifact.clone(),
        );
        let selection = review(&f, occurrence, c);
        let (m, rows) = run(&f, request(&f, selection));
        assert_eq!(m.complete, expected.is_some());
        assert_eq!(table(&rows, 0).writes, 1);
        assert_eq!(table(&rows, 0).issue, issue);
        if let Some(expected) = expected {
            assert!(rows.iter().any(|r|matches!(r,ExecutionEvidence::Outcome{stop:ExecutionStop::Returned{low:Some(n),..},..} if *n==expected)));
        }
        assert!(rows.iter().any(|r|matches!(r,ExecutionEvidence::Event{event:ExecutionEvent::RuntimeTable{event:RuntimeTableEvent::Written{offset:4,width:1,value,..},..},..} if *value==byte)));
    }
    let (f, occurrence) = fixture_code(&[
        0x00008413, 0x00c52303, 0x000300e7, 0x00040067, 0x00000013, 0x00700513, 0x00008067,
    ]);
    let c = contract(
        AccessRoot::Address { address: 0x3000 },
        occurrence.object.artifact.clone(),
    );
    let selection = review(&f, occurrence, c);
    let (m, rows) = run(&f, request(&f, selection));
    assert!(!m.complete);
    assert!(rows.iter().any(|r| matches!(
        r,
        ExecutionEvidence::Outcome {
            // The unknown slot loads an unknown target, which cannot be called.
            stop: ExecutionStop::Incomplete {
                reason: ExecutionGap::UnknownRegister { register: 6 },
                ..
            },
            ..
        }
    )));
    assert_eq!(table(&rows, 0).calls, 0);
}
#[test]
fn null_alias_missing_model_and_guard_failures_never_claim_interface_completion() {
    for variant in 0..5 {
        let (f, occurrence) = fixture();
        let mut c = contract(
            AccessRoot::Address { address: 0x3000 },
            occurrence.object.artifact.clone(),
        );
        if variant == 1 {
            let mut alias = c.slots[0].clone();
            alias.offset = 8;
            alias.name = "alias".into();
            c.slots.push(alias);
        }
        let selection = review(&f, occurrence, c);
        let mut r = request(&f, selection);
        let expected = match variant {
            0 => {
                r.cases[0].vendor.tables[0].slots[0].target = RuntimeSlotTarget::Null;
                RuntimeTableIssue::NullTarget
            }
            1 => {
                r.cases[0].vendor.tables[0].slots.push(RuntimeSlot {
                    offset: 8,
                    target: RuntimeSlotTarget::Code { address: 0x1014 },
                });
                RuntimeTableIssue::AmbiguousTarget {
                    target: 0x1014,
                    candidates: 2,
                }
            }
            2 => {
                r.cases[0].vendor.tables[0].slots[0].target =
                    RuntimeSlotTarget::Model { address: 0x2000 };
                RuntimeTableIssue::UnavailableTarget { target: 0x2000 }
            }
            3 => {
                r.cases[0].vendor.tables[0].seed.bytes.clear();
                RuntimeTableIssue::GuardUnknown { offset: 0 }
            }
            _ => {
                r.cases[0].vendor.tables[0].seed.bytes[0] = 0;
                RuntimeTableIssue::GuardMismatch {
                    offset: 0,
                    actual: 0,
                }
            }
        };
        r.cases[0].replacement = Some(r.cases[0].vendor.clone());
        let (m, rows) = run(&f, r);
        assert!(!m.complete);
        assert_eq!(m.verdict, Some(ComparisonVerdict::Incomplete));
        assert_eq!(table(&rows, 0).issue, Some(expected));
        assert_eq!(table(&rows, 0).status, ModelStatus::Incomplete);
    }
}
#[test]
fn reviewed_modeled_callbacks_keep_session_lifetimes_and_restore_exactly() {
    let (f, occurrence) = fixture();
    let c = contract(
        AccessRoot::Address { address: 0x3000 },
        occurrence.object.artifact.clone(),
    );
    let selection = review(&f, occurrence, c);
    let mut r = request(&f, selection);
    r.cases[0].vendor.tables[0].lifetime = RegionLifetime::Session;
    r.cases[0].vendor.tables[0].slots[0].target = RuntimeSlotTarget::Model { address: 0x2000 };
    r.cases[0].vendor.calls = vec![CallDeclaration {
        repetition: blobray_domain::CallRepetition::Finite,
        id: "callback-model".into(),
        applicability: "synthetic reviewed callback".into(),
        lifetime: RegionLifetime::Session,
        binding: CallBinding {
            address: 0x2000,
            boundary: CallBoundary::Unmapped,
            allow_tail: false,
        },
        argument_words: 1,
        responses: vec![
            CallResponse {
                return_words: [Some(7), None],
                outputs: vec![],
                allocation: None,
                delay_micros: None,
            },
            CallResponse {
                return_words: [Some(9), None],
                outputs: vec![],
                allocation: None,
                delay_micros: None,
            },
        ],
    }];
    r.cases[0].replacement = Some(r.cases[0].vendor.clone());
    let mut warm = r.cases[0].clone();
    warm.reset = SessionReset::Warm;
    warm.name = "warm".into();
    warm.vendor.tables.clear();
    warm.vendor.calls.clear();
    warm.replacement = Some(warm.vendor.clone());
    r.cases.push(warm);
    let record = f.run(r.clone(), budget());
    assert_eq!(record.state, RunState::Completed, "{record:?}");
    assert_eq!(
        record
            .diagnostics
            .as_ref()
            .unwrap()
            .progress
            .as_ref()
            .unwrap()
            .measurements
            .objects_prepared,
        1
    );
    let id = record.execution.unwrap();
    let original = f.read(&id);
    let (m, rows) = run(&f, r.clone());
    assert!(m.complete);
    assert_eq!(table(&rows, 0).status, ModelStatus::Open);
    assert_eq!(table(&rows, 1).calls, 2);
    assert_eq!(table(&rows, 1).status, ModelStatus::Complete);
    let request_path = f._dir.path().join("interfaces.json");
    fs::write(&request_path, serde_json::to_vec(&r).unwrap()).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args(["--format", "json", "compare", "--project"])
        .arg(&f.project)
        .arg("--request")
        .arg(&request_path)
        .args(["--limit-mode", "watchdog"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(output["run"]["execution"], id.as_str());
    let backup = f._dir.path().join("interfaces.blobray");
    let restored = f._dir.path().join("restored");
    for (command, project, flag, path) in [
        ("backup", &f.project, "--output", &backup),
        ("restore", &restored, "--backup", &backup),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_blobray"))
            .args([command, "--project"])
            .arg(project)
            .arg(flag)
            .arg(path)
            .args(["--limit-mode", "watchdog"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let replay = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args(["--format", "json", "replay", "--project"])
        .arg(&restored)
        .args(["--id", id.as_str(), "--limit-mode", "watchdog"])
        .output()
        .unwrap();
    assert!(
        replay.status.success(),
        "{}",
        String::from_utf8_lossy(&replay.stderr)
    );
    let replay: serde_json::Value = serde_json::from_slice(&replay.stdout).unwrap();
    assert_eq!(replay["run"]["execution"], id.as_str());
    let saved = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args(["--format", "json", "execution", "--project"])
        .arg(&restored)
        .args(["--id", id.as_str(), "--limit-mode", "watchdog"])
        .output()
        .unwrap();
    assert!(saved.status.success());
    let saved: serde_json::Value = serde_json::from_slice(&saved.stdout).unwrap();
    assert_eq!(saved["summary"], original["summary"]);
    assert_eq!(saved["records"], original["records"]);
}

#[test]
fn model_outputs_change_live_slots_before_the_next_indirect_call() {
    let (f, occurrence) = fixture_code(&[
        0x00008413, 0x00050493, 0x00452303, 0x000300e7, 0x00048513, 0x00452303, 0x000300e7,
        0x00040067, 0x00000013, 0x02a00513, 0x00008067,
    ]);
    let c = contract(
        AccessRoot::Address { address: 0x3000 },
        occurrence.object.artifact.clone(),
    );
    let selection = review(&f, occurrence, c);
    let mut r = request(&f, selection);
    r.cases[0].vendor.tables[0].slots[0].target = RuntimeSlotTarget::Model { address: 0x2000 };
    r.cases[0].vendor.calls = vec![CallDeclaration {
        repetition: blobray_domain::CallRepetition::Finite,
        id: "install-callback".into(),
        applicability: "synthetic slot update".into(),
        lifetime: RegionLifetime::Phase,
        binding: CallBinding {
            address: 0x2000,
            boundary: CallBoundary::Unmapped,
            allow_tail: false,
        },
        argument_words: 1,
        responses: vec![CallResponse {
            return_words: [None, None],
            outputs: vec![CallOutput {
                pointer_argument: 0,
                byte_offset: 4,
                width: 4,
                value: 0x1024,
                scope: CallOutputScope::NormalMemory,
            }],
            allocation: None,
            delay_micros: None,
        }],
    }];
    r.cases[0].replacement = Some(r.cases[0].vendor.clone());
    let (m, rows) = run(&f, r);
    assert!(m.complete);
    assert_eq!(table(&rows, 0).writes, 1);
    assert_eq!(table(&rows, 0).calls, 2);
    assert!(rows.iter().any(|r| matches!(
        r,
        ExecutionEvidence::Outcome {
            stop: ExecutionStop::Returned { low: Some(42), .. },
            ..
        }
    )));
}
#[test]
fn phase_tables_release_bytes_and_ids_before_repeated_warm_placement() {
    let (f, occurrence) = fixture();
    let mut c = contract(
        AccessRoot::Address { address: 0x3000 },
        occurrence.object.artifact.clone(),
    );
    c.layout_bytes = 4 * 1024 * 1024;
    let selection = review(&f, occurrence, c);
    let mut r = request(&f, selection);
    r.cases[0].vendor.tables[0].seed.length = 4 * 1024 * 1024;
    r.cases[0].replacement = Some(r.cases[0].vendor.clone());
    // Keep the private stack outside the deliberately large table owner.
    r.vendor.stack.address = 0x800000;
    r.replacement.as_mut().unwrap().stack.address = 0x800000;
    let mut warm = r.cases[0].clone();
    warm.reset = SessionReset::Warm;
    warm.name = "fresh-phase-table".into();
    r.cases.push(warm);
    let (m, rows) = run(&f, r);
    assert!(m.complete);
    assert_eq!(table(&rows, 0).status, ModelStatus::Complete);
    assert_eq!(table(&rows, 1).calls, 1);
    assert_eq!(table(&rows, 1).instance, table(&rows, 0).instance);
}
#[test]
fn absolute_symbol_roots_remain_physical_selected_identities() {
    let code = [
        0x00008413, 0x00452303, 0x000300e7, 0x00040067, 0x00000013, 0x00700513, 0x00008067,
    ];
    let (mut bytes, _) = super::goals::symbol_elf(&code, 0x1000, 0x1014);
    let sections = u32::from_le_bytes(bytes[32..36].try_into().unwrap()) as usize;
    let symbols = u32::from_le_bytes(
        bytes[sections + 3 * 40 + 16..sections + 3 * 40 + 20]
            .try_into()
            .unwrap(),
    ) as usize;
    bytes[symbols + 4 * 16 + 4..symbols + 4 * 16 + 8].copy_from_slice(&0x3000u32.to_le_bytes());
    bytes[symbols + 4 * 16 + 14..symbols + 4 * 16 + 16].copy_from_slice(&0xfff1u16.to_le_bytes());
    let object = ObjectId {
        artifact: ArtifactId::of_bytes(&bytes),
        location: ObjectLocation::Standalone,
    };
    let f = Fixture::from_inputs(vec![bytes]);
    let occurrence = KnowledgeOccurrence {
        revision: f.target.revision.clone(),
        source: f.target.source.clone(),
        object: object.clone(),
        symbol: None,
    };
    let c = contract(
        AccessRoot::Symbol {
            symbol: SymbolId {
                object,
                table: SymbolTableKind::Static,
                table_section: 3,
                index: 4,
            },
            addend: 0,
        },
        occurrence.object.artifact.clone(),
    );
    let selection = review(&f, occurrence, c);
    assert!(run(&f, request(&f, selection)).0.complete);
}
#[test]
fn rejected_review_layout_ownership_and_limits_preserve_saved_execution() {
    let (f, occurrence) = fixture();
    let c = contract(
        AccessRoot::Address { address: 0x3000 },
        occurrence.object.artifact.clone(),
    );
    let selection = review(&f, occurrence.clone(), c.clone());
    let base = request(&f, selection.clone());
    let retained = f.run(base.clone(), budget()).execution.unwrap();
    for variant in 0..4 {
        let mut r = base.clone();
        match variant {
            0 => r.cases[0].vendor.tables[0].seed.length = 32,
            1 => {
                r.cases[0].vendor.memory = vec![ExecutionRegion {
                    lifetime: RegionLifetime::Phase,
                    seed: MemorySeed {
                        address: 0x3000,
                        length: 16,
                        fill: None,
                        bytes: vec![],
                    },
                }]
            }
            2 => r.cases[0].vendor.tables[0].pointer_cells.push(0x1000),
            3 => {
                r.cases[0].vendor.tables[0].lifetime = RegionLifetime::Session;
                r.cases[0].replacement = Some(r.cases[0].vendor.clone());
                let mut warm = r.cases[0].clone();
                warm.name = "duplicate-live".into();
                warm.reset = SessionReset::Warm;
                r.cases.push(warm);
            }
            _ => unreachable!(),
        }
        let record = f.run(r, budget());
        assert!(matches!(
            record.error.unwrap().code,
            ErrorCode::InvalidRequest | ErrorCode::Conflict
        ));
        assert!(record.execution.is_none());
    }
    let mut r = base.clone();
    r.max_events = 1;
    let record = f.run(r, budget());
    assert_eq!(record.error.unwrap().code, ErrorCode::ResourceLimited);
    assert!(record.execution.is_none());
    let mut b = budget();
    b.max_work_units = Some(1);
    let record = f.run(base.clone(), b);
    assert_eq!(record.error.unwrap().code, ErrorCode::ResourceLimited);
    assert!(record.execution.is_none());
    let handle = f
        .app
        .start_execution(
            &f.project,
            base.clone(),
            &blobray_backend_riscv::RiscvExecutor,
            budget(),
        )
        .unwrap();
    handle.cancel();
    assert_eq!(handle.wait().state, RunState::Cancelled);
    // Accepted assertions are immutable; reject a new pending proposal at the next head.
    let rejected = review_as(
        &f,
        occurrence,
        c,
        Some(selection.knowledge),
        ReviewDecision::Reject,
    );
    let mut r = base;
    r.cases[0].vendor.tables[0].review = rejected;
    let record = f.run(r, budget());
    assert_eq!(record.error.unwrap().code, ErrorCode::InvalidRequest);
    assert!(record.execution.is_none());
    assert_eq!(f.read(&retained)["summary"]["manifest"]["complete"], true);
}

#[test]
fn root_unknown_overflow_and_missing_pointer_are_explicit_conditions() {
    for variant in 0..4 {
        let (f, occurrence) = fixture();
        let root = match variant {
            0 | 1 => AccessRoot::EntryWord {
                function: FunctionSelector::Range {
                    object: occurrence.object.clone(),
                    section: 1,
                    extent: CodeRange {
                        start: 0x1000,
                        length: 28,
                    },
                },
                word: 0,
            },
            2 => AccessRoot::Address { address: 0x4000 },
            _ => AccessRoot::Address { address: 0x3004 },
        };
        let mut c = contract(root, occurrence.object.artifact.clone());
        if variant == 1 {
            c.path.push(AccessStep::Offset { bytes: 4 });
        }
        if variant == 2 {
            c.path.push(AccessStep::LoadPointer { offset: 0 });
        }
        let selected = review(&f, occurrence, c);
        let mut r = request(&f, selected);
        if variant < 2 {
            r.cases[0].vendor.arguments[0] = (variant == 1).then_some(0xffff_fffc);
        }
        r.cases[0].replacement = Some(r.cases[0].vendor.clone());
        let (m, rows) = run(&f, r);
        assert!(!m.complete);
        let expected = match variant {
            0 => RuntimeTableIssue::UnknownRoot,
            1 => RuntimeTableIssue::RootAddressOverflow,
            2 => RuntimeTableIssue::UnknownPointer { address: 0x4000 },
            _ => RuntimeTableIssue::RootMismatch {
                actual: 0x3004,
                expected: 0x3000,
            },
        };
        assert_eq!(table(&rows, 0).issue, Some(expected));
        assert_eq!(table(&rows, 0).calls, 0);
    }
}
#[test]
fn guard_changes_are_checked_before_indirect_use_and_again_after_cold_reset() {
    let (f, occurrence) = fixture_code(&[
        0x00008413, 0x00050023, 0x00452303, 0x000300e7, 0x00040067, 0x00700513, 0x00008067,
    ]);
    let c = contract(
        AccessRoot::Address { address: 0x3000 },
        occurrence.object.artifact.clone(),
    );
    let selected = review(&f, occurrence, c);
    let mut r = request(&f, selected);
    let mut cold = r.cases[0].clone();
    cold.name = "fresh-cold".into();
    r.cases.push(cold);
    let (m, rows) = run(&f, r);
    assert!(!m.complete);
    for phase in 0..2 {
        let o = table(&rows, phase);
        assert_eq!(
            o.issue,
            Some(RuntimeTableIssue::GuardMismatch {
                offset: 0,
                actual: 0
            })
        );
        assert_eq!(o.initialized, 1);
        assert_eq!(o.writes, 1);
        assert_eq!(o.calls, 0);
    }
}

#[test]
fn review_selection_and_model_abi_cannot_be_inferred_from_target_addresses() {
    for variant in 0..5 {
        let (f, occurrence) = fixture();
        let c = contract(
            AccessRoot::Address { address: 0x3000 },
            occurrence.object.artifact.clone(),
        );
        let selected = review(&f, occurrence, c);
        let mut r = request(&f, selected);
        match variant {
            0 => r.cases[0].vendor.tables[0].review.assertion = "f".repeat(64).parse().unwrap(),
            1 => r.vendor.source = FunctionSource::Input { input: 1 },
            2 => r.vendor.revision = "e".repeat(64).parse().unwrap(),
            _ => {
                r.cases[0].vendor.tables[0].slots[0].target =
                    RuntimeSlotTarget::Model { address: 0x2000 };
                r.cases[0].vendor.calls = vec![CallDeclaration {
                    repetition: blobray_domain::CallRepetition::Finite,
                    id: "callback".into(),
                    applicability: "synthetic callback".into(),
                    lifetime: RegionLifetime::Phase,
                    binding: CallBinding {
                        address: 0x2000,
                        boundary: CallBoundary::Unmapped,
                        allow_tail: false,
                    },
                    argument_words: if variant == 3 { 0 } else { 1 },
                    responses: vec![CallResponse {
                        return_words: [Some(7), None],
                        outputs: vec![],
                        allocation: None,
                        delay_micros: None,
                    }],
                }];
                if variant == 4 {
                    r.cases[0].vendor.tables[0].lifetime = RegionLifetime::Session;
                    r.cases[0].replacement = Some(r.cases[0].vendor.clone());
                    let mut warm = r.cases[0].clone();
                    warm.name = "expired-call-model".into();
                    warm.reset = SessionReset::Warm;
                    warm.vendor.calls.clear();
                    warm.vendor.tables.clear();
                    warm.replacement = Some(warm.vendor.clone());
                    r.cases.push(warm);
                }
            }
        }
        if variant < 3 {
            let result = f.run(r, budget());
            assert!(matches!(
                result.error.unwrap().code,
                ErrorCode::NotFound | ErrorCode::InvalidRequest
            ));
            assert!(result.execution.is_none());
        } else {
            r.cases[0].replacement = Some(r.cases[0].vendor.clone());
            let (m, rows) = run(&f, r);
            assert!(!m.complete);
            let phase = if variant == 3 { 0 } else { 1 };
            let issue = if variant == 3 {
                RuntimeTableIssue::ModelAbi { target: 0x2000 }
            } else {
                RuntimeTableIssue::UnavailableTarget { target: 0x2000 }
            };
            assert_eq!(table(&rows, phase).issue, Some(issue));
        }
    }
}

#[test]
fn shared_snapshot_preparation_scales_and_restores_phase_order() {
    let (f, occurrence) = fixture();
    let selected = review(
        &f,
        occurrence.clone(),
        contract(
            AccessRoot::Address { address: 0x3000 },
            occurrence.object.artifact,
        ),
    );
    let mut base = request(&f, selected);
    base.replacement = None;
    base.binding = None;
    base.cases[0].replacement = None;
    base.cases[0].relation = None;
    let mut previous = None;
    let mut previous_total = None;
    let mut previous_read = None;
    for count in [16, 32, 64] {
        let mut request = base.clone();
        request.cases = (0..count)
            .map(|n| {
                let mut case = base.cases[0].clone();
                case.name = format!("cold-{n}");
                case.vendor.tables[0].id = format!("table-{n}");
                case.replacement = None;
                case
            })
            .collect();
        let record = f.run(request, budget());
        assert_eq!(record.state, RunState::Completed, "{record:?}");
        let progress = record.diagnostics.unwrap().progress.unwrap();
        assert_eq!(progress.measurements.objects_prepared, 1);
        assert_eq!(progress.measurements.knowledge_history_passes, 2);
        let work = progress.phases.prepare_object.work_units;
        assert!(work > 0);
        if let Some(previous) = previous {
            assert!(work < previous * 3, "{count}: {work}/{previous}");
        }
        previous = Some(work);
        let total = progress.work_used;
        if let Some(previous) = previous_total {
            assert!(
                total < previous * 3,
                "publication {count}: {total}/{previous}"
            );
        }
        previous_total = Some(total);
        let id = record.execution.unwrap();
        let output = f
            .app
            .query(
                &f.project,
                app::ReadQuery::Execution {
                    id: id.clone(),
                    omit_events: false,
                },
                budget(),
            )
            .unwrap();
        let read = output.report.diagnostics.progress.as_ref().unwrap();
        assert_eq!(read.measurements.knowledge_history_passes, 1);
        let read = read.work_used;
        if let Some(previous) = previous_read {
            assert!(read < previous * 3, "reopen {count}: {read}/{previous}");
        }
        previous_read = Some(read);
        drop(output);
        eprintln!("runtime phases={count}, preparation={work}, publication={total}, reopen={read}");
        let saved = f.read(&id);
        let manifest: ExecutionManifest =
            serde_json::from_value(saved["summary"]["manifest"].clone()).unwrap();
        assert!(manifest.complete);
        let rows: Vec<ExecutionEvidence> = saved["records"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| serde_json::from_value(r["value"].clone()).unwrap())
            .collect();
        for n in 0..count {
            let observation = table(&rows, n);
            assert_eq!(observation.id, format!("table-{n}"));
            assert_eq!(observation.status, ModelStatus::Complete);
            assert_eq!(observation.calls, 1);
        }
    }
}

#[test]
fn callback_then_unassociated_captured_jalr_is_incomplete_and_replays() {
    let (f, occurrence) = fixture_code(&[
        0x00008413, // save ra in s0
        0x00452303, // load selected callback
        0x000300e7, // jalr ra,t1
        0x00000297, // ordinary captured call: auipc t0,0 at 0x100c
        0x018280e7, // jalr ra,t0,0x18 => 0x1024 (no selected slot)
        0x00040067, // return through s0
        0x00000013, 0x00700513, // callback at 0x101c
        0x00008067, 0x00900513, // ordinary function at 0x1024
        0x00008067,
    ]);
    let selected = review(
        &f,
        occurrence.clone(),
        contract(
            AccessRoot::Address { address: 0x3000 },
            occurrence.object.artifact,
        ),
    );
    let mut request = request(&f, selected);
    request.cases[0].vendor.tables[0].slots[0].target = RuntimeSlotTarget::Code { address: 0x101c };
    request.cases[0].replacement = Some(request.cases[0].vendor.clone());
    let record = f.run(request, budget());
    assert_eq!(record.state, RunState::Completed, "{record:?}");
    let id = record.execution.unwrap();
    let saved = f.read(&id);
    let manifest: ExecutionManifest =
        serde_json::from_value(saved["summary"]["manifest"].clone()).unwrap();
    assert!(!manifest.complete);
    assert_eq!(manifest.verdict, Some(ComparisonVerdict::Incomplete));
    let rows: Vec<ExecutionEvidence> = saved["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| serde_json::from_value(r["value"].clone()).unwrap())
        .collect();
    let observed = table(&rows, 0);
    assert_eq!(observed.calls, 1);
    assert_eq!(
        observed.issue,
        Some(RuntimeTableIssue::UnassociatedTarget { target: 0x1024 })
    );
    assert!(rows.iter().any(|r| matches!(
        r,
        ExecutionEvidence::Event {
            event: ExecutionEvent::RuntimeTable {
                event: RuntimeTableEvent::IndirectTarget {
                    site: 0x1008,
                    target: 0x101c,
                    offset: 4
                },
                ..
            },
            ..
        }
    )));
    let replay = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args(["--format", "json", "replay", "--project"])
        .arg(&f.project)
        .args(["--id", id.as_str(), "--limit-mode", "watchdog"])
        .output()
        .unwrap();
    assert!(
        replay.status.success(),
        "{}",
        String::from_utf8_lossy(&replay.stderr)
    );
    let replay: serde_json::Value = serde_json::from_slice(&replay.stdout).unwrap();
    assert_eq!(replay["run"]["execution"], id.as_str());
    assert_eq!(f.read(&id)["records"], saved["records"]);
}

#[test]
fn preparation_groups_each_snapshot_and_physical_object_without_reordering_tables() {
    let code = [
        0x00008413, 0x00452303, 0x000300e7, 0x00040067, 0x00000013, 0x00700513, 0x00008067,
    ];
    let (primary, _) = super::goals::symbol_elf(&code, 0x1000, 0x1014);
    let (companion, _) = super::goals::symbol_elf(&[0x00008067; 7], 0x2000, 0x2014);
    let objects: Vec<_> = [&primary, &companion]
        .into_iter()
        .map(|bytes| ObjectId {
            artifact: ArtifactId::of_bytes(bytes),
            location: ObjectLocation::Standalone,
        })
        .collect();
    let mut f = Fixture::from_inputs(vec![primary, companion]);
    f.target.companions.push(1);
    let occurrence = |input: usize| KnowledgeOccurrence {
        revision: f.target.revision.clone(),
        source: FunctionSource::Input {
            input: input as u64,
        },
        object: objects[input].clone(),
        symbol: None,
    };
    let old = review(
        &f,
        occurrence(0),
        contract(
            AccessRoot::Address { address: 0x3000 },
            objects[0].artifact.clone(),
        ),
    );
    let new = review_as(
        &f,
        occurrence(1),
        contract(
            AccessRoot::Address { address: 0x4000 },
            objects[1].artifact.clone(),
        ),
        Some(old.knowledge.clone()),
        ReviewDecision::Accept,
    );
    let mut first = old.clone();
    first.knowledge = new.knowledge.clone();
    let mut request = request(&f, first);
    let mut second = request.cases[0].vendor.tables[0].clone();
    second.id = "companion".into();
    second.review = new;
    second.seed.address = 0x4000;
    second.slots[0].target = RuntimeSlotTarget::Null;
    request.cases[0].vendor.tables.insert(0, second);
    request.cases[0].replacement = Some(request.cases[0].vendor.clone());
    let mut later = request.cases[0].clone();
    later.name = "older-snapshot".into();
    later.vendor.tables.remove(0);
    later.vendor.tables[0].review = old;
    later.replacement = Some(later.vendor.clone());
    request.cases.push(later);
    let record = f.run(request.clone(), budget());
    assert_eq!(record.state, RunState::Completed, "{record:?}");
    let progress = record.diagnostics.unwrap().progress.unwrap();
    assert_eq!(progress.measurements.objects_prepared, 3);
    assert_eq!(progress.measurements.knowledge_history_passes, 4);
    let saved = f.read(&record.execution.unwrap());
    let manifest: ExecutionManifest =
        serde_json::from_value(saved["summary"]["manifest"].clone()).unwrap();
    assert!(manifest.complete);
    let rows: Vec<ExecutionEvidence> = saved["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| serde_json::from_value(r["value"].clone()).unwrap())
        .collect();
    let tables: Vec<_> = rows
        .iter()
        .filter_map(|r| match r {
            ExecutionEvidence::RuntimeTable {
                case,
                replacement: false,
                observation,
            } => Some((*case, observation.id.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(
        tables,
        [(0, "companion"), (0, "callbacks"), (1, "callbacks")]
    );
    // A later request in a shared object group still undergoes individual validation.
    request.cases[1].vendor.tables[0].seed.length = 32;
    let failed = f.run(request, budget());
    assert_eq!(failed.error.unwrap().code, ErrorCode::InvalidRequest);
    assert!(failed.execution.is_none());
}
