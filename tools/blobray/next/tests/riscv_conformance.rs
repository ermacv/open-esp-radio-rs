//! Executor conformance with the official RISC-V architectural tests.
//!
//! Each test of the selected RV32 suites is assembled with the caller's clang
//! against the target model in `riscv_conformance/`, executed through
//! in-process verification until it jumps to the executor's return sentinel,
//! and its signature (the memory between `begin_signature` and
//! `end_signature`) must equal the suite's reference output, which the RISC-V
//! Sail formal model produced.
//!
//! The suite is external input: set `BLOBRAY_RISCV_ARCH_TEST` to a checkout of
//! a riscv-arch-test release that carries reference outputs (2.x) and
//! `BLOBRAY_RISCV_CC` to a clang with the riscv32 target and lld, then run
//! `cargo test -p blobray-next --test riscv_conformance -- --ignored`.
use blobray_application as app;
use blobray_domain::*;
use object::{Object, ObjectSymbol};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Suites whose instructions the executor implements, each with the
/// instruction set its references were produced for. Link addresses in the
/// signatures depend on instruction sizes, so compressed encodings are
/// enabled only for the C suite.
const SUITES: [(&str, &str); 4] = [
    ("I", "rv32i"),
    ("M", "rv32im"),
    ("C", "rv32ic"),
    ("Zifencei", "rv32i_zifencei"),
];
/// Stack mapped for every test; the tests do not use it before writing it.
const STACK: u32 = 0x9000_0000;
const STACK_BYTES: u32 = 4096;
/// Tests that need machine-mode state the executor does not model, with the
/// reason. Each must still fail, so an entry cannot outlive its cause.
const EXCLUDED: &[(&str, &str)] = &[
    (
        "cebreak-01",
        "c.ebreak raises a breakpoint trap handled in machine mode, which the executor stops at",
    ),
    (
        "Fencei",
        "the test stores into its own code, and image loading rejects writable code segments",
    ),
];

fn environment(name: &str) -> PathBuf {
    std::env::var_os(name)
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("{name} must name the conformance input"))
}

fn assemble(
    cc: &Path,
    suite: &Path,
    march: &str,
    source: &Path,
    output: &Path,
) -> std::result::Result<(), String> {
    let model = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/riscv_conformance");
    let status = Command::new(cc)
        .args(["--target=riscv32", "-mabi=ilp32", "-nostdlib", "-static"])
        .arg(format!("-march={march}"))
        .args(["-fuse-ld=lld", "-DXLEN=32"])
        .arg("-I")
        .arg(suite.join("riscv-test-suite/env"))
        .arg("-I")
        .arg(&model)
        .arg("-T")
        .arg(model.join("link.ld"))
        .arg(source)
        .arg("-o")
        .arg(output)
        .output()
        .map_err(|e| e.to_string())?;
    if status.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&status.stderr).into_owned())
    }
}

fn symbol(file: &object::File<'_>, name: &str) -> u32 {
    file.symbols()
        .find(|s| s.name() == Ok(name))
        .map(|s| s.address() as u32)
        .unwrap_or_else(|| panic!("missing symbol {name}"))
}

