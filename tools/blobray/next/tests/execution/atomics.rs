use super::*;

fn op(function: u32, ordering: u32, dest: u32, base: u32, src: u32) -> u32 {
    function << 27 | ordering << 25 | src << 20 | base << 15 | 2 << 12 | dest << 7 | 0x2f
}
fn scenario(f: &Fixture, value: Option<u32>) -> ExecutionRequest {
    let mut request = f.request();
    request.cases[0].vendor.arguments = vec![Some(0x3000), Some(5)];
    request.cases[0].vendor.memory.push(ram(MemorySeed {
        address: 0x3000,
        length: 8,
        fill: None,
        bytes: value.map(|n| n.to_le_bytes().to_vec()).unwrap_or_default(),
    }));
    request.cases[0].replacement = Some(request.cases[0].vendor.clone());
    request
}

#[test]
fn every_word_amo_and_ordering_returns_old_value_and_updates_memory() {
    for (function, old, operand, expected) in [
        (1, 9, 5, 5u32),     // swap
        (0, u32::MAX, 5, 4), // add wraps
        (4, 0xaa, 0x0f, 0xa5),
        (12, 0xaa, 0x0f, 0x0a),
        (8, 0xaa, 0x0f, 0xaf),
        (16, 0x80000000, 5, 0x80000000),
        (20, 0x80000000, 5, 5),
        (24, 0x80000000, 5, 5),
        (28, 0x80000000, 5, 0x80000000),
    ] {
        for ordering in 0..4 {
            // amo t0,a1,(a0); lw a1,0(a0); mv a0,t0; ret
            let f = Fixture::new(&[
                op(function, ordering, 5, 10, 11),
                0x00052583,
                0x00028513,
                0x00008067,
            ]);
            let mut request = scenario(&f, Some(old));
            request.cases[0].vendor.arguments[1] = Some(operand);
            request.cases[0].replacement = Some(request.cases[0].vendor.clone());
            let run = f.run(request, budget());
            assert_eq!(run.state, RunState::Completed, "{run:?}");
            let result = f.read(&run.execution.unwrap());
            assert_eq!(result["summary"]["manifest"]["verdict"], "MATCH");
            assert_eq!(result["records"][0]["value"]["stop"]["low"], old);
            assert_eq!(result["records"][0]["value"]["stop"]["high"], expected);
            assert_eq!(
                result["records"].as_array().unwrap().len(),
                3,
                "RAM atomics must not emit MMIO events"
            );
        }
    }
}

#[test]
fn lr_sc_status_and_phase_reset_are_replayable() {
    // a1 nonzero: reserve/return. a1 zero: SC returns its status, then read memory.
    let f = Fixture::new(&[
        0x00058663,
        op(2, 2, 5, 10, 0),
        0x00008067,
        op(3, 1, 5, 10, 11),
        0x00052583,
        0x00028513,
        0x00008067,
    ]);
    let mut request = scenario(&f, Some(9));
    let mut second = request.cases[0].clone();
    second.reset = SessionReset::Warm;
    second.name = "no-reservation-crosses-phase".into();
    second.vendor.arguments[1] = Some(0);
    second.vendor.memory.clear();
    second.replacement = Some(second.vendor.clone());
    request.cases.push(second);
    let run = f.run(request, budget());
    let id = run.execution.unwrap();
    let result = f.read(&id);
    assert_eq!(result["records"][3]["value"]["stop"]["low"], 1);
    assert_eq!(result["records"][3]["value"]["stop"]["high"], 9);
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
    let replay: serde_json::Value = serde_json::from_slice(&replay.stdout).unwrap();
    assert_eq!(replay["run"]["execution"], id.as_str());

    // Successful SC writes once and clears its reservation; the next SC fails.
    for twice in [false, true] {
        let mut code = vec![op(2, 3, 5, 10, 0), op(3, 3, 5, 10, 11)];
        if twice {
            code.push(op(3, 0, 5, 10, 11));
        }
        code.extend([0x00052583, 0x00028513, 0x00008067]);
        let f = Fixture::new(&code);
        let run = f.run(scenario(&f, Some(9)), budget());
        let result = f.read(&run.execution.unwrap());
        assert_eq!(
            result["records"][0]["value"]["stop"]["low"],
            u32::from(twice)
        );
        assert_eq!(result["records"][0]["value"]["stop"]["high"], 5);
    }
}

#[test]
fn atomic_unknowns_permissions_alignment_and_mmio_do_not_fallback() {
    for instruction in [op(2, 0, 5, 10, 0), op(3, 0, 5, 10, 11), op(0, 0, 5, 10, 11)] {
        let f = Fixture::new(&[instruction, 0x00008067]);
        for address in [0x3001, 0x5000, 0x6000] {
            let mut request = scenario(&f, Some(9));
            request.cases[0].vendor.arguments[0] = Some(address);
            request.cases[0]
                .vendor
                .models
                .push(register_bank(vec![RegisterCell {
                    address: 0x6000,
                    width: 4,
                    value: 9,
                }]));
            request.cases[0].replacement = Some(request.cases[0].vendor.clone());
            let run = f.run(request, budget());
            let result = f.read(&run.execution.unwrap());
            assert_eq!(result["summary"]["manifest"]["verdict"], "INCOMPLETE");
            assert_eq!(
                result["records"][1]["value"]["stop"]["reason"]["access"],
                "atomic"
            );
            assert_eq!(
                result["records"][1]["value"]["stop"]["reason"]["address"],
                address
            );
        }
    }
    for instruction in [op(2, 0, 5, 10, 0), op(0, 0, 5, 10, 11)] {
        let f = Fixture::new(&[instruction, 0x00008067]);
        let run = f.run(scenario(&f, None), budget());
        assert_eq!(
            f.read(&run.execution.unwrap())["summary"]["manifest"]["verdict"],
            "INCOMPLETE"
        );
    }
    for instruction in [op(3, 0, 5, 10, 11), op(0, 0, 5, 10, 11)] {
        let f = Fixture::new(&[instruction, 0x00008067]);
        let mut request = scenario(&f, Some(9));
        request.cases[0].vendor.arguments[0] = Some(0x1000); // read-only executable mapping
        request.cases[0].replacement = Some(request.cases[0].vendor.clone());
        let run = f.run(request, budget());
        assert_eq!(
            f.read(&run.execution.unwrap())["summary"]["manifest"]["verdict"],
            "INCOMPLETE"
        );
    }
}

#[test]
fn atomic_effect_comparison_reopens_and_replays_after_backup_restore() {
    // AMOADD discards old value; read the changed word into the selected return.
    let f = Fixture::new(&[op(0, 3, 0, 10, 11), 0x00052503, 0x00008067]);
    let mut request = scenario(&f, Some(9));
    request.cases[0].replacement.as_mut().unwrap().arguments[1] = Some(6);
    let run = f.run(request, budget());
    let id = run.execution.unwrap();
    let result = f.read(&id);
    assert_eq!(result["summary"]["manifest"]["verdict"], "DIFF");
    assert_eq!(result["records"][0]["value"]["stop"]["low"], 14);
    assert_eq!(result["records"][1]["value"]["stop"]["low"], 15);
    let backup = f._dir.path().join("atomic.blobray");
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
