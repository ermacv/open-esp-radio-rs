use super::interfaces::cli;
use super::*;

fn saved(words: &[u32]) -> (Fixture, FunctionAnalysisId, Vec<FunctionRecord>) {
    let code: Vec<_> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let f = fixture(object(&code, code.len() as u64, false), false);
    let run = analyze(&f);
    assert_eq!(run.state, RunState::Completed, "{run:?}");
    let id = run.analysis.unwrap();
    let (_, records) = export(&f, id.clone());
    (f, id, records)
}
fn access(records: &[FunctionRecord], at: u64) -> u64 {
    records
        .iter()
        .position(|r| matches!(r, FunctionRecord::MemoryAccess {offset,..} if *offset == at))
        .unwrap() as u64
}
fn request(id: FunctionAnalysisId, records: &[FunctionRecord], at: u64) -> MemorySliceQuery {
    MemorySliceQuery {
        analysis: id,
        anchor: access(records, at),
        abi: Some(CallAbi::RiscvInteger),
        locations: vec![MemorySliceSelection::Argument {
            word: 0,
            offset: 0,
            width: 4,
        }],
    }
}
#[derive(Default)]
struct Sink(Vec<MemorySliceRecord>);
impl ElfSink for Sink {
    fn section(&mut self, _: &SectionRecord, _: &mut dyn RunControl) -> Result<()> {
        panic!("unexpected section")
    }
    fn symbol(&mut self, _: &SymbolRecord, _: &mut dyn RunControl) -> Result<()> {
        panic!("unexpected symbol")
    }
    fn relocation(&mut self, _: &RelocationRecord, _: &mut dyn RunControl) -> Result<()> {
        panic!("unexpected relocation")
    }
    fn diagnostic(&mut self, _: &Diagnostic, _: &mut dyn RunControl) -> Result<()> {
        panic!("unexpected diagnostic")
    }
}
impl InventorySink for Sink {}
impl app::DoctorSink for Sink {
    fn error(&mut self, _: &Error, _: &mut dyn RunControl) -> Result<()> {
        panic!("unexpected error")
    }
    fn unfinished(&mut self, _: &RunId, _: &mut dyn RunControl) -> Result<()> {
        panic!("unexpected unfinished")
    }
}
impl app::QuerySink for Sink {
    fn summary(&mut self, _: &app::QuerySummary, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
    fn memory_slice(&mut self, record: &MemorySliceRecord, _: &mut dyn RunControl) -> Result<()> {
        self.0.push(record.clone());
        Ok(())
    }
}
fn query(f: &Fixture, q: &MemorySliceQuery) -> (MemorySliceSummary, Vec<MemorySliceRecord>) {
    let mut out = f
        .app
        .query(
            &f.project,
            app::ReadQuery::MemorySlice { request: q.clone() },
            budget(),
        )
        .unwrap();
    let app::QuerySummary::MemorySlice { summary } = out.summary() else {
        panic!()
    };
    let summary = *summary.clone();
    let mut sink = Sink::default();
    out.records(&|| false, &mut sink).unwrap();
    (summary, sink.0)
}
fn definitions(rows: &[MemorySliceRecord]) -> Vec<(u64, DefinitionClass)> {
    let mut out: Vec<_> = rows
        .iter()
        .filter_map(|r| match r {
            MemorySliceRecord::Definition { offset, class, .. } => Some((*offset, *class)),
            _ => None,
        })
        .collect();
    out.sort_unstable_by_key(|r| r.0);
    out
}
fn location(rows: &[MemorySliceRecord]) -> (IncomingState, &[SliceIssue]) {
    let MemorySliceRecord::Location {
        incoming, issues, ..
    } = rows
        .iter()
        .find(|r| matches!(r, MemorySliceRecord::Location { .. }))
        .unwrap()
    else {
        unreachable!()
    };
    (*incoming, issues)
}

#[test]
fn saved_slice_kills_old_writes_excludes_current_anchor_and_reopens_without_sources() {
    let (f, id, records) = saved(&[0x00b52023, 0x00c52023, 0x00852703, 0x00d52223, 0x00008067]);
    let mut q = request(id, &records, 12);
    let (summary, rows) = query(&f, &q);
    assert_eq!(summary.anchor_offset, 12);
    assert_eq!(definitions(&rows), vec![(4, DefinitionClass::Must)]);
    assert_eq!(location(&rows), (IncomingState::Overwritten, &[][..]));
    let definition = rows
        .iter()
        .find(|r| matches!(r, MemorySliceRecord::Definition { .. }))
        .unwrap();
    let MemorySliceRecord::Definition {
        record,
        fact,
        witness,
        ..
    } = definition
    else {
        unreachable!()
    };
    assert_eq!(**fact, records[*record as usize]);
    assert_eq!(witness, &[4, 8, 12]);
    q.locations = vec![MemorySliceSelection::Access {
        record: access(&records, 8),
    }];
    let (_, incoming) = query(&f, &q);
    assert_eq!(location(&incoming), (IncomingState::Possible, &[][..]));
    assert!(definitions(&incoming).is_empty());
    q.locations.clear();
    let expected = query(&f, &q);
    assert_eq!(expected.0.locations, 1); // Current anchor writes +4, but has not run yet.
    fs::remove_file(f.dir.path().join("entry.o")).unwrap();
    fs::remove_file(f.dir.path().join("entry.a")).unwrap();
    assert_eq!(query(&f, &q), expected);
    let path = f.dir.path().join("slice.json");
    fs::write(&path, serde_json::to_vec(&q).unwrap()).unwrap();
    let result = cli(&f, &["memory-slice", "--request", path.to_str().unwrap()]);
    assert_eq!(
        result["summary"]["summary"],
        serde_json::to_value(&expected.0).unwrap()
    );
    let output = f.dir.path().join("slice-export.json");
    cli(
        &f,
        &[
            "memory-slice",
            "--request",
            path.to_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
        ],
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&fs::read(output).unwrap()).unwrap(),
        result
    );
}

