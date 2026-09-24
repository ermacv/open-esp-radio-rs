#![cfg(target_os = "linux")]
#[allow(dead_code)]
mod support;
use blobray_application as app;
use blobray_domain::*;
use blobray_next_host::linux::LinuxHost;
use object::{
    Architecture, BinaryFormat, Endianness, RelocationFlags, SectionKind, SymbolFlags, SymbolKind,
    SymbolScope,
    write::{Object, Relocation, Symbol, SymbolSection},
};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
};
fn budget() -> ResourceBudget {
    ResourceBudget {
        mode: LimitMode::Watchdog,
        working_memory_bytes: Some(32 * 1024 * 1024),
        timeout_ms: 30000,
        poll_ms: 5,
        grace_ms: 100,
        ..Default::default()
    }
}
fn application(root: &Path) -> app::Application {
    app::Application::with_temporary_storage(
        Arc::new(LinuxHost::new(env!("CARGO_BIN_EXE_blobray").into(), None)),
        app::ApplicationLimits::default(),
        app::TemporaryStoragePolicy {
            root: Some(root.into()),
            ..Default::default()
        },
    )
    .unwrap()
}
fn symbol(name: &[u8], section: SymbolSection, size: u64, kind: SymbolKind) -> Symbol {
    Symbol {
        name: name.into(),
        value: 0,
        size,
        kind,
        scope: SymbolScope::Linkage,
        weak: false,
        section,
        flags: SymbolFlags::None,
    }
}
const BRANCH: &[u8] = &[
    1, 0, 0x63, 0x04, 0x05, 0, 0x13, 0x05, 0x15, 0, 0x67, 0x80, 0, 0,
];
fn object(bytes: &[u8], size: u64, references: bool) -> Vec<u8> {
    let mut obj = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = obj.add_section(Vec::new(), b".text.entry".to_vec(), SectionKind::Text);
    obj.append_section_data(text, bytes, 2);
    obj.add_symbol(symbol(
        b"entry",
        SymbolSection::Section(text),
        size,
        SymbolKind::Text,
    ));
    if references {
        let call = obj.add_symbol(symbol(
            b"outside",
            SymbolSection::Undefined,
            0,
            SymbolKind::Text,
        ));
        let data = obj.add_symbol(symbol(
            b"data",
            SymbolSection::Undefined,
            0,
            SymbolKind::Data,
        ));
        for (offset, symbol, kind) in [
            (0, call, object::elf::R_RISCV_CALL),
            (8, data, object::elf::R_RISCV_HI20),
            (12, data, object::elf::R_RISCV_LO12_I),
        ] {
            obj.add_relocation(
                text,
                Relocation {
                    offset,
                    symbol,
                    addend: 0,
                    flags: RelocationFlags::Elf { r_type: kind },
                },
            )
            .unwrap();
        }
    }
    obj.write().unwrap()
}
struct Fixture {
    dir: tempfile::TempDir,
    project: PathBuf,
    app: app::Application,
    request: FunctionRequest,
    revision: RevisionId,
}
fn fixture(bytes: Vec<u8>, thin: bool) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    app::create_project(&project).unwrap();
    let app = application(&dir.path().join("runtime"));
    fs::write(dir.path().join("entry.o"), &bytes).unwrap();
    let path = dir.path().join("entry.a");
    fs::write(
        &path,
        support::archive(&[(b"entry.o", &bytes), (b"entry.o", &bytes)], thin),
    )
    .unwrap();
    app.import(
        &project,
        vec![app::ImportInput {
            role: "code".into(),
            path,
            expected: None,
        }],
        Target::Riscv32Ilp32,
        budget(),
    )
    .unwrap();
    let inventory = app::inventory(&project, None).unwrap();
    let symbol = inventory.revision.inputs[0]
        .inventory
        .as_ref()
        .unwrap()
        .objects[1]
        .elf
        .as_ref()
        .unwrap()
        .symbols
        .iter()
        .find(|s| s.name.as_deref() == Some(b"entry"))
        .unwrap()
        .id
        .clone();
    let revision = inventory.revision_id;
    let request = FunctionRequest {
        research: None,
        revision: Some(revision.clone()),
        source: FunctionSource::Input { input: 0 },
        selector: (symbol).into(),
        extent: None,
    };
    Fixture {
        dir,
        project,
        app,
        request,
        revision,
    }
}
fn analyze(f: &Fixture) -> app::RunRecord {
    f.app
        .start_analyze_function(&f.project, f.request.clone(), budget())
        .unwrap()
        .wait()
}
fn export(f: &Fixture, id: FunctionAnalysisId) -> (FunctionManifest, Vec<FunctionRecord>) {
    let mut output = f
        .app
        .query(
            &f.project,
            app::ReadQuery::Analysis { id, export: true },
            budget(),
        )
        .unwrap();
    let dir = f.dir.path().join("export");
    output.export_analysis(&dir, &|| false).unwrap();
    let manifest = serde_json::from_slice(&fs::read(dir.join("manifest.json")).unwrap()).unwrap();
    let records = fs::read_to_string(dir.join("records.jsonl"))
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    (manifest, records)
}
#[test]
fn function_graph_is_retained_for_exact_thin_occurrence_and_reopens_without_sources() {
    let f = fixture(object(BRANCH, BRANCH.len() as u64, false), true);
    fs::remove_file(f.dir.path().join("entry.a")).unwrap();
    fs::remove_file(f.dir.path().join("entry.o")).unwrap();
    let run = analyze(&f);
    assert_eq!(run.state, RunState::Completed, "{:?}", run.error);
    assert_eq!(
        run.assessment
            .as_ref()
            .and_then(|a| a.coverage.as_ref())
            .map(|c| c.status == CoverageStatus::Complete),
        Some(true)
    );
    let id = run.analysis.unwrap();
    let (manifest, records) = export(&f, id.clone());
    assert_eq!(
        manifest.recipe.selector.symbol().unwrap().clone(),
        f.request.selector.symbol().unwrap().clone()
    );
    assert_eq!(manifest.instructions, 4);
    assert_eq!(manifest.blocks, 3);
    assert!(records.iter().any(|r| matches!(
        r,
        FunctionRecord::Edge {
            from: 2,
            target: Some(10),
            relation: EdgeKind::Taken,
            ..
        }
    )));
    assert_eq!(
        app::inventory(&f.project, None).unwrap().revision_id,
        f.revision
    );
    let mut larger = budget();
    larger.working_memory_bytes = Some(64 * 1024 * 1024);
    larger.timeout_ms = 60000;
    assert_eq!(
        f.app
            .start_analyze_function(&f.project, f.request.clone(), larger)
            .unwrap()
            .wait()
            .analysis,
        Some(id.clone())
    );
    assert!(matches!(
        f.app
            .query(&f.project, app::ReadQuery::Doctor, budget())
            .unwrap()
            .summary(),
        app::QuerySummary::Doctor {
            checked_analyses: 1,
            errors: 0,
            ..
        }
    ));
    f.app.shutdown();
    let moved = f.dir.path().join("moved");
    fs::rename(&f.project, &moved).unwrap();
    let app = application(&f.dir.path().join("runtime2"));
    assert!(
        app.query(
            &moved,
            app::ReadQuery::Analysis { id, export: false },
            budget()
        )
        .is_ok()
    );
}
#[test]
fn external_calls_and_data_references_do_not_require_linking() {
    let bytes = [
        0x97, 0, 0, 0, 0xe7, 0x80, 0, 0, 0x37, 0x05, 0, 0, 0x03, 0x25, 0x05, 0, 0x67, 0x80, 0, 0,
    ];
    let f = fixture(object(&bytes, bytes.len() as u64, true), false);
    let run = analyze(&f);
    assert_eq!(run.state, RunState::Completed, "{:?}", run.error);
    let (manifest, records) = export(&f, run.analysis.unwrap());
    assert!(manifest.coverage.complete());
    assert_eq!(manifest.references, 3);
    assert!(records.iter().any(|r| matches!(
        r,
        FunctionRecord::Edge {
            from: 4,
            target: None,
            relation: EdgeKind::Call,
            external: true
        }
    )));
    assert!(records.iter().any(|r|matches!(r,FunctionRecord::Reference{target,known:true,..} if target.name==b"data" && target.section.is_none())));
}
#[test]
fn unknown_extent_needs_explicit_assumption() {
    let mut f = fixture(object(BRANCH, 0, false), false);
    let failed = analyze(&f);
    assert_eq!(failed.error.unwrap().code, ErrorCode::NeedsExtent);
    assert!(failed.analysis.is_none());
    f.request.extent = Some(CodeRange {
        start: 0,
        length: BRANCH.len() as u64,
    });
    let run = analyze(&f);
    assert_eq!(run.state, RunState::Completed, "{:?}", run.error);
    let (manifest, _) = export(&f, run.analysis.unwrap());
    assert!(manifest.recipe.user_extent);
}
#[test]
fn unknown_instructions_and_indirect_jump_retain_partial_results() {
    for bytes in [&[0xff, 0xff, 0xff, 0xff][..], &[0x67, 0, 0x03, 0][..]] {
        let f = fixture(object(bytes, bytes.len() as u64, false), false);
        let run = analyze(&f);
        assert_eq!(run.state, RunState::Completed, "{:?}", run.error);
        assert_eq!(
            run.assessment
                .as_ref()
                .and_then(|a| a.coverage.as_ref())
                .map(|c| c.status == CoverageStatus::Complete),
            Some(false)
        );
        let (manifest, records) = export(&f, run.analysis.unwrap());
        assert!(!manifest.coverage.control_flow);
        assert!(!records.is_empty());
    }
}
#[test]
fn cyclic_function_terminates_and_capacity_failure_does_not_publish() {
    let f = fixture(object(&[0x6f, 0, 0, 0], 4, false), false);
    let run = analyze(&f);
    assert_eq!(run.state, RunState::Completed, "{:?}", run.error);
    let (manifest, _) = export(&f, run.analysis.unwrap());
    assert_eq!(manifest.instructions, 1);
    let mut small = budget();
    small.working_memory_bytes = Some(1024);
    let run = f
        .app
        .start_analyze_function(&f.project, f.request.clone(), small)
        .unwrap()
        .wait();
    assert_eq!(run.state, RunState::ResourceLimited);
    assert!(run.analysis.is_none());
    assert_eq!(
        app::inventory(&f.project, None).unwrap().revision_id,
        f.revision
    );
}
#[test]
fn cli_analyzes_lists_reads_and_exports() {
    let f = fixture(object(BRANCH, BRANCH.len() as u64, false), false);
    let request = f.dir.path().join("request.json");
    fs::write(&request, serde_json::to_vec(&f.request).unwrap()).unwrap();
    let run = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args(["analyze-function", "--project"])
        .arg(&f.project)
        .arg("--request")
        .arg(&request)
        .args(["--limit-mode", "watchdog", "--format", "json"])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&run.stdout).unwrap();
    let id = report["run"]["analysis"].as_str().unwrap();
    for command in ["analyses", "analysis", "export-analysis"] {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_blobray"));
        cmd.arg(command).arg("--project").arg(&f.project).args([
            "--limit-mode",
            "watchdog",
            "--format",
            "json",
        ]);
        if command != "analyses" {
            cmd.args(["--id", id]);
        }
        if command == "export-analysis" {
            cmd.arg("--output").arg(f.dir.path().join("cli-export"));
        }
        let result = cmd.output().unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
}

