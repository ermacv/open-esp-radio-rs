use super::*;
#[derive(Default)]
struct Sink(Vec<TraceRecord>);
impl ElfSink for Sink {
    fn section(&mut self, _: &SectionRecord, _: &mut dyn RunControl) -> Result<()> {
        unreachable!()
    }
    fn symbol(&mut self, _: &SymbolRecord, _: &mut dyn RunControl) -> Result<()> {
        unreachable!()
    }
    fn relocation(&mut self, _: &RelocationRecord, _: &mut dyn RunControl) -> Result<()> {
        unreachable!()
    }
    fn diagnostic(&mut self, _: &Diagnostic, _: &mut dyn RunControl) -> Result<()> {
        unreachable!()
    }
}
impl InventorySink for Sink {}
impl app::DoctorSink for Sink {
    fn error(&mut self, _: &Error, _: &mut dyn RunControl) -> Result<()> {
        unreachable!()
    }
    fn unfinished(&mut self, _: &RunId, _: &mut dyn RunControl) -> Result<()> {
        unreachable!()
    }
}
impl app::QuerySink for Sink {
    fn summary(&mut self, _: &app::QuerySummary, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
    fn trace(&mut self, r: &TraceRecord, _: &mut dyn RunControl) -> Result<()> {
        self.0.push(r.clone());
        Ok(())
    }
}
fn target(f: &Fixture, analysis: FunctionAnalysisId) -> TraceTarget {
    let request = IrBuildRequest {
        scope: NavigationScope {
            revision: f.revision.clone(),
            publications: vec![],
            analyses: vec![analysis.clone()],
            knowledge: None,
        },
        profiles: vec![IrProfile {
            name: "trace".into(),
            roots: IrRoots::All,
            include_reachable: true,
        }],
    };
    let run = f
        .app
        .start_build_ir(&f.project, request, budget())
        .unwrap()
        .wait();
    assert_eq!(run.state, RunState::Completed, "{run:?}");
    TraceTarget {
        ir: run.semantic_ir.unwrap(),
        profile: "trace".into(),
        entry: analysis,
        abi: CallAbi::RiscvInteger,
        registers: vec![],
    }
}
fn request(f: &Fixture) -> TraceRequest {
    let run = analyze(f);
    assert_eq!(run.state, RunState::Completed, "{run:?}");
    TraceRequest {
        left: target(f, run.analysis.unwrap()),
        right: None,
        observation: TraceObservation {
            ranges: vec![ImageRegion {
                start: 0x60000000,
                length: 4096,
            }],
            fences: true,
        },
    }
}
fn query(f: &Fixture, q: &TraceRequest) -> (TraceSummary, Vec<TraceRecord>) {
    let mut out = f
        .app
        .query(
            &f.project,
            app::ReadQuery::Trace { request: q.clone() },
            budget(),
        )
        .unwrap();
    let app::QuerySummary::Trace { summary } = out.summary() else {
        panic!("trace")
    };
    let summary = (**summary).clone();
    assert_eq!(out.assessment().comparison, summary.verdict);
    let mut sink = Sink::default();
    out.records(&|| false, &mut sink).unwrap();
    (summary, sink.0)
}
fn two(left: &[u32], right: &[u32]) -> (Fixture, TraceRequest) {
    let mut obj = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    for (name, code) in [(b"entry".as_slice(), left), (b"other".as_slice(), right)] {
        let section = obj.add_section(vec![], [b".text.", name].concat(), SectionKind::Text);
        obj.append_section_data(section, &words(code), 4);
        obj.add_symbol(symbol(
            name,
            SymbolSection::Section(section),
            (code.len() * 4) as u64,
            SymbolKind::Text,
        ));
    }
    let f = fixture(obj.write().unwrap(), false);
    let mut q = request(&f);
    let snapshot = app::inventory(&f.project, None).unwrap();
    let other = snapshot.revision.inputs[0]
        .inventory
        .as_ref()
        .unwrap()
        .objects[1]
        .elf
        .as_ref()
        .unwrap()
        .symbols
        .iter()
        .find(|s| s.name.as_deref() == Some(b"other"))
        .unwrap()
        .id
        .clone();
    let mut req = f.request.clone();
    req.selector = other.into();
    let run = f
        .app
        .start_analyze_function(&f.project, req, budget())
        .unwrap()
        .wait();
    assert_eq!(run.state, RunState::Completed, "{run:?}");
    q.right = Some(target(&f, run.analysis.unwrap()));
    (f, q)
}
#[test]
fn static_trace_observes_ordered_memory_fences_and_symbolic_read_values() {
    // lui t0; lw a1,0(t0); addi a1,a1,1; sw a1,4(t0); fence rw,rw; ret.
    let code = [
        0x600002b7, 0x0002a583, 0x00158593, 0x00b2a223, 0x0330000f, 0x00008067,
    ];
    let (f, q) = two(&code, &code);
    let (summary, rows) = query(&f, &q);
    assert!(summary.left.exact, "{summary:?} {rows:?}");
    assert_eq!(summary.verdict, Some(ComparisonVerdict::Match));
    assert_eq!(summary.left.events, 3);
    let events: Vec<_> = rows
        .iter()
        .filter_map(|r| {
            if let TraceRecord::Event {
                side: TraceSide::Left,
                event,
                ..
            } = r
            {
                Some(*event)
            } else {
                None
            }
        })
        .collect();
    assert!(matches!(
        events.as_slice(),
        [
            TraceEvent::Memory {
                access: MemoryKind::Load,
                address: 0x60000000,
                ..
            },
            TraceEvent::Memory {
                access: MemoryKind::Store,
                address: 0x60000004,
                value: Some(TraceValue::Expression { .. }),
                ..
            },
            TraceEvent::Fence {
                predecessor: 3,
                successor: 3
            }
        ]
    ));
    fs::remove_file(f.dir.path().join("entry.a")).unwrap();
    fs::remove_file(f.dir.path().join("entry.o")).unwrap();
    assert_eq!(query(&f, &q), (summary, rows));

    let backup = f.dir.path().join("trace.blobray");
    f.app
        .query(&f.project, app::ReadQuery::Backup, budget())
        .unwrap()
        .export_backup(&backup, &|| false)
        .unwrap();
    let restored = f.dir.path().join("restored");
    f.app
        .query(
            &restored,
            app::ReadQuery::Restore {
                bundle: OriginPath::from_path(&backup),
            },
            budget(),
        )
        .unwrap()
        .publish_restore(&restored, &|| false)
        .unwrap();
    let mut out = f
        .app
        .query(
            &restored,
            app::ReadQuery::Trace { request: q.clone() },
            budget(),
        )
        .unwrap();
    let mut saved = Sink::default();
    out.records(&|| false, &mut saved).unwrap();
    drop(out);
    assert_eq!(query(&f, &q).1, saved.0);
    let path = f.dir.path().join("trace.json");
    fs::write(&path, serde_json::to_vec(&q).unwrap()).unwrap();
    let document = interfaces::cli(&f, &["trace", "--request", path.to_str().unwrap()]);
    assert_eq!(document["summary"]["summary"]["verdict"], "MATCH");
    let export = f.dir.path().join("trace-export.json");
    interfaces::cli(
        &f,
        &[
            "trace",
            "--request",
            path.to_str().unwrap(),
            "--output",
            export.to_str().unwrap(),
        ],
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&fs::read(&export).unwrap()).unwrap(),
        document
    );
}
#[test]
fn static_comparison_separates_known_difference_symbolic_uncertainty_and_incomplete_paths() {
    for case in 0..5 {
        let left = vec![0x600002b7, 0x00100593, 0x00b2a023, 0x00008067];
        let right = match case {
            0 => vec![0x600002b7, 0x00200593, 0x00b2a023, 0x00008067],
            1 => vec![0x600002b7, 0x00100593, 0x00b2a223, 0x00008067],
            2 => vec![0x600002b7, 0x00a2a023, 0x00008067], // symbolic vs constant
            3 => vec![0x600002b7, 0x00100593, 0x00b2a023, 0x0000006f], // visible prefix matches, loop
            _ => vec![0x600002b7, 0x00100593, 0x00b2a023, 0xffffffff],
        };
        let (f, q) = two(&left, &right);
        let (summary, rows) = query(&f, &q);
        assert_eq!(
            summary.verdict,
            Some(if case < 2 {
                ComparisonVerdict::Diff
            } else {
                ComparisonVerdict::Incomplete
            }),
            "case {case}: {summary:?} {rows:?}"
        );
        if case < 2 {
            assert!(
                rows.iter()
                    .any(|r| matches!(r, TraceRecord::Difference { .. }))
            );
        }
        if case == 3 {
            assert!(rows.iter().any(|r| matches!(
                r,
                TraceRecord::Blocked {
                    reason: TraceBlocker::Loop,
                    ..
                }
            )));
        }
    }
}
#[test]
fn static_trace_unknown_condition_address_and_partial_range_fail_explicitly() {
    for code in [
        vec![0x00050663, 0x600002b7, 0x0002a023, 0x00008067],
        vec![0x00052023, 0x00008067],
    ] {
        let f = fixture(object(&words(&code), (code.len() * 4) as u64, false), false);
        let q = request(&f);
        let (summary, rows) = query(&f, &q);
        assert!(!summary.left.exact, "{rows:?}");
        assert!(rows.iter().any(|r| matches!(
            r,
            TraceRecord::Blocked {
                reason: TraceBlocker::UnknownCondition | TraceBlocker::UnknownAddress,
                ..
            }
        )));
    }
    let code = [0x600002b7, 0x0002a023, 0x00008067];
    let f = fixture(object(&words(&code), 12, false), false);
    let mut q = request(&f);
    q.observation.ranges[0].start += 1;
    let (s, r) = query(&f, &q);
    assert!(!s.left.exact, "{r:?}");
    assert!(r.iter().any(|r| matches!(
        r,
        TraceRecord::Blocked {
            reason: TraceBlocker::CrossingRange,
            ..
        }
    )));
    q.left.registers = vec![TraceRegister {
        register: 0,
        value: 1,
    }];
    assert_eq!(
        f.app
            .query(&f.project, app::ReadQuery::Trace { request: q }, budget())
            .err()
            .unwrap()
            .code,
        ErrorCode::InvalidRequest
    );
}

