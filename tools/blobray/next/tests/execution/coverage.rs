use super::interfaces::run;
use super::*;

/// `beq a0, zero, +8`, `li a0, 1`, `ret`, then an unreachable `li a0, 2`.
const PROGRAM: [u32; 4] = [0x00050463, 0x00100513, 0x00008067, 0x00200513];

fn coverage(rows: &[ExecutionEvidence], side: bool) -> ExecutionCoverage {
    let mut found = rows.iter().filter_map(|r| match r {
        ExecutionEvidence::Coverage {
            replacement,
            coverage,
        } if *replacement == side => Some(coverage.clone()),
        _ => None,
    });
    let coverage = found.next().expect("side coverage");
    assert!(found.next().is_none(), "one coverage record per side");
    coverage
}

#[test]
fn coverage_accumulates_each_side_across_cold_sessions() {
    let f = Fixture::new(&PROGRAM);
    let mut r = f.request();
    // Vendor: taken, then (after a cold reset) fallthrough. Replacement: taken twice.
    let mut second = r.cases[0].clone();
    second.name = "fallthrough".into();
    second.reset = SessionReset::Cold;
    second.vendor.arguments[0] = Some(5);
    r.cases.push(second);
    let (_, rows) = run(&f, r);
    assert!(matches!(
        rows.last(),
        Some(ExecutionEvidence::Coverage {
            replacement: true,
            ..
        })
    ));
    assert_eq!(
        coverage(&rows, false),
        ExecutionCoverage {
            instructions: vec![0x1000, 0x1004, 0x1008],
            branches: vec![BranchCoverage {
                site: 0x1000,
                taken: true,
                fallthrough: true,
            }],
        }
    );
    assert_eq!(
        coverage(&rows, true),
        ExecutionCoverage {
            instructions: vec![0x1000, 0x1008],
            branches: vec![BranchCoverage {
                site: 0x1000,
                taken: true,
                fallthrough: false,
            }],
        }
    );
}

#[test]
fn coverage_does_not_depend_on_selected_timeline() {
    let f = Fixture::new(&PROGRAM);
    let r = f.request();
    let (_, plain) = run(&f, r.clone());
    let mut traced = r;
    let case = &mut traced.cases[0];
    case.vendor.observe_timeline.branches = true;
    case.replacement.as_mut().unwrap().observe_timeline.branches = true;
    case.relation.as_mut().unwrap().events.timeline.branches = true;
    let (_, traced) = run(&f, traced);
    assert_eq!(coverage(&plain, false), coverage(&traced, false));
    assert!(traced.iter().any(|r| matches!(
        r,
        ExecutionEvidence::Event {
            event: ExecutionEvent::Branch { .. },
            ..
        }
    )));
}

/// Root at 0x1000: save `ra` in `s0`, branch on `a0`. Fallthrough calls the
/// modeled 0x2000 through `lui`/`jalr`, calls the helper at 0x1028 and jumps
/// to the epilogue; the taken side calls through `a1`. 0x1024 is dead code.
const CLOSURE: [u32; 11] = [
    0x00008413, // 0x1000 mv s0, ra
    0x00050a63, // 0x1004 beq a0, zero, 0x1018
    0x000022b7, // 0x1008 lui t0, 0x2
    0x000280e7, // 0x100c jalr ra, 0(t0)
    0x018000ef, // 0x1010 jal ra, 0x1028
    0x0080006f, // 0x1014 j 0x101c
    0x000580e7, // 0x1018 jalr ra, 0(a1)
    0x00040093, // 0x101c mv ra, s0
    0x00008067, // 0x1020 ret
    0x00200513, // 0x1024 li a0, 2
    0x00008067, // 0x1028 ret
];

