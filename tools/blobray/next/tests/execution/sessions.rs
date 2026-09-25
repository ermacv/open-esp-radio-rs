use super::*;

#[test]
fn explicit_guest_register_inputs_allow_spills_without_inventing_unknown_values() {
    // Entry prelude sets s0, then tail-enters a conventional save/restore leaf.
    // The alternate entry has the same body but no declared s0 input. A filled
    // stack cannot turn that unknown register into a known stored value.
    let f = Fixture::new(&[
        0x00000413, 0x0080006f, 0x00000013, 0xff010113, 0x00812023, 0x00700513, 0x00012403,
        0x01010113, 0x00008067,
    ]);
    let mut request = f.request();
    request.vendor.stack.fill = Some(0xa5);
    request.replacement.as_mut().unwrap().stack.fill = Some(0xa5);
    request.cases[0].name = "explicit-saved-register".into();
    let mut unknown = request.cases[0].clone();
    unknown.name = "undeclared-saved-register".into();
    unknown.vendor.entry = 0x100c;
    unknown.replacement.as_mut().unwrap().entry = 0x100c;
    request.cases.push(unknown);
    let run = f.run(request, budget());
    assert_eq!(run.state, RunState::Completed, "{run:?}");
    let facts = f.read(&run.execution.unwrap());
    let records = facts["records"].as_array().unwrap();
    for replacement in [false, true] {
        let stop = |case| {
            &records
                .iter()
                .find(|r| {
                    r["value"]["kind"] == "outcome"
                        && r["value"]["case"] == case
                        && r["value"]["replacement"] == replacement
                })
                .unwrap()["value"]["stop"]
        };
        assert_eq!(stop(0)["kind"], "returned");
        assert_eq!(stop(0)["low"], 7);
        assert_eq!(stop(1)["kind"], "incomplete");
        assert_eq!(stop(1)["reason"]["kind"], "unknown-register");
        assert_eq!(stop(1)["reason"]["register"], 8);
    }
    let verdicts: Vec<_> = records
        .iter()
        .filter(|r| r["value"]["kind"] == "comparison")
        .map(|r| r["value"]["result"]["verdict"].as_str().unwrap())
        .collect();
    assert_eq!(verdicts, ["MATCH", "INCOMPLETE"]);
}

fn phases(f: &Fixture, lifetime: RegionLifetime) -> ExecutionRequest {
    let mut request = f.request();
    let first = &mut request.cases[0];
    first.name = "setup".into();
    first.vendor.arguments = vec![Some(0x3000), Some(41)];
    first.vendor.memory = vec![ExecutionRegion {
        seed: MemorySeed {
            address: 0x3000,
            length: 8,
            fill: Some(0),
            bytes: vec![],
        },
        lifetime,
    }];
    first.replacement = Some(first.vendor.clone());
    let mut second = first.clone();
    second.name = "read".into();
    second.reset = SessionReset::Warm;
    second.vendor.entry = 0x1008;
    second.vendor.memory.clear();
    second.replacement = Some(second.vendor.clone());
    request.cases.push(second);
    request
}