#[test]
fn static_trace_inputs_choose_a_known_path_and_budget_failures_deliver_no_result() {
    let code = [0x00050663, 0x600002b7, 0x0002a023, 0x00008067];
    let f = fixture(object(&words(&code), 16, false), false);
    let mut q = request(&f);
    for (value, count) in [(0, 0), (1, 1)] {
        q.left.registers = vec![TraceRegister {
            register: 10,
            value,
        }];
        let (s, r) = query(&f, &q);
        assert!(s.left.exact, "{s:?} {r:?}");
        assert_eq!(s.left.events, count);
    }
    for memory in [true, false] {
        let mut limits = budget();
        if memory {
            limits.working_memory_bytes = Some(1024 * 1024);
        } else {
            limits.max_work_units = Some(1);
        }
        let handle = f
            .app
            .start_query(
                &f.project,
                app::ReadQuery::Trace { request: q.clone() },
                limits,
            )
            .unwrap();
        let failed = handle.wait();
        assert_eq!(failed.error.unwrap().code, ErrorCode::ResourceLimited);
        assert!(handle.take_output().is_err());
    }
    let cancelled = f
        .app
        .start_query(
            &f.project,
            app::ReadQuery::Trace { request: q.clone() },
            budget(),
        )
        .unwrap();
    assert!(cancelled.cancel());
    assert_eq!(cancelled.wait().state, RunState::Cancelled);
    drop(cancelled);
    assert!(query(&f, &q).0.left.exact);
    q.observation.ranges[0].length = u64::MAX;
    assert_eq!(
        f.app
            .query(&f.project, app::ReadQuery::Trace { request: q }, budget())
            .err()
            .unwrap()
            .code,
        ErrorCode::InvalidRequest
    );
}