#[test]
fn human_cli_reports_extent_failure_and_partial_coverage() {
    for (bytes, size, expected, success) in [
        (BRANCH, 0, "NeedsExtent", false),
        (&[0xff, 0xff, 0xff, 0xff][..], 4, "(partial)", true),
        (BRANCH, BRANCH.len() as u64, "(complete)", true),
    ] {
        let f = fixture(object(bytes, size, false), false);
        let request = f.dir.path().join("request.json");
        fs::write(&request, serde_json::to_vec(&f.request).unwrap()).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_blobray"))
            .args(["analyze-function", "--project"])
            .arg(&f.project)
            .arg("--request")
            .arg(&request)
            .args(["--limit-mode", "watchdog"])
            .output()
            .unwrap();
        assert_eq!(output.status.success(), success);
        let text = String::from_utf8_lossy(if success {
            &output.stdout
        } else {
            &output.stderr
        });
        assert!(text.contains(expected), "{text}");
        if !success {
            assert!(text.contains("extent"), "{text}");
        }
    }
}

#[test]
fn target_inside_instruction_and_truncation_are_visible_gaps() {
    // beq zero, zero, +2 targets the second halfword of itself.
    for bytes in [&[0x63, 0x01, 0, 0, 0x67, 0x80, 0, 0][..], &[0x13, 0, 0][..]] {
        let f = fixture(object(bytes, bytes.len() as u64, false), false);
        let run = analyze(&f);
        assert_eq!(run.state, RunState::Completed, "{:?}", run.error);
        assert_eq!(
            run.assessment
                .as_ref()
                .and_then(|a| a.coverage.as_ref())
                .map(|c| c.status == CoverageStatus::Complete),
            Some(false)
        );
        let (_, records) = export(&f, run.analysis.unwrap());
        assert!(records.iter().any(|r| matches!(
            r,
            FunctionRecord::Gap { .. }
                | FunctionRecord::Edge {
                    relation: EdgeKind::Conflict,
                    ..
                }
        )));
    }
}
#[test]
fn corrupt_saved_records_do_not_trigger_reanalysis() {
    let f = fixture(object(BRANCH, BRANCH.len() as u64, false), false);
    let id = analyze(&f).analysis.unwrap();
    let (manifest, _) = export(&f, id.clone());
    fs::write(
        f.project
            .join(".blobray-next/objects")
            .join(manifest.records.as_str()),
        b"corrupt",
    )
    .unwrap();
    assert_eq!(
        f.app
            .query(
                &f.project,
                app::ReadQuery::Analysis { id, export: false },
                budget()
            )
            .err()
            .unwrap()
            .code,
        ErrorCode::Integrity
    );
    assert!(
        matches!(f.app.query(&f.project,app::ReadQuery::Doctor,budget()).unwrap().summary(),app::QuerySummary::Doctor{errors,..} if *errors>0)
    );
}