fn closure_request(f: &Fixture, a0: u32, a1: u32) -> ExecutionRequest {
    let mut r = f.request();
    r.replacement = None;
    r.binding = None;
    let case = &mut r.cases[0];
    case.relation = None;
    case.replacement = None;
    case.vendor.arguments[0] = Some(a0);
    case.vendor.arguments[1] = Some(a1);
    case.vendor.calls = vec![CallDeclaration {
        repetition: CallRepetition::Unbounded,
        id: "external".into(),
        applicability: "synthetic external ABI assumption".into(),
        lifetime: RegionLifetime::Phase,
        binding: CallBinding {
            address: 0x2000,
            boundary: CallBoundary::Unmapped,
            allow_tail: false,
        },
        argument_words: 0,
        responses: vec![CallResponse {
            return_words: [Some(0), None],
            outputs: vec![],
            allocation: None,
            delay_micros: None,
        }],
    }];
    r
}

fn code_coverage(f: &Fixture, ids: &[&ArtifactId]) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_blobray"));
    command
        .args(["--format", "json", "code-coverage", "--project"])
        .arg(&f.project);
    for id in ids {
        command.args(["--execution", id.as_str()]);
    }
    command.args(["--limit-mode", "watchdog"]).output().unwrap()
}

fn report(f: &Fixture, ids: &[&ArtifactId]) -> CodeCoverageReport {
    let output = code_coverage(f, ids);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    serde_json::from_value(value["summary"]["report"].clone()).unwrap()
}

#[test]
fn code_coverage_reports_root_closures_boundaries_and_unions() {
    let f = Fixture::new(&CLOSURE);
    let fallthrough = f.run(closure_request(&f, 5, 0), budget());
    assert_eq!(fallthrough.state, RunState::Completed, "{fallthrough:?}");
    let fallthrough = fallthrough.execution.unwrap();
    let one = report(&f, &[&fallthrough]);
    assert_eq!(one.executions, vec![fallthrough.clone()]);
    assert_eq!(one.outside, 0);
    let [root, helper] = &one.functions[..] else {
        panic!("{one:?}")
    };
    assert_eq!(root.entry, 0x1000);
    assert_eq!(root.name, None);
    assert_eq!(root.modeled, vec![0x100c]);
    assert_eq!(root.unresolved, vec![0x1018]);
    assert!(root.gaps.is_empty());
    assert_eq!(
        root.blocks,
        CoverageCount {
            reached: 3,
            total: 4
        }
    );
    assert_eq!(root.uncovered_blocks, vec![0x1018]);
    assert_eq!(
        root.uncovered_directions,
        vec![BranchDirection {
            site: 0x1004,
            taken: true
        }]
    );
    assert_eq!(helper.entry, 0x1028);
    assert_eq!(
        helper.blocks,
        CoverageCount {
            reached: 1,
            total: 1
        }
    );
    let [root_report] = &one.roots[..] else {
        panic!("{one:?}")
    };
    assert_eq!(root_report.functions, vec![0x1000, 0x1028]);
    assert_eq!(
        root_report.blocks,
        CoverageCount {
            reached: 4,
            total: 5
        }
    );
    assert_eq!(
        root_report.directions,
        CoverageCount {
            reached: 1,
            total: 2
        }
    );

    // The taken side calls the helper through `a1`; together both executions
    // reach every block and direction.
    let taken = f.run(closure_request(&f, 0, 0x1028), budget());
    assert_eq!(taken.state, RunState::Completed, "{taken:?}");
    let taken = taken.execution.unwrap();
    let both = report(&f, &[&fallthrough, &taken]);
    assert!(both.roots[0].blocks.complete() && both.roots[0].directions.complete());
    assert!(
        both.functions
            .iter()
            .all(|f| f.uncovered_blocks.is_empty() && f.uncovered_directions.is_empty())
    );

    // Executions of different vendor targets and repeated executions do not combine.
    let mut other = closure_request(&f, 5, 0);
    other.vendor.stack.fill = Some(0);
    let other = f.run(other, budget()).execution.unwrap();
    assert!(!code_coverage(&f, &[&fallthrough, &other]).status.success());
    assert!(
        !code_coverage(&f, &[&fallthrough, &fallthrough])
            .status
            .success()
    );
}