#[test]
fn static_trace_does_not_accept_unsupported_fence_or_suppress_requested_fences() {
    let (f, mut q) = two(&[0x0330000f, 0x00008067], &[0x00008067]);
    assert_eq!(query(&f, &q).0.verdict, Some(ComparisonVerdict::Diff));
    q.observation.fences = false;
    assert_eq!(query(&f, &q).0.verdict, Some(ComparisonVerdict::Match));
    let (f, q) = two(&[0x8330000f, 0x00008067], &[0x8330000f, 0x00008067]);
    let (summary, rows) = query(&f, &q);
    assert_eq!(summary.verdict, Some(ComparisonVerdict::Incomplete));
    assert!(
        rows.iter().any(|r| matches!(
            r,
            TraceRecord::Blocked {
                reason: TraceBlocker::UnsupportedFence,
                ..
            }
        )),
        "{rows:?}"
    );
}

#[test]
fn relocatable_call_link_address_never_becomes_a_physical_section_offset() {
    let mut obj = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let caller = obj.add_section(vec![], b".text.entry".to_vec(), SectionKind::Text);
    obj.append_section_data(
        caller,
        &words(&[
            0x00008313, 0x60000537, 0x00c000ef, 0x00030093, 0x00008067, 0x00152023, 0x00008067,
        ]),
        4,
    );
    obj.add_symbol(symbol(
        b"entry",
        SymbolSection::Section(caller),
        20,
        SymbolKind::Text,
    ));
    let mut callee = symbol(
        b"callee",
        SymbolSection::Section(caller),
        8,
        SymbolKind::Text,
    );
    callee.value = 20;
    obj.add_symbol(callee);
    let f = fixture(obj.write().unwrap(), false);
    let caller = analyze(&f).analysis.unwrap();
    let snapshot = app::inventory(&f.project, None).unwrap();
    let symbol = snapshot.revision.inputs[0]
        .inventory
        .as_ref()
        .unwrap()
        .objects[1]
        .elf
        .as_ref()
        .unwrap()
        .symbols
        .iter()
        .find(|s| s.name.as_deref() == Some(b"callee"))
        .unwrap()
        .id
        .clone();
    let mut req = f.request.clone();
    req.selector = symbol.into();
    let run = f
        .app
        .start_analyze_function(&f.project, req, budget())
        .unwrap()
        .wait();
    assert_eq!(run.state, RunState::Completed, "{run:?}");
    let run = f
        .app
        .start_build_ir(
            &f.project,
            IrBuildRequest {
                scope: NavigationScope {
                    revision: f.revision.clone(),
                    publications: vec![],
                    analyses: vec![caller.clone(), run.analysis.unwrap()],
                    knowledge: None,
                },
                profiles: vec![IrProfile {
                    name: "trace".into(),
                    roots: IrRoots::All,
                    include_reachable: true,
                }],
            },
            budget(),
        )
        .unwrap()
        .wait();
    assert_eq!(run.state, RunState::Completed, "{run:?}");
    let target = TraceTarget {
        ir: run.semantic_ir.unwrap(),
        profile: "trace".into(),
        entry: caller,
        abi: CallAbi::RiscvInteger,
        registers: vec![TraceRegister {
            register: 1,
            value: 123,
        }],
    };
    let q = TraceRequest {
        left: target.clone(),
        right: Some(target),
        observation: TraceObservation {
            ranges: vec![ImageRegion {
                start: 0x60000000,
                length: 4,
            }],
            fences: false,
        },
    };
    let (summary, rows) = query(&f, &q);
    assert_eq!(
        summary.verdict,
        Some(ComparisonVerdict::Incomplete),
        "{summary:?} {rows:?}"
    );
    assert_eq!(summary.left.invocations, 2, "{rows:?}");
    assert!(rows.iter().any(|r| matches!(
        r,
        TraceRecord::Blocked {
            reason: TraceBlocker::UnknownValue,
            ..
        }
    )));
}
