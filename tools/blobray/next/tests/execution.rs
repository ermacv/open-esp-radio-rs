#![cfg(target_os = "linux")]
use blobray_application as app;
use blobray_domain::*;
use blobray_next_host::linux::LinuxHost;
use std::{fs, path::PathBuf, process::Command, sync::Arc};
fn budget() -> ResourceBudget {
    ResourceBudget {
        mode: LimitMode::Watchdog,
        poll_ms: 5,
        grace_ms: 100,
        timeout_ms: 30000,
        working_memory_bytes: Some(32 * 1024 * 1024),
        ..Default::default()
    }
}
fn elf(code: &[u32]) -> Vec<u8> {
    let mut bytes = vec![0; 256];
    bytes[..7].copy_from_slice(b"\x7fELF\x01\x01\x01");
    for (offset, value) in [(16, 2u16), (18, 243), (40, 52), (42, 32), (44, 1)] {
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }
    for (offset, value) in [
        (20, 1u32),
        (24, 0x1000),
        (28, 52),
        (52, 1),
        (56, 256),
        (60, 0x1000),
        (64, 0x1000),
        (68, (code.len() * 4) as u32),
        (72, (code.len() * 4) as u32),
        (76, 5),
        (80, 4),
    ] {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    for op in code {
        bytes.extend_from_slice(&op.to_le_bytes());
    }
    bytes
}
struct Fixture {
    _dir: tempfile::TempDir,
    project: PathBuf,
    app: app::Application,
    target: ExecutionTarget,
}
impl Fixture {
    fn new(code: &[u32]) -> Self {
        Self::from_inputs(vec![elf(code)])
    }
    fn from_inputs(inputs: Vec<Vec<u8>>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        app::create_project(&project).unwrap();
        let paths: Vec<_> = inputs
            .iter()
            .enumerate()
            .map(|(i, bytes)| {
                let path = dir.path().join(format!("input-{i}.elf"));
                fs::write(&path, bytes).unwrap();
                path
            })
            .collect();
        let app = app::Application::with_temporary_storage(
            Arc::new(LinuxHost::new(env!("CARGO_BIN_EXE_blobray").into(), None)),
            app::ApplicationLimits::default(),
            app::TemporaryStoragePolicy {
                root: Some(dir.path().join("runtime")),
                ..Default::default()
            },
        )
        .unwrap();
        let run = app
            .import(
                &project,
                paths
                    .iter()
                    .map(|path| app::ImportInput {
                        role: "code".into(),
                        path: path.clone(),
                        expected: None,
                    })
                    .collect(),
                Target::Riscv32Ilp32,
                budget(),
            )
            .unwrap();
        assert_eq!(run.state, RunState::Completed, "{run:?}");
        for path in paths {
            fs::remove_file(path).unwrap();
        }
        let target = ExecutionTarget {
            revision: run.revision.unwrap(),
            source: FunctionSource::Input { input: 0 },
            companions: vec![],
            abi: CallAbi::RiscvInteger,
            stack: MemorySeed {
                address: 0x8000,
                length: 4096,
                fill: None,
                bytes: vec![],
            },
        };
        Self {
            _dir: dir,
            project,
            app,
            target,
        }
    }
    fn request(&self) -> ExecutionRequest {
        let invocation = Invocation {
            observe_calls: None,
            observe_timeline: TimelineCapture::default(),
            observe_memory: vec![],
            goal: ExecutionGoal::Return,
            entry: 0x1000,
            arguments: vec![Some(0); 8],
            memory: vec![],
            models: vec![],
            calls: vec![],
            tables: vec![],
            services: vec![],
        };
        ExecutionRequest {
            schema: EXECUTION_SCHEMA,
            vendor: self.target.clone(),
            replacement: Some(self.target.clone()),
            binding: Some(CompiledBinding::SharedCore),
            cases: vec![ExecutionCase {
                relation: Some(fixture_relation(true)),
                reset: SessionReset::Cold,
                name: "case".into(),
                vendor: invocation.clone(),
                replacement: Some(invocation),
            }],

            max_events: 16,
        }
    }
    fn run(&self, r: ExecutionRequest, b: ResourceBudget) -> app::RunRecord {
        self.app
            .start_execution(&self.project, r, &blobray_backend_riscv::RiscvExecutor, b)
            .unwrap()
            .wait()
    }
    fn read(&self, id: &ArtifactId) -> serde_json::Value {
        let output = Command::new(env!("CARGO_BIN_EXE_blobray"))
            .args(["--format", "json", "execution", "--project"])
            .arg(&self.project)
            .args(["--id", id.as_str(), "--limit-mode", "watchdog"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
}
#[test]
fn comparison_replay_and_preservation_use_captured_bytes() {
    let f = Fixture::new(&[0x00012503, 0x00150513, 0x00008067]);
    let mut request = f.request();
    request.cases[0].vendor.arguments.push(Some(7));
    request.cases[0].replacement = Some(request.cases[0].vendor.clone());
    let run = f.run(request.clone(), budget());
    assert_eq!(run.state, RunState::Completed, "{run:?}");
    let id = run.execution.unwrap();
    let result = f.read(&id);
    assert_eq!(result["summary"]["manifest"]["verdict"], "MATCH");
    let typed: blobray_next_host::wire::RecordDocument<ExecutionEvidence> =
        serde_json::from_value(result).unwrap();
    assert_eq!(typed.schema, blobray_next_host::wire::RECORDS_SCHEMA);
    assert!(typed.records.iter().all(|r| r.kind == "execution"));
    assert!(typed.records.iter().any(|r| matches!(
        r.value,
        ExecutionEvidence::Comparison {
            result: CaseComparison {
                verdict: ComparisonVerdict::Match,
                ..
            },
            ..
        }
    )));
    let app::QuerySummary::Execution { manifest, .. } = typed.summary else {
        panic!("execution query returns an execution summary")
    };
    assert_eq!(manifest.verdict, Some(ComparisonVerdict::Match));
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
    let r: blobray_next_host::wire::RunDocument = serde_json::from_slice(&replay.stdout).unwrap();
    assert_eq!(r.run.execution.as_ref(), Some(&id));
    let mut different = request;
    different.cases[0].replacement.as_mut().unwrap().arguments[8] = Some(9);
    let run = f.run(different, budget());
    assert_eq!(
        f.read(&run.execution.unwrap())["summary"]["manifest"]["verdict"],
        "DIFF"
    );
    let backup = f._dir.path().join("backup.blobray");
    let restored = f._dir.path().join("restored");
    for (cmd, path, args) in [
        (
            "backup",
            &f.project,
            vec!["--output", backup.to_str().unwrap()],
        ),
        (
            "restore",
            &restored,
            vec!["--backup", backup.to_str().unwrap()],
        ),
    ] {
        let status = Command::new(env!("CARGO_BIN_EXE_blobray"))
            .args([cmd, "--project"])
            .arg(path)
            .args(["--limit-mode", "watchdog"])
            .args(args)
            .output()
            .unwrap();
        assert!(
            status.status.success(),
            "{}",
            String::from_utf8_lossy(&status.stderr)
        );
    }
    let moved = f._dir.path().join("moved");
    fs::rename(&restored, &moved).unwrap();
    let status = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args(["execution", "--project"])
        .arg(&moved)
        .args(["--id", id.as_str(), "--limit-mode", "watchdog"])
        .output()
        .unwrap();
    assert!(status.status.success());
    let replay = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args(["--format", "json", "replay", "--project"])
        .arg(&moved)
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
}
#[test]
fn concrete_memory_state_and_unknowns_are_not_invented() {
    let f = Fixture::new(&[0x00052283, 0x00128293, 0x00552023, 0x00028513, 0x00008067]);
    let mut r = f.request();
    r.replacement = None;
    r.binding = None;
    r.cases[0].replacement = None;
    r.cases[0].relation = None;
    r.cases[0].vendor.arguments[0] = Some(0x3000);
    r.cases[0].vendor.memory.push(ram(MemorySeed {
        address: 0x3000,
        length: 4,
        fill: Some(0),
        bytes: vec![],
    }));
    let mut second = r.cases[0].clone();
    second.reset = SessionReset::Warm;
    second.name = "second".into();
    second.vendor.memory.clear();
    r.cases.push(second);
    let run = f.run(r.clone(), budget());
    let facts = f.read(&run.execution.unwrap());
    let low: Vec<_> = facts["records"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["value"]["stop"]["low"].as_u64())
        .collect();
    assert_eq!(low, vec![1, 2]);
    r.cases[1].reset = SessionReset::Cold;
    let run = f.run(r.clone(), budget());
    assert_eq!(
        run.assessment
            .as_ref()
            .and_then(|a| a.coverage.as_ref())
            .map(|c| c.status == CoverageStatus::Complete),
        Some(false)
    );
    r.cases[1].reset = SessionReset::Warm;
    r.cases[0].vendor.memory.clear();
    let run = f.run(r, budget());
    let facts = f.read(&run.execution.unwrap());
    assert_eq!(
        facts["records"][1]["value"]["stop"]["kind"],
        "blocked-by-prior-phase"
    );
}
#[test]
fn mmio_fence_and_capacity_are_observable() {
    let f = Fixture::new(&[0x00b52023, 0x00052503, 0x0330000f, 0x00008067]);
    let mut r = f.request();
    {
        let i = &mut r.cases[0].vendor;
        i.arguments[0] = Some(0x3000);
        i.arguments[1] = Some(123);
        i.models.push(register_bank(vec![RegisterCell {
            address: 0x3000,
            width: 4,
            value: 0,
        }]));
    }
    r.cases[0].replacement = Some(r.cases[0].vendor.clone());
    let run = f.run(r.clone(), budget());
    let facts = f.read(&run.execution.unwrap());
    assert_eq!(facts["summary"]["manifest"]["verdict"], "MATCH");
    assert_eq!(facts["records"][0]["value"]["event"]["kind"], "write");
    assert_eq!(facts["records"][1]["value"]["event"]["value"], 123);
    assert_eq!(facts["records"][2]["value"]["event"]["kind"], "fence");
    r.max_events = 1;
    let run = f.run(r, budget());
    assert_eq!(run.error.unwrap().code, ErrorCode::ResourceLimited);
    assert!(run.execution.is_none());
}
#[test]
fn invalid_memory_and_unsupported_code_stay_incomplete() {
    for code in [
        &[0x00b52023, 0x00008067][..],
        &[0x00012503, 0x00008067][..],
        &[0x00000073][..],
    ] {
        let f = Fixture::new(code);
        let mut r = f.request();
        r.cases[0].vendor.arguments[0] = Some(0x1000);
        r.cases[0].replacement = Some(r.cases[0].vendor.clone());
        let run = f.run(r, budget());
        assert_eq!(run.state, RunState::Completed);
        assert_eq!(
            f.read(&run.execution.unwrap())["summary"]["manifest"]["verdict"],
            "INCOMPLETE"
        );
    }
}
#[test]
fn loops_memory_limits_and_cancellation_publish_nothing() {
    let f = Fixture::new(&[0x0000006f]);
    let r = f.request();
    let mut b = budget();
    b.max_work_units = Some(100000);
    let run = f.run(r.clone(), b);
    assert_eq!(run.error.unwrap().code, ErrorCode::ResourceLimited);
    assert!(run.execution.is_none());
    let mut b = budget();
    b.working_memory_bytes = Some(1024 * 1024);
    let run = f.run(r.clone(), b);
    assert_eq!(run.error.unwrap().code, ErrorCode::ResourceLimited);
    let mut b = budget();
    b.timeout_ms = 200;
    let run = f.run(r.clone(), b);
    assert_eq!(run.error.unwrap().code, ErrorCode::TimedOut);
    let handle = f
        .app
        .start_execution(
            &f.project,
            r,
            &blobray_backend_riscv::RiscvExecutor,
            budget(),
        )
        .unwrap();
    handle.cancel();
    let run = handle.wait();
    assert_eq!(run.state, RunState::Cancelled);
    assert!(run.execution.is_none());
}

#[test]
fn rv32_arithmetic_edges_and_machine_calls_execute_instructions() {
    for (instruction, a, b, expected) in [
        (0x02b54533, 123, 0, u32::MAX), // div by zero
        (0x02b54533, 0x80000000, u32::MAX, 0x80000000),
        (0x02b56533, 0x80000000, u32::MAX, 0), // signed remainder overflow
        (0x02b51533, u32::MAX, 2, u32::MAX),   // signed multiply high
        (0x40b55533, 0x80000000, 33, 0xc0000000), // masked shift amount
    ] {
        let f = Fixture::new(&[instruction, 0x00008067]);
        let mut r = f.request();
        r.replacement = None;
        r.binding = None;
        r.cases[0].replacement = None;
        r.cases[0].relation = None;
        r.cases[0].vendor.arguments[0] = Some(a);
        r.cases[0].vendor.arguments[1] = Some(b);
        let run = f.run(r, budget());
        let facts = f.read(&run.execution.unwrap());
        assert_eq!(facts["records"][0]["value"]["stop"]["low"], expected);
    }
    let f = Fixture::new(&[
        0x00008293, 0x00c000ef, 0x00028067, 0x00000013, 0x00750513, 0x00008067,
    ]);
    let run = f.run(f.request(), budget());
    let facts = f.read(&run.execution.unwrap());
    assert_eq!(facts["records"][0]["value"]["stop"]["low"], 7);
}
#[test]
fn signed_loads_and_phase_stack_reset_are_explicit() {
    let f = Fixture::new(&[0x00050503, 0x00008067]);
    let mut r = f.request();
    r.replacement = None;
    r.binding = None;
    r.cases[0].replacement = None;
    r.cases[0].relation = None;
    r.cases[0].vendor.arguments[0] = Some(0x3000);
    r.cases[0].vendor.memory.push(ram(MemorySeed {
        address: 0x3000,
        length: 1,
        fill: Some(0x80),
        bytes: vec![],
    }));
    let run = f.run(r, budget());
    assert_eq!(
        f.read(&run.execution.unwrap())["records"][0]["value"]["stop"]["low"],
        0xffffff80u32
    );
    // First phase writes the fresh stack, second phase reads the reset unknown byte.
    let f = Fixture::new(&[0x00050663, 0xfea12e23, 0x00008067, 0xffc12503, 0x00008067]);
    let mut r = f.request();
    r.replacement = None;
    r.binding = None;
    r.cases[0].replacement = None;
    r.cases[0].relation = None;
    r.cases[0].vendor.arguments[0] = Some(7);
    let mut second = r.cases[0].clone();
    second.reset = SessionReset::Warm;
    second.name = "read-stack".into();
    second.vendor.arguments[0] = Some(0);
    r.cases.push(second);
    let run = f.run(r, budget());
    let facts = f.read(&run.execution.unwrap());
    assert_eq!(facts["records"][0]["value"]["stop"]["kind"], "returned");
    assert_eq!(facts["records"][1]["value"]["stop"]["kind"], "incomplete");
}

#[test]
fn selected_companion_code_and_elf_zero_fill_obey_session_ownership() {
    let main = elf(&[0x000022b7, 0x00028067]);
    let mut companion = elf(&[0x00900513, 0x00008067]);
    for offset in [24, 60, 64] {
        companion[offset..offset + 4].copy_from_slice(&0x2000u32.to_le_bytes());
    }
    let f = Fixture::from_inputs(vec![main, companion]);
    let mut r = f.request();
    r.replacement = None;
    r.binding = None;
    r.cases[0].replacement = None;
    r.cases[0].relation = None;
    let run = f.run(r.clone(), budget());
    assert_eq!(
        run.assessment
            .as_ref()
            .and_then(|a| a.coverage.as_ref())
            .map(|c| c.status == CoverageStatus::Complete),
        Some(false)
    );
    r.vendor.companions = vec![1];
    let run = f.run(r.clone(), budget());
    assert_eq!(
        f.read(&run.execution.unwrap())["records"][0]["value"]["stop"]["low"],
        9
    );
    r.vendor.companions = vec![0];
    let run = f.run(r, budget());
    assert_eq!(run.error.unwrap().code, ErrorCode::Conflict);
    assert!(run.execution.is_none());
    let mut image = elf(&[0x00052283, 0x00128293, 0x00552023, 0x00028513, 0x00008067]);
    image[44..46].copy_from_slice(&2u16.to_le_bytes());
    for (offset, value) in [
        (84, 1u32),
        (92, 0x3000),
        (96, 0x3000),
        (104, 4),
        (108, 6),
        (112, 4),
    ] {
        image[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    let f = Fixture::from_inputs(vec![image]);
    let mut r = f.request();
    r.replacement = None;
    r.binding = None;
    r.cases[0].replacement = None;
    r.cases[0].relation = None;
    r.cases[0].vendor.arguments[0] = Some(0x3000);
    r.cases.push(r.cases[0].clone());
    for (mode, expected) in [
        (SessionReset::Warm, vec![1, 2]),
        (SessionReset::Cold, vec![1, 1]),
    ] {
        r.cases[1].reset = mode;
        let run = f.run(r.clone(), budget());
        let facts = f.read(&run.execution.unwrap());
        let values: Vec<_> = facts["records"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["value"]["stop"]["low"].as_u64().unwrap())
            .collect();
        assert_eq!(values, expected);
    }
}

#[path = "execution/arguments.rs"]
mod arguments;
#[path = "execution/atomics.rs"]
mod atomics;

fn ram(seed: MemorySeed) -> ExecutionRegion {
    ExecutionRegion {
        seed,
        lifetime: RegionLifetime::Session,
    }
}
#[path = "execution/goals.rs"]
mod goals;
#[path = "execution/sessions.rs"]
mod sessions;

fn register_bank(cells: Vec<RegisterCell>) -> DeviceDeclaration {
    DeviceDeclaration {
        id: "registers".into(),
        applicability: "fixture register-bank assumption".into(),
        lifetime: RegionLifetime::Phase,
        behavior: DeviceBehavior::RegisterBank { cells },
    }
}
#[path = "execution/devices.rs"]
mod devices;

#[path = "execution/calls.rs"]
mod calls;

#[path = "execution/interfaces.rs"]
mod interfaces;

#[path = "execution/services.rs"]
mod services;

fn fixture_relation(low: bool) -> ComparisonRelation {
    ComparisonRelation {
        effects: None,
        projection: None,
        calls: false,
        reviewed_calls: None,
        returns: ReturnWords { low, high: false },
        events: EventChannels {
            timeline: TimelineCapture::default(),
            mmio_read: true,
            mmio_write: true,
            fence: true,
            delay: true,
        },
        memory: vec![],
    }
}

#[path = "execution/comparison.rs"]
mod comparison;

#[path = "execution/capture.rs"]
mod capture;

#[path = "execution/call_pairs.rs"]
mod call_pairs;

#[path = "execution/timeline.rs"]
mod timeline;

#[path = "execution/effects.rs"]
mod effects;
#[path = "execution/projections.rs"]
mod projections;
#[path = "execution/static_trace.rs"]
mod static_trace;

#[path = "execution/command_bank.rs"]
mod command_bank;

#[path = "execution/unaligned.rs"]
mod unaligned;