#[test]
fn relax_with_null_symbol_is_metadata_not_missing_external_definition() {
    use object::{Object as _, ObjectSection as _};
    let code = [
        0x97, 0, 0, 0, 0xe7, 0x80, 0, 0, 0x37, 0x05, 0, 0, 0x03, 0x25, 0x05, 0, 0x67, 0x80, 0, 0,
    ];
    let mut bytes = object(&code, code.len() as u64, true);
    let file = object::File::parse(&*bytes).unwrap();
    let (offset, _) = file
        .section_by_name(".rela.text.entry")
        .unwrap()
        .file_range()
        .unwrap();
    // Replace the third relocation with a valid r_sym=0 RELAX marker.
    let info = offset as usize + 2 * 12 + 4;
    bytes[info..info + 4].copy_from_slice(&object::elf::R_RISCV_RELAX.to_le_bytes());
    let f = fixture(bytes, false);
    let run = analyze(&f);
    assert_eq!(run.state, RunState::Completed, "{:?}", run.error);
    let (_, records) = export(&f, run.analysis.unwrap());
    assert!(records.iter().any(|r|matches!(r,FunctionRecord::Reference{raw,reference_kind:ReferenceKind::Metadata,known:true,..} if raw.target.definition==SymbolDefinition::Null && raw.target.symbol.index==0)));
}