#[test]
fn diamond_definitions_are_alternatives_and_missing_branch_preserves_incoming() {
    for branch in [0x00c52023, 0x00000013] {
        let (f, id, records) = saved(&[
            0x00070663, 0x00b52023, 0x0080006f, branch, 0x00d52223, 0x00008067,
        ]);
        let q = request(id, &records, 16);
        let (_, rows) = query(&f, &q);
        let expected = if branch == 0x00000013 {
            vec![(4, DefinitionClass::Alternative)]
        } else {
            vec![
                (4, DefinitionClass::Alternative),
                (12, DefinitionClass::Alternative),
            ]
        };
        assert_eq!(definitions(&rows), expected, "{rows:#?}");
        assert_eq!(
            location(&rows).0,
            if branch == 0x00000013 {
                IncomingState::Possible
            } else {
                IncomingState::Overwritten
            }
        );
    }
}

#[test]
fn partial_writes_and_unknown_aliases_never_become_must_values() {
    for second in [0x00c51123, 0x00c72023] {
        // halfword +2 or unrelated entry pointer a4
        let (f, id, records) = saved(&[0x00b52023, second, 0x00d52223, 0x00008067]);
        let mut q = request(id, &records, 8);
        let (_, rows) = query(&f, &q);
        assert!(
            definitions(&rows)
                .iter()
                .all(|r| r.1 == DefinitionClass::Candidate),
            "{rows:#?}"
        );
        assert_eq!(location(&rows).0, IncomingState::Overwritten);
        assert!(location(&rows).1.contains(&if second == 0x00c51123 {
            SliceIssue::PartialOverlap
        } else {
            SliceIssue::MayAlias
        }));
        if second == 0x00c51123 {
            q.anchor = access(&records, 4);
            q.locations = vec![MemorySliceSelection::Argument {
                word: 0,
                offset: 2,
                width: 2,
            }];
            let (_, narrow) = query(&f, &q);
            assert_eq!(definitions(&narrow), vec![(0, DefinitionClass::Candidate)]);
            assert_eq!(location(&narrow).0, IncomingState::Overwritten);
        }
    }
}

#[test]
fn one_loaded_pointer_has_stable_fields_but_loop_loads_keep_dynamic_identity() {
    let (f, id, records) = saved(&[0x00052703, 0x00b72023, 0x00c72223, 0x00d52423, 0x00008067]);
    let mut q = request(id, &records, 12);
    q.locations = vec![MemorySliceSelection::Access {
        record: access(&records, 4),
    }];
    let (_, rows) = query(&f, &q);
    assert_eq!(
        definitions(&rows),
        vec![(4, DefinitionClass::Must)],
        "{rows:#?}"
    );
    let MemorySliceRecord::Location { location: loc, .. } = &rows[0] else {
        panic!()
    };
    assert!(matches!(loc.address, SliceAddress::Value { .. }));

    let (f, id, records) = saved(&[
        0x00052703,
        0x00b72023,
        branch_back(15, -8),
        0x00d52423,
        0x00008067,
    ]);
    let mut q = request(id, &records, 12);
    q.locations = vec![MemorySliceSelection::Access {
        record: access(&records, 4),
    }];
    let (_, rows) = query(&f, &q);
    assert_eq!(
        definitions(&rows),
        vec![(4, DefinitionClass::Candidate)],
        "{rows:#?}"
    );
    assert!(location(&rows).1.contains(&SliceIssue::DynamicIdentity));
    assert_eq!(location(&rows).0, IncomingState::Unknown);
}

