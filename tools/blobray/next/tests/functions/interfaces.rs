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
            signature: InterfaceSignature {
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
            },
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