#[test]
fn function_cancellation_timeout_and_disk_limit_never_publish() {
    let mut code = [1, 0].repeat(20000);
    code.extend_from_slice(&[0x67, 0x80, 0, 0]);
    let f = fixture(object(&code, code.len() as u64, false), false);
    let job = f
        .app
        .start_analyze_function(&f.project, f.request.clone(), budget())
        .unwrap();
    assert!(job.cancel());
    assert_eq!(job.wait().state, RunState::Cancelled);
    let mut short = budget();
    short.timeout_ms = 1;
    let run = f
        .app
        .start_analyze_function(&f.project, f.request.clone(), short)
        .unwrap()
        .wait();
    assert_eq!(run.state, RunState::TimedOut);
    let limited = app::Application::with_temporary_storage(
        Arc::new(LinuxHost::new(env!("CARGO_BIN_EXE_blobray").into(), None)),
        app::ApplicationLimits::default(),
        app::TemporaryStoragePolicy {
            root: Some(f.dir.path().join("limited")),
            operation_bytes: 2 * TEMPORARY_CONTROL_BYTES,
            total_bytes: 4 * TEMPORARY_CONTROL_BYTES,
        },
    )
    .unwrap();
    let run = limited
        .start_analyze_function(&f.project, f.request.clone(), budget())
        .unwrap()
        .wait();
    assert_eq!(run.state, RunState::ResourceLimited, "{:?}", run.error);
    assert!(run.error.unwrap().storage.is_some());
    assert_eq!(limited.temporary_storage_status().reserved_bytes, 0);
    assert!(matches!(
        f.app
            .query(&f.project, app::ReadQuery::Analyses, budget())
            .unwrap()
            .summary(),
        app::QuerySummary::Analyses { count: 0 }
    ));
    assert_eq!(
        app::inventory(&f.project, None).unwrap().revision_id,
        f.revision
    );
}
#[test]
fn killed_function_coordinator_requires_explicit_recovery() {
    use std::{
        process::Stdio,
        time::{Duration, Instant},
    };
    let code = [1, 0].repeat(262144);
    let f = fixture(object(&code, code.len() as u64, false), false);
    let request = f.dir.path().join("request.json");
    fs::write(&request, serde_json::to_vec(&f.request).unwrap()).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args(["analyze-function", "--project"])
        .arg(&f.project)
        .arg("--request")
        .arg(request)
        .args(["--limit-mode", "watchdog"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(runs) = app::runs(&f.project)
            && runs.iter().any(|r| {
                matches!(r.operation, app::RunOperation::AnalyzeFunction { .. })
                    && r.state == RunState::Running
            })
        {
            break;
        }
        assert!(Instant::now() < deadline, "worker did not start");
        std::thread::sleep(Duration::from_millis(5));
    }
    child.kill().unwrap();
    child.wait().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let recovered = loop {
        match f.app.recover(&f.project) {
            Ok(r) => break r,
            Err(e) if e.code == ErrorCode::Busy && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10))
            }
            Err(e) => panic!("{e}"),
        }
    };
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].state, RunState::Abandoned);
    assert!(recovered[0].analysis.is_none());
    assert!(f.app.recover(&f.project).unwrap().is_empty());
    assert_eq!(
        app::inventory(&f.project, None).unwrap().revision_id,
        f.revision
    );
}