/// The signature words, or why the test did not produce one.
fn run(elf: &[u8]) -> std::result::Result<Vec<u32>, String> {
    let file = object::File::parse(elf).map_err(|e| e.to_string())?;
    let (entry, begin, end) = (
        symbol(&file, "rvtest_entry_point"),
        symbol(&file, "begin_signature"),
        symbol(&file, "end_signature"),
    );
    let target = ExecutionTarget {
        revision: ArtifactId::of_bytes(elf).as_str().parse().unwrap(),
        source: FunctionSource::Input { input: 0 },
        companions: vec![],
        abi: CallAbi::RiscvInteger,
        stack: MemorySeed {
            address: STACK,
            length: STACK_BYTES,
            fill: Some(0),
            bytes: vec![],
        },
    };
    let request = ExecutionRequest {
        schema: EXECUTION_SCHEMA,
        vendor: target,
        replacement: None,
        binding: None,
        max_events: 16,
        cases: vec![ExecutionCase {
            name: "conformance".into(),
            reset: SessionReset::Cold,
            stack_fill: None,
            relation: None,
            vendor: Invocation {
                entry,
                goal: ExecutionGoal::Return,
                arguments: vec![Some(0); 8],
                memory: vec![],
                models: vec![],
                calls: vec![],
                tables: vec![],
                services: vec![],
                observe_memory: vec![MemorySelection {
                    name: "signature".into(),
                    address: begin,
                    length: end - begin,
                }],
                observe_calls: None,
                observe_timeline: TimelineCapture::default(),
            },
            replacement: None,
        }],
    };
    let memory = WorkingMemory::new(64 * 1024 * 1024).unwrap();
    let result = app::in_process::verify(
        &app::in_process::InProcessComparison {
            request: &request,
            vendor: &[elf],
            replacement: None,
            effects: &[],
            projections: &[],
        },
        &blobray_backend_riscv::RiscvExecutor,
        &memory,
        &mut || Ok(()),
    )
    .map_err(|e| format!("{e:?}"))?;
    let mut bytes = vec![0u8; (end - begin) as usize];
    for record in &result.records {
        match record {
            ExecutionEvidence::Outcome { stop, .. } if !stop.completed() => {
                return Err(format!("did not complete: {stop:?}"));
            }
            ExecutionEvidence::FinalMemory { chunk, .. } => {
                let mask = chunk.mask().unwrap();
                if chunk.known & mask != mask {
                    return Err(format!("unknown signature bytes at {:#x}", chunk.offset));
                }
                let at = chunk.offset as usize;
                bytes[at..at + chunk.length as usize]
                    .copy_from_slice(&chunk.bytes[..chunk.length as usize]);
            }
            _ => {}
        }
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|w| u32::from_le_bytes(w.try_into().unwrap()))
        .collect())
}

fn reference(path: &Path) -> Vec<u32> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| u32::from_str_radix(l.trim(), 16).unwrap())
        .collect()
}

#[test]
#[ignore = "needs the RISC-V architectural tests and a riscv32 clang"]
fn architectural_tests_match_their_reference_signatures() {
    let suite = environment("BLOBRAY_RISCV_ARCH_TEST");
    let cc = environment("BLOBRAY_RISCV_CC");
    let output = tempfile::tempdir().unwrap();
    let mut passed = 0;
    let mut failures = vec![];
    let mut excluded_passing = vec![];
    for (extension, march) in SUITES {
        let root = suite.join("riscv-test-suite/rv32i_m").join(extension);
        let mut sources: Vec<_> = std::fs::read_dir(root.join("src"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "S"))
            .collect();
        sources.sort();
        for source in sources {
            let name = source.file_stem().unwrap().to_string_lossy().into_owned();
            let elf = output.path().join(format!("{extension}-{name}.elf"));
            let outcome = assemble(&cc, &suite, march, &source, &elf).and_then(|()| {
                let signature = run(&std::fs::read(&elf).unwrap())?;
                let expected = reference(
                    &root
                        .join("references")
                        .join(format!("{name}.reference_output")),
                );
                if signature == expected {
                    Ok(())
                } else {
                    let first = signature
                        .iter()
                        .zip(&expected)
                        .position(|(a, b)| a != b)
                        .unwrap_or(signature.len().min(expected.len()));
                    Err(format!(
                        "signature word {first} differs ({} words, {} expected)",
                        signature.len(),
                        expected.len()
                    ))
                }
            });
            let excluded = EXCLUDED.iter().find(|(test, _)| *test == name);
            match (outcome, excluded) {
                (Ok(()), None) => passed += 1,
                (Ok(()), Some(_)) => excluded_passing.push(name),
                (Err(_), Some(_)) => {}
                (Err(reason), None) => failures.push(format!("{extension}/{name}: {reason}")),
            }
        }
    }
    eprintln!("{passed} architectural tests passed");
    assert!(
        excluded_passing.is_empty(),
        "exclusions that now pass: {excluded_passing:?}"
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