#[test]
fn multi_entry_setup_warm_cold_and_blocking_have_one_replayable_publication() {
    // Separate setup and read entries over the same captured address space.
    let f = Fixture::new(&[0x00b52023, 0x00008067, 0x00052503, 0x00008067]);
    let mut request = phases(&f, RegionLifetime::Session);
    let mut cold_read = request.cases[1].clone();
    cold_read.name = "cold-has-no-prior-ram".into();
    cold_read.reset = SessionReset::Cold;
    request.cases.push(cold_read);
    let mut blocked = request.cases[0].clone();
    blocked.name = "blocked-setup-cannot-resume-chain".into();
    blocked.reset = SessionReset::Warm;
    request.cases.push(blocked);
    let mut restart = request.cases[0].clone();
    restart.name = "new-independent-chain".into();
    restart.vendor.arguments[1] = Some(99);
    restart.replacement = Some(restart.vendor.clone());
    request.cases.push(restart);
    let mut last = request.cases[1].clone();
    last.name = "new-chain-read".into();
    request.cases.push(last);
    let run = f.run(request.clone(), budget());
    assert_eq!(run.state, RunState::Completed, "{run:?}");
    let id = run.execution.unwrap();
    let result = f.read(&id);
    let manifest = &result["summary"]["manifest"];
    assert_eq!(manifest["complete"], false);
    assert_eq!(manifest["verdict"], "INCOMPLETE");
    // The manifest names the retained canonical request by identity.
    assert_eq!(
        manifest["request"],
        blobray_application::encode_execution_request(&request)
            .unwrap()
            .0
            .as_str()
    );
    let records = result["records"].as_array().unwrap();
    assert_eq!(records.len(), 18);
    assert_eq!(records[3]["value"]["stop"]["low"], 41);
    assert_eq!(records[6]["value"]["stop"]["reason"]["address"], 0x3000);
    assert_eq!(
        records[9]["value"]["stop"]["kind"],
        "blocked-by-prior-phase"
    );
    assert_eq!(records[9]["value"]["steps"], 0);
    assert_eq!(records[15]["value"]["stop"]["low"], 99);
    let backup = f._dir.path().join("phases.blobray");
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
fn phase_ram_expires_and_redeclaration_uses_new_seed_without_changing_session_owner() {
    let f = Fixture::new(&[0x00b52023, 0x00008067, 0x00052503, 0x00008067]);
    let mut request = phases(&f, RegionLifetime::Phase);
    let run = f.run(request.clone(), budget());
    let result = f.read(&run.execution.unwrap());
    assert_eq!(result["records"][3]["value"]["stop"]["kind"], "incomplete");
    let mut region = request.cases[0].vendor.memory[0].clone();
    region.seed.bytes = 77u32.to_le_bytes().to_vec();
    request.cases[1].vendor.memory.push(region);
    request.cases[1].replacement = Some(request.cases[1].vendor.clone());
    let run = f.run(request, budget());
    let result = f.read(&run.execution.unwrap());
    assert_eq!(result["records"][3]["value"]["stop"]["low"], 77);
    assert_eq!(result["summary"]["manifest"]["verdict"], "MATCH");
    let prior = phases(&f, RegionLifetime::Session);
    let saved = f.run(prior.clone(), budget()).execution.unwrap();
    let mut bad = prior;
    let mut region = bad.cases[0].vendor.memory[0].clone();
    region.lifetime = RegionLifetime::Phase;
    bad.cases[1].vendor.memory.push(region);
    bad.cases[1].replacement = Some(bad.cases[1].vendor.clone());
    let failed = f.run(bad, budget());
    assert_eq!(failed.error.unwrap().code, ErrorCode::Conflict);
    assert!(failed.execution.is_none());
    assert_eq!(f.read(&saved)["summary"]["manifest"]["verdict"], "MATCH");
}

#[test]
fn phase_entry_reset_validation_and_shared_budget_fail_without_publication() {
    let f = Fixture::new(&[0x00b52023, 0x00008067, 0x00052503, 0x00008067]);
    for which in 0..3 {
        let mut request = phases(&f, RegionLifetime::Session);
        match which {
            0 => request.cases[0].reset = SessionReset::Warm,
            1 => request.cases[1].vendor.entry = 0x1009,
            _ => request.cases[1].replacement.as_mut().unwrap().entry = u32::MAX - 1,
        }
        let err = match f.app.start_execution(
            &f.project,
            request,
            &blobray_backend_riscv::RiscvExecutor,
            budget(),
        ) {
            Ok(_) => panic!("invalid phase was admitted"),
            Err(err) => err,
        };
        assert_eq!(err.code, ErrorCode::InvalidRequest);
    }
    let mut request = phases(&f, RegionLifetime::Session);
    // A later phase loops forever; completed setup must not publish partial evidence.
    let f = Fixture::new(&[0x00b52023, 0x00008067, 0x0000006f]);
    request.vendor = f.target.clone();
    request.replacement = Some(f.target.clone());
    let mut b = budget();
    b.max_work_units = Some(100000);
    let run = f.run(request, b);
    assert_eq!(run.error.unwrap().code, ErrorCode::ResourceLimited);
    assert!(run.execution.is_none());
}

#[test]
fn cold_reset_releases_both_sides_before_allocating_the_next_address_spaces() {
    let f = Fixture::new(&[0x00008067]);
    let mut request = f.request();
    request.cases[0].vendor.memory = vec![ram(MemorySeed {
        address: 0x10000000,
        length: 4,
        fill: None,
        bytes: vec![],
    })];
    request.cases[0].replacement = Some(request.cases[0].vendor.clone());
    request.cases[0].replacement.as_mut().unwrap().memory[0]
        .seed
        .length = 10 * 1024 * 1024;
    let mut second = request.cases[0].clone();
    second.name = "swap-large-side-on-cold-reset".into();
    second.vendor.memory[0].seed.length = 10 * 1024 * 1024;
    second.replacement.as_mut().unwrap().memory[0].seed.length = 4;
    request.cases.push(second);
    // Each phase fits the 32 MiB budget. Retaining the old replacement while
    // preparing the new vendor would overlap two 20 MiB byte/knownness pairs.
    let run = f.run(request, budget());
    assert_eq!(run.state, RunState::Completed, "{run:?}");
    assert_eq!(
        f.read(&run.execution.unwrap())["summary"]["manifest"]["verdict"],
        "MATCH"
    );
}