fn words(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}
#[test]
fn values_and_memory_effects_survive_export_without_origins() {
    let code = words(&[
        0x60000537, 0x12050513, 0x02a00593, 0x00b52223, 0x00852603, 0xff010113, 0x00112623,
        0x00900013, 0x00008067,
    ]);
    let f = fixture(object(&code, code.len() as u64, false), false);
    fs::remove_file(f.dir.path().join("entry.a")).unwrap();
    fs::remove_file(f.dir.path().join("entry.o")).unwrap();
    let run = analyze(&f);
    assert_eq!(
        run.assessment
            .as_ref()
            .and_then(|a| a.coverage.as_ref())
            .map(|c| c.status == CoverageStatus::Complete),
        Some(true),
        "{:?}",
        run.error
    );
    let id = run.analysis.unwrap();
    let (manifest, records) = export(&f, id.clone());
    assert_eq!(manifest.schema, FUNCTION_SCHEMA);
    assert!(manifest.recipe.semantics.is_some());
    let s = manifest.semantics.unwrap();
    assert_eq!(s.accesses, 3);
    assert_eq!(s.known_addresses, 3);
    assert!(records.iter().any(|r| matches!(
        r,
        FunctionRecord::MemoryAccess {
            offset: 12,
            access: MemoryKind::Store,
            width: 4,
            address: AbstractValue::Constant { value: 0x60000124 },
            value: Some(AbstractValue::Constant { value: 42 }),
            ..
        }
    )));
    assert!(records.iter().any(|r| matches!(
        r,
        FunctionRecord::MemoryAccess {
            offset: 16,
            access: MemoryKind::Load,
            address: AbstractValue::Constant { value: 0x60000128 },
            value: None,
            ..
        }
    )));
    assert!(records.iter().any(|r| matches!(
        r,
        FunctionRecord::Value {
            register: 12,
            value: AbstractValue::Expression { .. },
            ..
        }
    )));
    assert!(records.iter().any(|r| matches!(
        r,
        FunctionRecord::Value {
            register: 0,
            value: AbstractValue::Constant { value: 0 },
            ..
        }
    )));
    assert!(records.iter().any(|r| matches!(
        r,
        FunctionRecord::MemoryAccess {
            offset: 24,
            address: AbstractValue::EntryStack { offset: -4 },
            ..
        }
    )));
    let human = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args(["analysis", "--project"])
        .arg(&f.project)
        .args(["--id", id.as_str(), "--limit-mode", "watchdog"])
        .output()
        .unwrap();
    assert!(human.status.success());
    let text = String::from_utf8(human.stdout).unwrap();
    assert!(
        text.contains("Store 32-bit [0x60000124] <- 0x0000002a"),
        "{text}"
    );
    assert!(text.contains("entry-sp-4"));
    assert!(text.contains("Semantics: complete"));
}
#[test]
fn opaque_calls_and_atomic_accesses_preserve_unknowns() {
    // li a0, 64; jal ra, +128; lw a1, 0(a0); lr.w a2,(a0);
    // sc.w a3,a1,(a0); amoadd.w a4,a1,(a0); ret
    let code = words(&[
        0x04000513, 0x080000ef, 0x00052583, 0x1005262f, 0x18b526af, 0x00b5272f, 0x00008067,
    ]);
    let f = fixture(object(&code, code.len() as u64, false), false);
    let run = analyze(&f);
    assert_eq!(run.state, RunState::Completed);
    assert_eq!(
        run.assessment
            .as_ref()
            .and_then(|a| a.coverage.as_ref())
            .map(|c| c.status == CoverageStatus::Complete),
        Some(false)
    );
    let (m, records) = export(&f, run.analysis.unwrap());
    assert!(!m.semantics.unwrap().complete);
    for kind in [
        MemoryKind::Load,
        MemoryKind::LoadReserved,
        MemoryKind::StoreConditional,
        MemoryKind::Atomic,
    ] {
        assert!(records.iter().any(|r|matches!(r,FunctionRecord::MemoryAccess{access,address:AbstractValue::Unknown | AbstractValue::Expression { .. },..} if *access==kind)),"{kind:?}");
    }
    assert!(records.iter().any(|r| matches!(
        r,
        FunctionRecord::SemanticGap {
            reason: SemanticGapReason::OpaqueCall,
            ..
        }
    )));
}

