use super::*;
pub(super) fn symbol_elf(code: &[u32], base: u32, goal: u32) -> (Vec<u8>, ExecutionSymbol) {
    let mut bytes = elf(code);
    for offset in [24, 60, 64] {
        bytes[offset..offset + 4].copy_from_slice(&base.to_le_bytes());
    }
    let strings = b"\0entry\0goal\0alias\0data\0";
    let string_offset = bytes.len() as u32;
    bytes.extend_from_slice(strings);
    while !bytes.len().is_multiple_of(4) {
        bytes.push(0);
    }
    let symbols = bytes.len() as u32;
    bytes.extend_from_slice(&[0; 16]);
    for (name, address, info) in [
        (1u32, base, 0x12u8),
        (7, goal, 0x10),
        (12, goal, 0x12),
        (18, goal, 0x11),
    ] {
        bytes.extend_from_slice(&name.to_le_bytes());
        bytes.extend_from_slice(&address.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes()); // zero-sized exact boundary
        bytes.extend_from_slice(&[info, 0, 1, 0]);
    }
    let sections = bytes.len() as u32;
    for fields in [
        [0; 10],
        [0, 1, 6, base, 256, code.len() as u32 * 4, 0, 0, 2, 0],
        [0, 3, 0, 0, string_offset, strings.len() as u32, 0, 0, 1, 0],
        [0, 2, 0, 0, symbols, 80, 2, 1, 4, 16],
    ] {
        for word in fields {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
    }
    bytes[32..36].copy_from_slice(&sections.to_le_bytes());
    bytes[46..48].copy_from_slice(&40u16.to_le_bytes());
    bytes[48..50].copy_from_slice(&4u16.to_le_bytes());
    bytes[50..52].copy_from_slice(&2u16.to_le_bytes());
    let point = ExecutionSymbol {
        source: FunctionSource::Input { input: 0 },
        symbol: SymbolId {
            object: ObjectId {
                artifact: ArtifactId::of_bytes(&bytes),
                location: ObjectLocation::Standalone,
            },
            table: SymbolTableKind::Static,
            table_section: 3,
            index: 2,
        },
    };
    (bytes, point)
}
pub(super) fn fixture(code: &[u32], goal: u32) -> (Fixture, ExecutionSymbol) {
    let (bytes, point) = symbol_elf(code, 0x1000, goal);
    (Fixture::from_inputs(vec![bytes]), point)
}
fn request(f: &Fixture, goal: ExecutionGoal) -> ExecutionRequest {
    let mut request = f.request();
    request.compare_return = matches!(goal, ExecutionGoal::Return);
    request.cases[0].vendor.goal = goal;
    request.cases[0].replacement = Some(request.cases[0].vendor.clone());
    request
}
#[test]
fn exact_goal_boundaries_stop_before_callee_body_and_replay_after_restore() {
    // Save return link; call a NOTYPE, zero-sized physical boundary whose body is unsupported.
    let (f, point) = fixture(
        &[0x00008413, 0x00c000ef, 0x00040067, 0x00000013, 0x00000073],
        0x1010,
    );
    let mut last = None;
    for (goal, kind, pc) in [
        (
            ExecutionGoal::ReachSymbol {
                target: point.clone(),
            },
            "reached-symbol",
            0x1010,
        ),
        (
            ExecutionGoal::ObserveCall {
                target: point.clone(),
                include_tail: false,
            },
            "observed-call",
            0x1004,
        ),
    ] {
        let r = request(&f, goal);
        let run = f.run(r, budget());
        assert_eq!(run.state, RunState::Completed, "{run:?}");
        let id = run.execution.unwrap();
        let result = f.read(&id);
        assert_eq!(result["summary"]["manifest"]["complete"], true);
        assert_eq!(result["summary"]["manifest"]["verdict"], "MATCH");
        assert_eq!(result["records"][0]["value"]["stop"]["kind"], kind);
        assert_eq!(result["records"][0]["value"]["stop"]["pc"], pc);
        assert_eq!(result["records"][0]["value"]["steps"], 2);
        last = Some(id);
    }
    let run = f.run(f.request(), budget());
    assert_eq!(
        f.read(&run.execution.unwrap())["summary"]["manifest"]["verdict"],
        "INCOMPLETE"
    );
    let id = last.unwrap();
    let backup = f._dir.path().join("goals.blobray");
    let restored = f._dir.path().join("restored");
    for (command, project, flag, path) in [
        ("backup", &f.project, "--output", &backup),
        ("restore", &restored, "--backup", &backup),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_blobray"))
            .args([command, "--project"])
            .arg(project)
            .arg(flag)
            .arg(path)
            .args(["--limit-mode", "watchdog"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let replay = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args(["--format", "json", "replay", "--project"])
        .arg(&restored)
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
fn known_indirect_calls_tail_policy_and_returns_are_distinct() {
    for (code, tail) in [
        (
            vec![0x000012b7, 0x01028293, 0x000280e7, 0x00008067, 0x00000073],
            false,
        ),
        (
            vec![0x010002ef, 0x00008067, 0x00000013, 0x00000013, 0x00000073],
            false,
        ),
        (
            vec![0x0100006f, 0x00008067, 0x00000013, 0x00000013, 0x00000073],
            true,
        ),
    ] {
        let (f, point) = fixture(&code, 0x1010);
        let run = f.run(
            request(
                &f,
                ExecutionGoal::ObserveCall {
                    target: point.clone(),
                    include_tail: true,
                },
            ),
            budget(),
        );
        let result = f.read(&run.execution.unwrap());
        assert_eq!(
            result["records"][0]["value"]["stop"]["kind"],
            "observed-call"
        );
        assert_eq!(result["records"][0]["value"]["stop"]["tail"], tail);
        if tail {
            let run = f.run(
                request(
                    &f,
                    ExecutionGoal::ObserveCall {
                        target: point,
                        include_tail: false,
                    },
                ),
                budget(),
            );
            assert_eq!(
                f.read(&run.execution.unwrap())["summary"]["manifest"]["verdict"],
                "INCOMPLETE"
            );
        }
    }
    // A canonical return to the boundary is not an observed call, even with tails enabled.
    let (f, point) = fixture(
        &[0x000010b7, 0x01008093, 0x00008067, 0x00000013, 0x00000073],
        0x1010,
    );
    let run = f.run(
        request(
            &f,
            ExecutionGoal::ObserveCall {
                target: point,
                include_tail: true,
            },
        ),
        budget(),
    );
    assert_eq!(
        f.read(&run.execution.unwrap())["records"][0]["value"]["stop"]["kind"],
        "incomplete"
    );
    let (f, point) = fixture(&[0x000280e7, 0x00000073], 0x1004);
    let run = f.run(
        request(
            &f,
            ExecutionGoal::ObserveCall {
                target: point,
                include_tail: true,
            },
        ),
        budget(),
    );
    assert_eq!(
        f.read(&run.execution.unwrap())["records"][0]["value"]["stop"]["reason"]["kind"],
        "unknown-register"
    );
}
#[test]
fn premature_return_blocks_warm_phase_and_equal_prefixes_do_not_match() {
    let (f, point) = fixture(&[0x00008067, 0x00000073], 0x1004);
    let mut r = request(&f, ExecutionGoal::ReachSymbol { target: point });
    let mut warm = r.cases[0].clone();
    warm.reset = SessionReset::Warm;
    warm.name = "dependent".into();
    r.cases.push(warm);
    let run = f.run(r, budget());
    let result = f.read(&run.execution.unwrap());
    assert_eq!(result["summary"]["manifest"]["verdict"], "INCOMPLETE");
    assert_eq!(
        result["records"][0]["value"]["stop"]["kind"],
        "goal-not-reached"
    );
    assert_eq!(
        result["records"][3]["value"]["stop"]["kind"],
        "blocked-by-prior-phase"
    );
}
#[test]
fn invalid_physical_goals_and_relations_publish_nothing() {
    let (f, point) = fixture(&[0x00008067, 0x00008067], 0x1004);
    for which in 0..5 {
        let mut point = point.clone();
        match which {
            0 => point.symbol.index = 900,
            1 => point.symbol.table_section = 2,
            2 => point.symbol.table = SymbolTableKind::Dynamic,
            3 => point.symbol.index = 4, // data symbol in executable section
            _ => point.symbol.object.artifact = ArtifactId::of_bytes(b"different"),
        }
        let run = f.run(
            request(&f, ExecutionGoal::ReachSymbol { target: point }),
            budget(),
        );
        assert!(run.error.is_some(), "{run:?}");
        assert!(run.execution.is_none());
    }
    for which in 0..3 {
        let mut r = request(
            &f,
            ExecutionGoal::ReachSymbol {
                target: point.clone(),
            },
        );
        match which {
            0 => r.compare_return = true,
            1 => r.cases[0].replacement.as_mut().unwrap().goal = ExecutionGoal::Return,
            _ => {
                if let ExecutionGoal::ReachSymbol { target } = &mut r.cases[0].vendor.goal {
                    target.source = FunctionSource::Input { input: 1 };
                }
            }
        }
        match f.app.start_execution(
            &f.project,
            r,
            &blobray_backend_riscv::RiscvExecutor,
            budget(),
        ) {
            Ok(_) => panic!("invalid goal relation admitted"),
            Err(error) => assert_eq!(error.code, ErrorCode::InvalidRequest),
        }
    }
}
#[test]
fn comparison_of_observed_call_prefixes_keeps_known_differences() {
    let (f, point) = fixture(
        &[0x00b52023, 0x00c000ef, 0x00008067, 0x00000013, 0x00000073],
        0x1010,
    );
    let mut r = request(
        &f,
        ExecutionGoal::ObserveCall {
            target: point,
            include_tail: false,
        },
    );
    r.cases[0].vendor.arguments = vec![Some(0x3000), Some(7)];
    r.cases[0]
        .vendor
        .models
        .push(register_bank(vec![RegisterCell {
            address: 0x3000,
            width: 4,
            value: 0,
        }]));
    r.cases[0].replacement = Some(r.cases[0].vendor.clone());
    let saved = f.run(r.clone(), budget()).execution.unwrap();
    assert_eq!(f.read(&saved)["summary"]["manifest"]["verdict"], "MATCH");
    r.cases[0].replacement.as_mut().unwrap().arguments[1] = Some(8);
    let run = f.run(r, budget());
    assert_eq!(
        f.read(&run.execution.unwrap())["summary"]["manifest"]["verdict"],
        "DIFF"
    );
}

#[test]
fn multiple_physical_goals_prepare_each_object_once_and_share_failure_budgets() {
    let (f, point) = fixture(
        &[0x00008413, 0x00c000ef, 0x00040067, 0x00000013, 0x00000073],
        0x1010,
    );
    let mut r = request(
        &f,
        ExecutionGoal::ReachSymbol {
            target: point.clone(),
        },
    );
    let mut second = r.cases[0].clone();
    second.reset = SessionReset::Warm;
    second.name = "different-physical-alias".into();
    let mut alias = point;
    alias.symbol.index = 3;
    second.vendor.goal = ExecutionGoal::ObserveCall {
        target: alias,
        include_tail: false,
    };
    second.replacement = Some(second.vendor.clone());
    r.cases.push(second);
    let mut third = r.cases[0].clone();
    third.reset = SessionReset::Warm;
    third.name = "already-at-boundary".into();
    third.vendor.entry = 0x1010;
    third.replacement = Some(third.vendor.clone());
    r.cases.push(third);
    let run = f.run(r, budget());
    assert_eq!(run.state, RunState::Completed, "{run:?}");
    assert_eq!(
        run.diagnostics
            .as_ref()
            .unwrap()
            .progress
            .as_ref()
            .unwrap()
            .measurements
            .objects_prepared,
        1
    );
    let result = f.read(&run.execution.unwrap());
    assert_eq!(result["summary"]["manifest"]["verdict"], "MATCH");
    assert_eq!(result["records"][6]["value"]["steps"], 0);
    let (f, point) = fixture(&[0x0000006f, 0x00000073], 0x1004);
    let r = request(&f, ExecutionGoal::ReachSymbol { target: point });
    let mut b = budget();
    b.max_work_units = Some(10000);
    let run = f.run(r.clone(), b);
    assert_eq!(run.error.unwrap().code, ErrorCode::ResourceLimited);
    assert!(run.execution.is_none());
    let mut b = budget();
    b.working_memory_bytes = Some(1024 * 1024);
    let run = f.run(r.clone(), b);
    assert_eq!(run.error.unwrap().code, ErrorCode::ResourceLimited);
    assert!(run.execution.is_none());
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
fn symbol_goals_require_and_use_the_explicit_companion_mapping() {
    let (companion, mut point) = symbol_elf(&[0x00000073], 0x2000, 0x2000);
    point.source = FunctionSource::Input { input: 1 };
    let f = Fixture::from_inputs(vec![elf(&[0x000022b7, 0x00028067]), companion]);
    let mut r = request(&f, ExecutionGoal::ReachSymbol { target: point });
    assert_eq!(r.validate().unwrap_err().code, ErrorCode::InvalidRequest);
    r.vendor.companions.push(1);
    r.replacement.as_mut().unwrap().companions.push(1);
    let run = f.run(r, budget());
    assert_eq!(run.state, RunState::Completed, "{run:?}");
    let result = f.read(&run.execution.unwrap());
    assert_eq!(result["summary"]["manifest"]["verdict"], "MATCH");
    assert_eq!(result["records"][0]["value"]["stop"]["pc"], 0x2000);
}