fn branch_back(register: u32, offset: i32) -> u32 {
    let bits = offset as u32;
    ((bits >> 12) & 1) << 31
        | ((bits >> 5) & 63) << 25
        | register << 15
        | ((bits >> 1) & 15) << 8
        | ((bits >> 11) & 1) << 7
        | 0x63
}

#[test]
fn anchor_in_a_loop_can_observe_its_previous_iteration() {
    let (f, id, records) = saved(&[0x00b52023, branch_back(15, -4), 0x00008067]);
    let q = request(id, &records, 0);
    let (_, rows) = query(&f, &q);
    assert_eq!(location(&rows), (IncomingState::Possible, &[][..]));
    assert_eq!(definitions(&rows), vec![(0, DefinitionClass::Alternative)]);
    assert!(
        rows.iter()
            .any(|r| matches!(r, MemorySliceRecord::Definition {witness,..} if witness==&[0,4,0]))
    );
}

#[test]
fn calls_clobber_memory_but_an_unconditional_later_store_kills_the_barrier() {
    for after in [false, true] {
        let words = if after {
            vec![
                0x100002b7, 0x00b2a023, 0x020000ef, 0x100002b7, 0x00d2a223, 0x00008067,
            ]
        } else {
            vec![0x020000ef, 0x100002b7, 0x00b2a023, 0x00d2a223, 0x00008067]
        };
        let (f, id, records) = saved(&words);
        let mut q = request(id, &records, if after { 16 } else { 12 });
        q.locations = vec![MemorySliceSelection::Address {
            address: 0x10000000,
            width: 4,
        }];
        let (_, rows) = query(&f, &q);
        assert_eq!(
            definitions(&rows),
            vec![(
                if after { 4 } else { 8 },
                if after {
                    DefinitionClass::Candidate
                } else {
                    DefinitionClass::Must
                }
            )],
            "{rows:#?}"
        );
        assert_eq!(
            location(&rows).1.contains(&SliceIssue::CallClobber),
            after,
            "{rows:#?}"
        );
    }
}

#[test]
fn malformed_selections_and_resource_failure_leave_saved_analysis_usable() {
    let (f, id, records) = saved(&[0x00b52023, 0x00d52223, 0x00008067]);
    let q = request(id, &records, 4);
    let expected = query(&f, &q);
    for variant in 0..5 {
        let mut bad = q.clone();
        match variant {
            0 => bad.anchor = u64::MAX,
            1 => {
                bad.anchor = records
                    .iter()
                    .position(|r| matches!(r, FunctionRecord::Instruction { offset: 0, .. }))
                    .unwrap() as u64
            }
            2 => bad.locations = vec![MemorySliceSelection::Access { record: u64::MAX }],
            3 => {
                bad.locations = vec![MemorySliceSelection::Stack {
                    offset: 0,
                    width: 3,
                }]
            }
            _ => bad.abi = None,
        }
        let error = f
            .app
            .query(
                &f.project,
                app::ReadQuery::MemorySlice { request: bad },
                budget(),
            )
            .err()
            .unwrap();
        assert_eq!(error.code, ErrorCode::InvalidRequest, "{error:?}");
    }
    for capacity in [false, true] {
        let mut limits = budget();
        if capacity {
            limits.working_memory_bytes = Some(1024 * 1024);
        } else {
            limits.max_work_units = Some(1);
        }
        let error = f
            .app
            .query(
                &f.project,
                app::ReadQuery::MemorySlice { request: q.clone() },
                limits,
            )
            .err()
            .unwrap();
        assert_eq!(error.code, ErrorCode::ResourceLimited, "{error:?}");
    }
    assert_eq!(query(&f, &q), expected);
    assert_eq!(
        app::inventory(&f.project, None).unwrap().revision_id,
        f.revision
    );
}