fn pcrel_object(clobber: bool, ambiguous: bool) -> Vec<u8> {
    let code = words(&[
        0x00000517,
        0x02a00593,
        if clobber { 0x00000513 } else { 0x00000013 },
        0x00052603,
        0x00008067,
    ]);
    let mut obj = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = obj.add_section(Vec::new(), b".text.entry".to_vec(), SectionKind::Text);
    obj.append_section_data(text, &code, 4);
    obj.add_symbol(symbol(
        b"entry",
        SymbolSection::Section(text),
        code.len() as u64,
        SymbolKind::Text,
    ));
    let label = obj.add_symbol(symbol(
        b"hi_label",
        SymbolSection::Section(text),
        0,
        SymbolKind::Text,
    ));
    let data = obj.add_symbol(symbol(
        b"data",
        SymbolSection::Undefined,
        0,
        SymbolKind::Data,
    ));
    for (offset, symbol, r_type, addend) in [
        (0, data, object::elf::R_RISCV_PCREL_HI20, 4),
        (12, label, object::elf::R_RISCV_PCREL_LO12_I, 0),
    ] {
        obj.add_relocation(
            text,
            Relocation {
                offset,
                symbol,
                addend,
                flags: RelocationFlags::Elf { r_type },
            },
        )
        .unwrap();
    }
    if ambiguous {
        obj.add_relocation(
            text,
            Relocation {
                offset: 0,
                symbol: data,
                addend: 8,
                flags: RelocationFlags::Elf {
                    r_type: object::elf::R_RISCV_PCREL_HI20,
                },
            },
        )
        .unwrap();
    }
    obj.write().unwrap()
}
#[test]
fn relocated_addresses_require_identity_pairing_and_unclobbered_register() {
    for (clobber, ambiguous) in [(false, false), (true, false), (false, true)] {
        let f = fixture(pcrel_object(clobber, ambiguous), false);
        let run = analyze(&f);
        assert_eq!(run.state, RunState::Completed, "{:?}", run.error);
        let (m, records) = export(&f, run.analysis.unwrap());
        let access = records
            .iter()
            .find_map(|r| match r {
                FunctionRecord::MemoryAccess {
                    offset: 12,
                    address,
                    relocation,
                    ..
                } => Some((address, relocation)),
                _ => None,
            })
            .unwrap();
        if clobber || ambiguous {
            assert_eq!(access.0, &AbstractValue::Unknown);
            assert!(!m.semantics.unwrap().complete);
        } else {
            assert!(m.semantics.unwrap().complete);
            let target = records
                .iter()
                .find_map(|r| match r {
                    FunctionRecord::Reference { raw, target, .. } if raw.offset == 0 => {
                        Some(&target.symbol)
                    }
                    _ => None,
                })
                .unwrap();
            assert_eq!(
                access.0,
                &AbstractValue::Symbol {
                    symbol: target.clone(),
                    addend: 4
                }
            );
            assert!(access.1.is_some());
        }
        // AUIPC upper alone is not the final relocated address.
        assert!(records.iter().any(|r| matches!(
            r,
            FunctionRecord::Value {
                offset: 0,
                value: AbstractValue::Unknown,
                ..
            }
        )));
    }
}

