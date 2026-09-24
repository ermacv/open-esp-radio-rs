//! Both consumers start from the same captured ELF, never handcrafted call inputs.
use super::*;

fn jal(from: u32, to: u32, dest: u32) -> u32 {
    let d = to.wrapping_sub(from);
    ((d >> 20 & 1) << 31)
        | ((d >> 1 & 1023) << 21)
        | ((d >> 11 & 1) << 20)
        | ((d >> 12 & 255) << 12)
        | (dest << 7)
        | 0x6f
}
fn word(code: &mut Vec<u8>, op: u32) {
    code.extend_from_slice(&op.to_le_bytes());
}
fn linked(functions: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let end = functions
        .iter()
        .map(|(a, b)| *a as usize + b.len())
        .max()
        .unwrap();
    let mut bytes = elf(&vec![0x13; (end - 0x1000).div_ceil(4)]);
    for (address, code) in functions {
        let at = 256 + (*address - 0x1000) as usize;
        bytes[at..at + code.len()].copy_from_slice(code);
    }
    let strings = b"\0entry\0other\0callee\0nested\0";
    let string_offset = bytes.len() as u32;
    bytes.extend_from_slice(strings);
    while !bytes.len().is_multiple_of(4) {
        bytes.push(0);
    }
    let symbols = bytes.len() as u32;
    bytes.extend_from_slice(&[0; 16]);
    for ((address, code), name) in functions.iter().zip([1u32, 7, 13, 20]) {
        bytes.extend_from_slice(&name.to_le_bytes());
        bytes.extend_from_slice(&address.to_le_bytes());
        bytes.extend_from_slice(&(code.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&[0x12, 0, 1, 0]);
    }
    let sections = bytes.len() as u32;
    for fields in [
        [0; 10],
        [0, 1, 6, 0x1000, 256, (end - 0x1000) as u32, 0, 0, 2, 0],
        [0, 3, 0, 0, string_offset, strings.len() as u32, 0, 0, 1, 0],
        [
            0,
            2,
            0,
            0,
            symbols,
            (functions.len() as u32 + 1) * 16,
            2,
            1,
            4,
            16,
        ],
    ] {
        for field in fields {
            bytes.extend_from_slice(&field.to_le_bytes());
        }
    }
    bytes[32..36].copy_from_slice(&sections.to_le_bytes());
    bytes[46..48].copy_from_slice(&40u16.to_le_bytes());
    bytes[48..50].copy_from_slice(&4u16.to_le_bytes());
    bytes[50..52].copy_from_slice(&2u16.to_le_bytes());
    bytes
}
fn cli(f: &Fixture, command: &str, args: &[&str]) -> serde_json::Value {
    let output = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args(["--format", "json", command, "--project"])
        .arg(&f.project)
        .args(["--limit-mode", "watchdog"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    if output.stdout.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&output.stdout).unwrap()
    }
}
fn trace_request(f: &Fixture) -> TraceRequest {
    let published = cli(f, "analyze-project", &[]);
    let publication = published["run"]["publication"].as_str().unwrap();
    let functions = cli(f, "functions", &["--id", publication]);
    let analyses: Vec<FunctionAnalysisId> = functions["records"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| {
            r["value"]["outcome"]["analysis"]
                .as_str()
                .map(|s| s.parse().unwrap())
        })
        .collect();
    let run = f
        .app
        .start_build_ir(
            &f.project,
            IrBuildRequest {
                scope: NavigationScope {
                    revision: f.target.revision.clone(),
                    publications: vec![publication.parse().unwrap()],
                    analyses: vec![],
                    knowledge: None,
                },
                profiles: vec![IrProfile {
                    name: "links".into(),
                    roots: IrRoots::All,
                    include_reachable: true,
                }],
            },
            budget(),
        )
        .unwrap()
        .wait();
    assert_eq!(run.state, RunState::Completed, "{run:?}");
    let target = |entry| TraceTarget {
        ir: run.semantic_ir.clone().unwrap(),
        profile: "links".into(),
        entry,
        abi: CallAbi::RiscvInteger,
        registers: vec![
            TraceRegister {
                register: 1,
                value: u32::MAX - 1,
            },
            TraceRegister {
                register: 2,
                value: 0x9000,
            },
        ],
    };
    TraceRequest {
        left: target(analyses[0].clone()),
        right: Some(target(analyses[1].clone())),
        observation: TraceObservation {
            ranges: vec![ImageRegion {
                start: 0x60000000,
                length: 8,
            }],
            fences: false,
        },
    }
}
fn trace(f: &Fixture, request: &TraceRequest) -> serde_json::Value {
    let path = f._dir.path().join("trace.json");
    fs::write(&path, serde_json::to_vec(request).unwrap()).unwrap();
    cli(f, "trace", &["--request", path.to_str().unwrap()])
}
fn concrete(f: &Fixture, other: u32) -> serde_json::Value {
    let mut request = f.request();
    request.max_events = 64;
    request.cases[0].relation.as_mut().unwrap().returns = ReturnWords {
        low: false,
        high: false,
    };
    let case = &mut request.cases[0];
    for input in [&mut case.vendor, case.replacement.as_mut().unwrap()] {
        input.models = vec![register_bank(vec![
            RegisterCell {
                address: 0x60000000,
                width: 4,
                value: 0,
            },
            RegisterCell {
                address: 0x60000004,
                width: 4,
                value: 0,
            },
        ])];
    }
    request.cases[0].replacement.as_mut().unwrap().entry = other;
    let run = f.run(request, budget());
    assert_eq!(run.state, RunState::Completed, "{run:?}");
    f.read(&run.execution.unwrap())
}
#[test]
fn elf_call_link_effects_agree_with_concrete_execution_and_restore() {
    // JAL x1/x5, JALR x1/x5, C.JAL, C.JALR, direct/indirect tails.
    for variant in 0..8 {
        let link = if matches!(variant, 1 | 3) { 5 } else { 1 };
        let tail = variant >= 6;
        let mut expected = Vec::new();
        let mut functions = Vec::new();
        for base in [0x1000, 0x1100] {
            let mut code = Vec::new();
            word(&mut code, 0x00008313); // preserve entry ra in t1; leaf callee leaves it intact
            word(&mut code, 0x60000537);
            if matches!(variant, 2 | 3 | 5 | 7) {
                word(&mut code, 0x000012b7); // t0 = 0x1300
                word(&mut code, 0x30028293);
            }
            let pc = base + code.len() as u32;
            match variant {
                0 | 1 | 6 => word(&mut code, jal(pc, 0x1300, if tail { 0 } else { link })),
                2 | 3 | 7 => word(&mut code, 0x00028067 | (if tail { 0 } else { link } << 7)),
                4 => {
                    // C.JAL immediate encoding, independently checked by concrete execution.
                    let d = 0x1300 - pc;
                    let op = 0x2001u16
                        | (((d >> 11 & 1) << 12
                            | (d >> 4 & 1) << 11
                            | (d >> 8 & 3) << 9
                            | (d >> 10 & 1) << 8
                            | (d >> 6 & 1) << 7
                            | (d >> 7 & 1) << 6
                            | (d >> 1 & 7) << 3
                            | (d >> 5 & 1) << 2) as u16);
                    code.extend_from_slice(&op.to_le_bytes());
                }
                5 => code.extend_from_slice(&0x9282u16.to_le_bytes()), // c.jalr t0
                _ => unreachable!(),
            }
            expected.push(if tail {
                u32::MAX - 1
            } else {
                base + code.len() as u32
            });
            if !tail {
                word(&mut code, 0x00030093); // restore ra
                word(&mut code, 0x00008067);
            }
            functions.push((base, code));
        }
        let mut callee = Vec::new();
        word(&mut callee, (link << 20) | 0x52023);
        word(&mut callee, (link << 15) | 0x67);
        functions.push((0x1300, callee));
        let f = Fixture::from_inputs(vec![linked(&functions)]);
        let mut q = trace_request(&f);
        let saved = trace(&f, &q);
        let executed = concrete(&f, 0x1100);
        let verdict = if tail { "MATCH" } else { "DIFF" };
        assert_eq!(saved["summary"]["summary"]["policy"], 2);
        assert_eq!(
            saved["summary"]["summary"]["verdict"], verdict,
            "variant {variant}: {saved}"
        );
        assert_eq!(executed["summary"]["manifest"]["verdict"], verdict);
        assert_eq!(executed["summary"]["manifest"]["complete"], true);
        let values: Vec<u32> = saved["records"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["value"]["kind"] == "event")
            .map(|r| r["value"]["event"]["value"]["value"].as_u64().unwrap() as u32)
            .collect();
        assert_eq!(values, expected, "variant {variant}");
        let values: Vec<u32> = executed["records"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["value"]["event"]["kind"] == "write")
            .map(|r| r["value"]["event"]["value"].as_u64().unwrap() as u32)
            .collect();
        assert_eq!(values, expected);
        q.right = Some(q.left.clone());
        assert_eq!(trace(&f, &q)["summary"]["summary"]["verdict"], "MATCH");
        assert_eq!(
            concrete(&f, 0x1000)["summary"]["manifest"]["verdict"],
            "MATCH"
        );
        if variant == 0 {
            q.right.as_mut().unwrap().entry = trace_request(&f).right.unwrap().entry;
            let backup = f._dir.path().join("links.backup");
            cli(&f, "backup", &["--output", backup.to_str().unwrap()]);
            let restored = f._dir.path().join("restored");
            let output = Command::new(env!("CARGO_BIN_EXE_blobray"))
                .args(["restore", "--project"])
                .arg(&restored)
                .arg("--backup")
                .arg(&backup)
                .args(["--limit-mode", "watchdog"])
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let restored_f = Fixture {
                project: restored,
                ..f
            };
            assert_eq!(trace(&restored_f, &q), saved);
        }
    }
}

#[test]
fn repeated_nested_invocations_have_distinct_link_values() {
    let mut functions = Vec::new();
    for base in [0x1000, 0x1100] {
        let mut code = Vec::new();
        for op in [
            0x00008313,
            0x60000537,
            jal(base + 8, 0x1300, 1),
            jal(base + 12, 0x1300, 1),
            0x00030093,
            0x00008067,
        ] {
            word(&mut code, op);
        }
        functions.push((base, code));
    }
    let mut callee = Vec::new();
    for op in [
        0x00152023,
        0x00008393,
        jal(0x1308, 0x1400, 1),
        0x00038093,
        0x00008067,
    ] {
        word(&mut callee, op);
    }
    functions.push((0x1300, callee));
    let mut nested = Vec::new();
    word(&mut nested, 0x00152223);
    word(&mut nested, 0x00008067);
    functions.push((0x1400, nested));
    let f = Fixture::from_inputs(vec![linked(&functions)]);
    let q = trace_request(&f);
    let saved = trace(&f, &q);
    let executed = concrete(&f, 0x1100);
    assert_eq!(saved["summary"]["summary"]["verdict"], "DIFF", "{saved}");
    assert_eq!(executed["summary"]["manifest"]["verdict"], "DIFF");
    assert_eq!(executed["summary"]["manifest"]["complete"], true);
    let expected = [
        0x100c, 0x130c, 0x1010, 0x130c, 0x110c, 0x130c, 0x1110, 0x130c,
    ];
    let values: Vec<u32> = saved["records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["value"]["kind"] == "event")
        .map(|r| r["value"]["event"]["value"]["value"].as_u64().unwrap() as u32)
        .collect();
    assert_eq!(values, expected);
    let values: Vec<u32> = executed["records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["value"]["event"]["kind"] == "write")
        .map(|r| r["value"]["event"]["value"].as_u64().unwrap() as u32)
        .collect();
    assert_eq!(values, expected);
}