/// `elf(code)` with `.text`, `.symtab` and `.strtab` naming global functions.
fn elf_with_symbols(code: &[u32], symbols: &[(&str, u32, u32)]) -> Vec<u8> {
    let put = |b: &mut Vec<u8>, v: u32| b.extend_from_slice(&v.to_le_bytes());
    let mut b = elf(code);
    let text = (256, (code.len() * 4) as u32);
    let mut strtab = vec![0u8];
    let mut symtab = vec![0u8; 16];
    for (name, address, size) in symbols {
        put(&mut symtab, strtab.len() as u32);
        strtab.extend_from_slice(name.as_bytes());
        strtab.push(0);
        put(&mut symtab, *address);
        put(&mut symtab, *size);
        symtab.extend_from_slice(&[0x12, 0]); // STB_GLOBAL | STT_FUNC, default visibility
        symtab.extend_from_slice(&1u16.to_le_bytes()); // .text
    }
    let shstrtab = b"\0.text\0.symtab\0.strtab\0.shstrtab\0";
    let place = |b: &mut Vec<u8>, bytes: &[u8]| {
        while !b.len().is_multiple_of(4) {
            b.push(0);
        }
        let offset = b.len() as u32;
        b.extend_from_slice(bytes);
        (offset, bytes.len() as u32)
    };
    let symtab = place(&mut b, &symtab);
    let strtab = place(&mut b, &strtab);
    let shstrtab = place(&mut b, shstrtab);
    let headers = place(&mut b, &[0; 40]).0;
    // name, type, flags, address, offset, size, link, info, align, entsize
    for header in [
        [1, 1, 6, 0x1000, text.0, text.1, 0, 0, 4, 0],
        [7, 2, 0, 0, symtab.0, symtab.1, 3, 1, 4, 16],
        [15, 3, 0, 0, strtab.0, strtab.1, 0, 0, 1, 0],
        [23, 3, 0, 0, shstrtab.0, shstrtab.1, 0, 0, 1, 0],
    ] {
        for v in header {
            put(&mut b, v);
        }
    }
    b[32..36].copy_from_slice(&headers.to_le_bytes());
    b[46..48].copy_from_slice(&40u16.to_le_bytes());
    b[48..50].copy_from_slice(&5u16.to_le_bytes());
    b[50..52].copy_from_slice(&4u16.to_le_bytes());
    b
}

#[test]
fn a_jump_to_a_defined_function_start_is_a_named_tail_call() {
    // 0x1000 `j 0x1008`; 0x1004 dead `ret`; 0x1008 `ret`.
    let code = [0x0080006f, 0x00008067, 0x00008067];
    let symbols = [("root", 0x1000, 8), ("tail", 0x1008, 4)];
    let named = Fixture::from_inputs(vec![elf_with_symbols(&code, &symbols)]);
    let mut r = named.request();
    r.cases[0].relation = None;
    r.cases[0].replacement = None;
    r.replacement = None;
    r.binding = None;
    let id = named.run(r.clone(), budget()).execution.unwrap();
    let named_report = report(&named, &[&id]);
    let names: Vec<_> = named_report
        .functions
        .iter()
        .map(|f| (f.entry, f.name.as_deref(), f.blocks))
        .collect();
    assert_eq!(
        names,
        vec![
            (
                0x1000,
                Some("root"),
                CoverageCount {
                    reached: 1,
                    total: 1
                }
            ),
            (
                0x1008,
                Some("tail"),
                CoverageCount {
                    reached: 1,
                    total: 1
                }
            ),
        ]
    );
    assert_eq!(named_report.roots[0].name.as_deref(), Some("root"));
    // Without symbols the same jump stays inside the root.
    let plain = Fixture::new(&code);
    let mut r = plain.request();
    r.cases[0].relation = None;
    r.cases[0].replacement = None;
    r.replacement = None;
    r.binding = None;
    let id = plain.run(r, budget()).execution.unwrap();
    let plain_report = report(&plain, &[&id]);
    assert_eq!(plain_report.functions.len(), 1);
    assert_eq!(
        plain_report.functions[0].blocks,
        CoverageCount {
            reached: 2,
            total: 2
        }
    );
}
