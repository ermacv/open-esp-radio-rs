use super::interfaces::cli;
use super::*;
use std::collections::BTreeMap;
fn lui(rd: u32, value: u32) -> u32 {
    value | rd << 7 | 0x37
}
fn addi(rd: u32, rs: u32, value: i32) -> u32 {
    (value as u32 & 0xfff) << 20 | rs << 15 | rd << 7 | 0x13
}
fn jal(from: u32, to: u32) -> u32 {
    let d = to.wrapping_sub(from);
    ((d >> 20) & 1) << 31
        | ((d >> 1) & 0x3ff) << 21
        | ((d >> 11) & 1) << 20
        | ((d >> 12) & 0xff) << 12
        | 0xef
}
fn code() -> Vec<u8> {
    let bodies: Vec<(&str, u64, Vec<u32>)> = vec![
        (
            "entry",
            0,
            vec![
                lui(10, 0x3000),
                lui(11, 0x4000),
                addi(12, 0, 25),
                jal(12, 0x800),
                0x8067,
            ],
        ),
        (
            "delivery",
            0x100,
            vec![
                lui(10, 0x2000),
                jal(0x104, 0x810),
                lui(5, 0x2000),
                0x0002a583,
                addi(5, 0, 25),
                0x00558463,
                0x8067,
                jal(0x11c, 0x860),
                0x8067,
            ],
        ),
        (
            "registration",
            0x200,
            vec![
                lui(10, 0x4000),
                0x597,
                addi(11, 11, 0x700 - 0x204),
                jal(0x20c, 0x820),
                0x8067,
            ],
        ),
        (
            "consumer",
            0x300,
            vec![
                lui(10, 0x3000),
                lui(11, 0x2000),
                jal(0x308, 0x810),
                lui(5, 0x2000),
                0x0002a503,
                jal(0x314, 0x830),
                0x8067,
            ],
        ),
        (
            "publish",
            0x400,
            vec![
                lui(10, 0x4000),
                addi(11, 0, 7),
                addi(12, 0, 0x55),
                jal(0x40c, 0x800),
                0x8067,
            ],
        ),
        (
            "attach",
            0x500,
            vec![lui(10, 0x4000), addi(11, 0, 3), jal(0x508, 0x840), 0x8067],
        ),
        (
            "subscribe",
            0x600,
            vec![
                lui(11, 0x5000),
                0x297,
                addi(5, 5, 0x700 - 0x604),
                0x0055a023,
                addi(10, 0, 3),
                jal(0x614, 0x850),
                0x8067,
            ],
        ),
        (
            "callback",
            0x700,
            vec![addi(5, 0, 7), 0x00550463, 0x8067, jal(0x70c, 0x860), 0x8067],
        ),
        ("send", 0x800, vec![0x8067]),
        ("receive", 0x810, vec![0x8067]),
        ("register", 0x820, vec![0x8067]),
        ("invoke", 0x830, vec![0x8067]),
        ("attach_service", 0x840, vec![0x8067]),
        ("subscribe_service", 0x850, vec![0x8067]),
        ("handler", 0x860, vec![jal(0x860, 0x870), 0x8067]),
        ("terminal", 0x870, vec![0x8067]),
    ];
    let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = object.add_section(vec![], b".text.routes".to_vec(), SectionKind::Text);
    let mut data = vec![0; 0x880];
    for (name, at, words) in bodies {
        let bytes: Vec<_> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        data[at as usize..at as usize + bytes.len()].copy_from_slice(&bytes);
        let mut symbol = symbol(
            name.as_bytes(),
            SymbolSection::Section(text),
            bytes.len() as u64,
            SymbolKind::Text,
        );
        symbol.value = at;
        object.add_symbol(symbol);
    }
    object.append_section_data(text, &data, 4);
    object.write().unwrap()
}
struct Saved {
    id: FunctionAnalysisId,
    records: Vec<FunctionRecord>,
    selector: FunctionSelector,
}
fn setup() -> (Fixture, BTreeMap<String, Saved>) {
    setup_bytes(code())
}
fn setup_bytes(bytes: Vec<u8>) -> (Fixture, BTreeMap<String, Saved>) {
    let f = fixture(bytes, false);
    let inventory = app::inventory(&f.project, None).unwrap();
    let symbols = &inventory.revision.inputs[0]
        .inventory
        .as_ref()
        .unwrap()
        .objects[1]
        .elf
        .as_ref()
        .unwrap()
        .symbols;
    let mut saved = BTreeMap::new();
    for symbol in symbols
        .iter()
        .filter(|s| s.symbol_type == object::elf::STT_FUNC)
    {
        let name = String::from_utf8(symbol.name.clone().unwrap()).unwrap();
        let mut req = f.request.clone();
        req.selector = symbol.id.clone().into();
        let run = f
            .app
            .start_analyze_function(&f.project, req, budget())
            .unwrap()
            .wait();
        assert_eq!(run.state, RunState::Completed, "{name}: {run:?}");
        let id = run.analysis.unwrap();
        let json = cli(&f, &["analysis", "--id", id.as_str()]);
        let records = json["records"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| serde_json::from_value(r["value"].clone()).unwrap())
            .collect();
        saved.insert(
            name,
            Saved {
                id,
                records,
                selector: symbol.id.clone().into(),
            },
        );
    }
    (f, saved)
}