#[test]
fn value_state_capacity_failure_has_its_own_phase_and_never_publishes() {
    let mut code = [1, 0].repeat(1000);
    code.extend_from_slice(&0x00008067u32.to_le_bytes());
    let f = fixture(object(&code, code.len() as u64, false), false);
    let mut small = budget();
    small.working_memory_bytes = Some(2 * 1024 * 1024);
    let run = f
        .app
        .start_analyze_function(&f.project, f.request.clone(), small)
        .unwrap()
        .wait();
    assert_eq!(run.state, RunState::ResourceLimited, "{:?}", run.error);
    assert_eq!(
        run.error.unwrap().memory.unwrap().phase,
        RunPhase::AnalyzeValues
    );
    assert!(run.analysis.is_none());
    assert!(matches!(
        f.app
            .query(&f.project, app::ReadQuery::Analyses, budget())
            .unwrap()
            .summary(),
        app::QuerySummary::Analyses { count: 0 }
    ));
    assert_eq!(
        app::inventory(&f.project, None).unwrap().revision_id,
        f.revision
    );
    assert_eq!(f.app.temporary_storage_status().reserved_bytes, 0);
}

#[test]
fn relocated_tail_call_is_an_opaque_effect_not_an_unknown_address_pair() {
    let code = words(&[0x00000317, 0x00030067]);
    let mut obj = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = obj.add_section(Vec::new(), b".text.entry".to_vec(), SectionKind::Text);
    obj.append_section_data(text, &code, 4);
    obj.add_symbol(symbol(
        b"entry",
        SymbolSection::Section(text),
        8,
        SymbolKind::Text,
    ));
    let callee = obj.add_symbol(symbol(
        b"tail",
        SymbolSection::Undefined,
        0,
        SymbolKind::Text,
    ));
    obj.add_relocation(
        text,
        Relocation {
            offset: 0,
            symbol: callee,
            addend: 0,
            flags: RelocationFlags::Elf {
                r_type: object::elf::R_RISCV_CALL,
            },
        },
    )
    .unwrap();
    let f = fixture(obj.write().unwrap(), false);
    let run = analyze(&f);
    assert_eq!(
        run.assessment
            .as_ref()
            .and_then(|a| a.coverage.as_ref())
            .map(|c| c.status == CoverageStatus::Complete),
        Some(false)
    );
    let (manifest, records) = export(&f, run.analysis.unwrap());
    assert_eq!(manifest.semantics.unwrap().gaps, 1);
    assert!(records.iter().any(|r| matches!(
        r,
        FunctionRecord::SemanticGap {
            offset: 4,
            reason: SemanticGapReason::OpaqueCall
        }
    )));
    assert!(!records.iter().any(|r| matches!(
        r,
        FunctionRecord::SemanticGap {
            reason: SemanticGapReason::UnresolvedRelocation,
            ..
        }
    )));
}

