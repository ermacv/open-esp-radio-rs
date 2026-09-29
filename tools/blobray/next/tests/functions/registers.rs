use super::cli;
use super::*;
#[derive(Default)]
struct Sink(Vec<RegisterRecord>);
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

#[test]
fn finite_addresses_filters_partial_scope_and_bad_requests_stay_explicit() {
    let words: [u32; 6] = [
        0x00050663, 0x000202b7, 0x0080006f, 0x000212b7, 0x0002a303, 0xffffffff,
    ];
    let code: Vec<_> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let f = fixture(object(&code, code.len() as u64, false), false);
    let run = analyze(&f);
    assert_eq!(run.state, RunState::Completed, "{run:?}");
    let mut q = RegisterQuery {
        scope: NavigationScope {
            revision: f.revision.clone(),
            publications: vec![],
            analyses: vec![run.analysis.unwrap()],
        },
        ranges: vec![],
    };
    let (summary, rows) = query(&f, &q);
    assert_eq!(summary.partial_analyses, 1);
    assert_eq!(summary.alternative_observations, 2);
    let mut addresses: Vec<_> = rows
        .iter()
        .filter_map(|r| match r {
            RegisterRecord::Observation { address, .. } => *address,
            _ => None,
        })
        .collect();
    addresses.sort_unstable();
    assert_eq!(addresses, [0x20000, 0x21000]);
    q.ranges = vec![ImageRegion {
        start: 0x21000,
        length: 4,
    }];
    assert_eq!(query(&f, &q).0.alternative_observations, 1);
    for case in 0..4 {
        let mut bad = q.clone();
        match case {
            0 => bad.scope.analyses.clear(),
            1 => bad.ranges[0].length = 0,
            2 => {
                bad.ranges = vec![ImageRegion {
                    start: u32::MAX,
                    length: 2,
                }]
            }
            _ => bad.ranges = vec![bad.ranges[0]; 257],
        }
        let error = f
            .app
            .query(
                &f.project,
                app::ReadQuery::Registers { request: bad },
                budget(),
            )
            .err()
            .unwrap();
        assert_eq!(error.code, ErrorCode::InvalidRequest, "{error:?}");
    }
    assert_eq!(query(&f, &q).0.candidate_addresses, 1);
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
    fn register(&mut self, record: &RegisterRecord, _: &mut dyn RunControl) -> Result<()> {
        self.0.push(record.clone());
        Ok(())
    }
}
fn query(f: &Fixture, q: &RegisterQuery) -> (RegisterSummary, Vec<RegisterRecord>) {
    let mut out = f
        .app
        .query(
            &f.project,
            app::ReadQuery::Registers { request: q.clone() },
            budget(),
        )
        .unwrap();
    assert_eq!(out.summary().assessment(), ResultAssessment::default());
    let app::QuerySummary::Registers { summary } = out.summary() else {
        panic!()
    };
    let summary = *summary.clone();
    let mut sink = Sink::default();
    out.records(&|| false, &mut sink).unwrap();
    (summary, sink.0)
}

#[test]
fn register_discovery_and_source_free_export_share_scope() {
    // lui t0,0x20; lw t1,0(t0); andi t2,t1,0xf; andi t1,t1,-16;
    // ori t1,t1,3; sw t1,0(t0); lb t2,1(t0); lw t2,0(a0); ret.
    let words: [u32; 9] = [
        0x000202b7, 0x0002a303, 0x00f37393, 0xff037313, 0x00336313, 0x0062a023, 0x00128383,
        0x00052383, 0x00008067,
    ];
    let code: Vec<_> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let mut f = fixture(object(&code, code.len() as u64, false), false);
    let run = analyze(&f);
    assert_eq!(run.state, RunState::Completed, "{run:?}");
    let analysis = run.analysis.unwrap();
    let q = RegisterQuery {
        scope: NavigationScope {
            revision: f.revision.clone(),
            publications: vec![],
            analyses: vec![analysis],
        },
        ranges: vec![],
    };
    let (summary, rows) = query(&f, &q);
    assert_eq!(summary.selected_analyses, 1);
    assert!(summary.unresolved_addresses > 0);
    assert!(
        rows.iter()
            .any(|r| matches!(r, RegisterRecord::Address { address: 0x20000,
        access_widths, local_accesses: 2, .. } if access_widths == &[4]))
    );
    assert!(rows.iter().any(|r| matches!(
        r,
        RegisterRecord::Observation {
            mask: Some(RegisterMask {
                kind: RegisterMaskKind::ReadSelection,
                bits: 15
            }),
            ..
        }
    )));
    assert!(rows.iter().any(|r| matches!(
        r,
        RegisterRecord::Observation {
            mask: Some(RegisterMask {
                kind: RegisterMaskKind::WriteReplacement,
                bits: 15
            }),
            ..
        }
    )));
    fs::remove_file(f.dir.path().join("entry.a")).unwrap();
    fs::remove_file(f.dir.path().join("entry.o")).unwrap();
    let file = f.dir.path().join("register-query.json");
    let output = f.dir.path().join("register-export.json");
    fs::write(&file, serde_json::to_vec(&q).unwrap()).unwrap();
    cli(
        &f,
        &[
            "registers",
            "--request",
            file.to_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
        ],
    );
    let bytes = fs::read(&output).unwrap();
    let exported: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(exported["summary"]["kind"], "registers");
    let expected = query(&f, &q);
    let backup = f.dir.path().join("registers.backup");
    cli(&f, &["backup", "--output", backup.to_str().unwrap()]);
    f.project = f.dir.path().join("restored");
    cli(&f, &["restore", "--backup", backup.to_str().unwrap()]);
    assert_eq!(query(&f, &q), expected);
    fs::remove_file(&output).unwrap();
    cli(
        &f,
        &[
            "registers",
            "--request",
            file.to_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
        ],
    );
    assert_eq!(fs::read(&output).unwrap(), bytes);
    for index in 0..2 {
        let mut limit = budget();
        if index == 0 {
            limit.max_work_units = Some(1);
        } else {
            limit.working_memory_bytes = Some(1024);
        }
        let out = f.app.query(
            &f.project,
            app::ReadQuery::Registers { request: q.clone() },
            limit,
        );
        assert!(out.is_err());
    }
}