fn patched(at: usize, word: u32) -> Vec<u8> {
    use object::{Object as _, ObjectSection as _};
    let mut bytes = code();
    let start = object::File::parse(bytes.as_slice())
        .unwrap()
        .section_by_name(".text.routes")
        .unwrap()
        .file_range()
        .unwrap()
        .0 as usize;
    bytes[start + at..start + at + 4].copy_from_slice(&word.to_le_bytes());
    bytes
}
fn check_status(json: &serde_json::Value, name: &str) -> String {
    json["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["value"]["name"] == name)
        .unwrap()["value"]["status"]
        .as_str()
        .unwrap()
        .into()
}
#[test]
fn replaced_or_truncated_callback_registration_is_not_a_valid_broker_binding() {
    for (at, word, check, status) in [
        (
            0x610,
            0x0005a023,
            "callback-store-reaches-subscription",
            "mismatch",
        ),
        (0x608, 0x0ff2f293, "callback-identity", "unresolved"),
    ] {
        let (f, saved) = setup_bytes(patched(at, word));
        let route = routes(&saved).pop().unwrap();
        let observed = query(&f, &route);
        assert_eq!(check_status(&observed, check), status, "{observed}");
    }
}
#[test]
fn a_case_cannot_revisit_its_predicate_to_choose_another_edge() {
    let (f, saved) = setup_bytes(patched(0x708, jal(0x708, 0x704) & !0xf80));
    let mut route = routes(&saved).pop().unwrap();
    let EventRouteMechanism::BrokerSubscription(r) = &mut route.route else {
        panic!()
    };
    r.selector = 8;
    r.case.taken = false;
    let observed = query(&f, &route);
    assert_eq!(
        check_status(&observed, "broker-selector-handler-case"),
        "mismatch",
        "{observed}"
    );
}
fn record(saved: &Saved, at: u64, kind: &str) -> u64 {
    if kind == "call"
        && !saved
            .records
            .iter()
            .any(|r| matches!(r,FunctionRecord::Transfer {offset,..} if *offset==at))
    {
        return saved.records.iter().position(|r|matches!(r,FunctionRecord::Instruction {offset,decoded,..} if *offset==at && matches!(decoded.flow,InstructionFlow::Jump {link:true,..}|InstructionFlow::Indirect {link:true,..}))).unwrap() as u64;
    }
    saved
        .records
        .iter()
        .position(|r| match (kind, r) {
            ("call", FunctionRecord::Transfer { offset, .. })
            | ("memory", FunctionRecord::MemoryAccess { offset, .. })
            | ("condition", FunctionRecord::Condition { offset, .. }) => *offset == at,
            _ => false,
        })
        .unwrap_or_else(|| panic!("missing {kind} at {at:x}: {:?}", saved.records)) as u64
}
fn hop(saved: &BTreeMap<String, Saved>, caller: &str, at: u64, callee: &str) -> FlowHop {
    FlowHop {
        caller: saved[caller].id.clone(),
        record: record(&saved[caller], at, "call"),
        callee: saved[callee].id.clone(),
    }
}
fn site(saved: &BTreeMap<String, Saved>, name: &str, at: u64, kind: &str) -> SavedSite {
    SavedSite {
        analysis: saved[name].id.clone(),
        record: record(&saved[name], at, kind),
    }
}
fn routes(saved: &BTreeMap<String, Saved>) -> Vec<ReviewedEventRoute> {
    let case = |name, at, callsite| RouteCase {
        condition: site(saved, name, at, "condition"),
        taken: true,
        handler: hop(saved, name, callsite, "handler"),
    };
    let mechanisms = vec![
        EventRouteMechanism::SelectorDelivery(Box::new(SelectorDeliveryRoute {
            dispatch: RouteArgument {
                call: hop(saved, "entry", 12, "send"),
                word: 2,
            },
            selector: 25,
            delivery: RouteArgument {
                call: hop(saved, "delivery", 0x104, "receive"),
                word: 0,
            },
            selector_load: site(saved, "delivery", 0x10c, "memory"),
            selector_offset: 0,
            selector_width: 4,
            case: case("delivery", 0x114, 0x11c),
        })),
        EventRouteMechanism::StaticCallback(Box::new(StaticCallbackRoute {
            dispatches: vec![EventDispatch {
                call: hop(saved, "entry", 12, "send"),
                object_word: 1,
                queue_word: 0,
            }],
            registration: CallbackRegistration {
                call: hop(saved, "registration", 0x20c, "register"),
                object_word: 0,
                callback_word: 1,
            },
            receive: EventReceive {
                call: hop(saved, "consumer", 0x308, "receive"),
                queue_word: 0,
                output: EventReceiveOutput::Argument {
                    word: 1,
                    load: site(saved, "consumer", 0x310, "memory"),
                },
            },
            invoke: RouteArgument {
                call: hop(saved, "consumer", 0x314, "invoke"),
                word: 0,
            },
            callback: saved["callback"].id.clone(),
        })),
        EventRouteMechanism::BrokerSubscription(Box::new(BrokerRoute {
            publish: hop(saved, "publish", 0x40c, "send"),
            object_word: 0,
            selector_word: 1,
            selector: 7,
            payload_word: 2,
            domain: BrokerDomain {
                attach: hop(saved, "attach", 0x508, "attach_service"),
                object_word: 0,
                selector_word: 1,
                selector: 3,
            },
            subscribe: hop(saved, "subscribe", 0x614, "subscribe_service"),
            domain_word: 0,
            subscriber_word: 1,
            callback_store: site(saved, "subscribe", 0x60c, "memory"),
            callback_offset: 0,
            callback: saved["callback"].id.clone(),
            callback_selector_word: 0,
            case: case("callback", 0x704, 0x70c),
        })),
    ];
    mechanisms
        .into_iter()
        .map(|route| ReviewedEventRoute {
            mechanism: "fixture.events".to_owned().try_into().unwrap(),
            execution_context: "task/interrupt, conditional".into(),
            applicability: "captured fixture".into(),
            terminal: if matches!(route, EventRouteMechanism::StaticCallback(_)) {
                vec![
                    hop(saved, "callback", 0x70c, "handler"),
                    hop(saved, "handler", 0x860, "terminal"),
                ]
            } else {
                vec![hop(saved, "handler", 0x860, "terminal")]
            },
            upstream: vec![],
            route,
        })
        .collect()
}
fn query(f: &Fixture, route: &ReviewedEventRoute) -> serde_json::Value {
    let path = f.dir.path().join("event-route.json");
    fs::write(
        &path,
        serde_json::to_vec(&EventRouteQuery {
            revision: f.revision.clone(),
            route: route.clone(),
        })
        .unwrap(),
    )
    .unwrap();
    cli(f, &["event-route", "--request", path.to_str().unwrap()])
}
#[test]
fn three_native_event_mechanisms_have_checked_physical_bindings() {
    let (mut f, saved) = setup();
    let mut base = None;
    let mut expected = Vec::new();
    for (i, route) in routes(&saved).into_iter().enumerate() {
        let json = query(&f, &route);
        assert_eq!(json["summary"]["summary"]["unresolved"], 0, "{json}");
        assert_eq!(json["summary"]["summary"]["mismatched"], 0, "{json}");
        assert_eq!(
            json["summary"]["summary"]["conditions"],
            if i == 0 { 5 } else { 6 }
        );
        assert!(json["summary"]["summary"].get("complete").is_none());
        assert_eq!(json["summary"]["summary"]["analyses_read"], [6, 10, 9][i]);
        let root = saved.values().find(|s| &s.id == route.root()).unwrap();
        let proposal = KnowledgeProposal {
            subject: format!("fixture.route{i}").try_into().unwrap(),
            occurrence: KnowledgeOccurrence {
                revision: f.revision.clone(),
                source: f.request.source.clone(),
                object: root.selector.object().clone(),
                symbol: root.selector.symbol().cloned(),
            },
            claim: KnowledgeClaim::EventRoute {
                route: Box::new(route.clone()),
            },
            evidence: vec![EvidenceRef::Analysis {
                analysis: root.id.clone(),
                record: None,
            }],
            note: None,
        };
        for fault in 0..4 {
            let mut bad = proposal.clone();
            let KnowledgeClaim::EventRoute { route: bad_route } = &mut bad.claim else {
                panic!()
            };
            match &mut bad_route.route {
                EventRouteMechanism::SelectorDelivery(r) => match fault {
                    0 => r.selector = 26,
                    1 => r.selector_offset = 4,
                    2 => r.case.taken = false,
                    _ => r.delivery.word = 8,
                },
                EventRouteMechanism::StaticCallback(r) => match fault {
                    0 => r.registration.callback_word = 8,
                    1 => r.callback = saved["terminal"].id.clone(),
                    2 => r.dispatches[0].queue_word = 2,
                    _ => r.invoke.word = 3,
                },
                EventRouteMechanism::BrokerSubscription(r) => match fault {
                    0 => r.domain.selector = 4,
                    1 => r.callback_offset = 4,
                    2 => r.case.taken = false,
                    _ => r.callback_selector_word = 8,
                },
            }
            // Terminal belongs to the declared endpoint, including the deliberately wrong callback.
            if let EventRouteMechanism::StaticCallback(r) = &bad_route.route {
                bad_route.terminal.clear();
                assert!(!r.dispatches.is_empty());
            }
            let preview = query(&f, bad_route);
            assert!(
                preview["summary"]["summary"]["unresolved"]
                    .as_u64()
                    .unwrap()
                    + preview["summary"]["summary"]["mismatched"]
                        .as_u64()
                        .unwrap()
                    > 0,
                "{preview}"
            );
            let failed = super::interfaces::propose(&f, bad, base.clone());
            assert_eq!(failed.state, RunState::Failed, "{failed:?}");
            assert!(failed.knowledge.is_none());
        }
        let proposed = super::interfaces::propose(&f, proposal, base.clone());
        assert_eq!(proposed.state, RunState::Completed, "{proposed:?}");
        let entries = cli(&f, &["knowledge", "show"]);
        let assertion: AssertionId = entries["records"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["value"]["proposal"]["subject"] == format!("fixture.route{i}"))
            .unwrap()["value"]["id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let accepted = super::interfaces::review(
            &f,
            proposed.knowledge.unwrap(),
            assertion,
            ReviewDecision::Accept,
        );
        assert_eq!(accepted.state, RunState::Completed, "{accepted:?}");
        base = accepted.knowledge;
        expected.push((route, json));
    }
    fs::remove_file(f.dir.path().join("entry.o")).unwrap();
    fs::remove_file(f.dir.path().join("entry.a")).unwrap();
    for (i, (route, result)) in expected.iter().enumerate() {
        assert_eq!(&query(&f, route), result);
        let output = f.dir.path().join(format!("route{i}-export.json"));
        cli(
            &f,
            &[
                "event-route",
                "--request",
                f.dir.path().join("event-route.json").to_str().unwrap(),
                "--output",
                output.to_str().unwrap(),
            ],
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&fs::read(output).unwrap()).unwrap(),
            *result
        );
    }
    // A second selected interpretation of the callback cannot be silently chosen.
    let alternate = f
        .app
        .start_analyze_function(
            &f.project,
            FunctionRequest {
                revision: Some(f.revision.clone()),
                source: f.request.source.clone(),
                selector: saved["callback"].selector.clone(),
                extent: Some(CodeRange {
                    start: 0x700,
                    length: 20,
                }),
                research: None,
            },
            budget(),
        )
        .unwrap()
        .wait();
    assert_eq!(alternate.state, RunState::Completed);
    let mut ambiguous = expected[1].0.clone();
    ambiguous.terminal.last_mut().unwrap().callee = alternate.analysis.unwrap();
    let observation = query(&f, &ambiguous);
    assert!(
        observation["records"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["value"]["name"] == "callback-identity"
                && r["value"]["status"] == "unresolved")
    );
    // Cycle, absent site and bad physical layout are invalid requests, never partial success.
    for fault in 0..3 {
        let mut bad = expected[0].0.clone();
        match fault {
            0 => bad.terminal[0].callee = bad.terminal[0].caller.clone(),
            1 => {
                if let EventRouteMechanism::SelectorDelivery(r) = &mut bad.route {
                    r.selector_load.record = u64::MAX;
                }
            }
            _ => {
                if let EventRouteMechanism::SelectorDelivery(r) = &mut bad.route {
                    r.selector_width = 3;
                }
            }
        }
        let error = f
            .app
            .query(
                &f.project,
                app::ReadQuery::EventRoute {
                    request: EventRouteQuery {
                        revision: f.revision.clone(),
                        route: bad,
                    },
                },
                budget(),
            )
            .err()
            .unwrap();
        assert_eq!(error.code, ErrorCode::InvalidRequest, "{error:?}");
    }
    let before = cli(&f, &["knowledge", "show"]);
    for memory in [false, true] {
        let mut limits = budget();
        if memory {
            limits.working_memory_bytes = Some(1024 * 1024);
        } else {
            limits.max_work_units = Some(1);
        }
        let error = f
            .app
            .query(
                &f.project,
                app::ReadQuery::EventRoute {
                    request: EventRouteQuery {
                        revision: f.revision.clone(),
                        route: expected[0].0.clone(),
                    },
                },
                limits,
            )
            .err()
            .unwrap();
        assert_eq!(error.code, ErrorCode::ResourceLimited, "{error:?}");
    }
    assert_eq!(cli(&f, &["knowledge", "show"]), before);
    let output = f.dir.path().join("reviewed-routes.json");
    cli(
        &f,
        &[
            "knowledge",
            "export",
            "--revision",
            base.unwrap().as_str(),
            "--output",
            output.to_str().unwrap(),
        ],
    );
    assert!(fs::read_to_string(output).unwrap().contains("event-route"));
    let backup = f.dir.path().join("routes.backup");
    cli(&f, &["backup", "--output", backup.to_str().unwrap()]);
    f.project = f.dir.path().join("restored");
    cli(&f, &["restore", "--backup", backup.to_str().unwrap()]);
    assert_eq!(cli(&f, &["knowledge", "show"]), before);
    for (route, result) in &expected {
        assert_eq!(&query(&f, route), result);
    }
}