#[path = "functions/investigations.rs"]
mod investigations;

#[path = "functions/data.rs"]
mod data;
#[path = "functions/interfaces.rs"]
mod interfaces;
#[path = "functions/knowledge.rs"]
mod knowledge;

#[test]
fn ten_thousand_section_relocations_fit_small_function_capacity() {
    let mut obj = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = obj.add_section(Vec::new(), b".text.entry".to_vec(), SectionKind::Text);
    obj.append_section_data(text, &words(&vec![0x00008067; 10001]), 4);
    obj.add_symbol(symbol(
        b"entry",
        SymbolSection::Section(text),
        4,
        SymbolKind::Text,
    ));
    let external = obj.add_symbol(symbol(
        b"external",
        SymbolSection::Undefined,
        0,
        SymbolKind::Data,
    ));
    for i in 1..=10000 {
        obj.add_relocation(
            text,
            Relocation {
                offset: i * 4,
                symbol: external,
                addend: 0,
                flags: RelocationFlags::Elf {
                    r_type: object::elf::R_RISCV_32,
                },
            },
        )
        .unwrap();
    }
    let f = fixture(obj.write().unwrap(), false);
    let mut limits = budget();
    limits.working_memory_bytes = Some(16 * 1024 * 1024);
    let run = f
        .app
        .start_analyze_function(&f.project, f.request.clone(), limits)
        .unwrap()
        .wait();
    assert_eq!(run.state, RunState::Completed, "{:?}", run.error);
    let progress = run.diagnostics.unwrap().progress.unwrap();
    assert_eq!(progress.measurements.objects_prepared, 1);
    assert_eq!(progress.measurements.sections_prepared, 1);
    assert!(progress.working_memory.unwrap().peak_reserved_bytes < 16 * 1024 * 1024);
}

#[path = "functions/reports.rs"]
mod reports;

#[path = "functions/ranges.rs"]
mod ranges;

#[path = "functions/contracts.rs"]
mod contracts;

#[path = "functions/navigation.rs"]
mod navigation;

#[path = "functions/event_routes.rs"]
mod event_routes;
#[path = "functions/memory_slice.rs"]
mod memory_slice;

#[path = "functions/registers.rs"]
mod registers;

#[path = "functions/semantic_ir.rs"]
mod semantic_ir;
