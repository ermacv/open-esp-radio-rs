use super::*;

#[test]
fn register_and_stack_words_execute_compare_and_replay_through_both_frontends() {
    // lw t0,0(sp); lw t1,4(sp); add a0,a7,t0; add a0,a0,t1;
    // andi a1,sp,15; ret. Expected result derives from explicit input words.
    let f = Fixture::new(&[
        0x00012283, 0x00412303, 0x00588533, 0x00650533, 0x00f17593, 0x00008067,
    ]);
    let mut request = f.request();
    request.cases[0].vendor.arguments = vec![None; 7];
    request.cases[0]
        .vendor
        .arguments
        .extend([Some(9), Some(10), Some(11)]);
    request.cases[0].replacement = Some(request.cases[0].vendor.clone());
    let run = f.run(request.clone(), budget());
    assert_eq!(run.state, RunState::Completed, "{run:?}");
    let result = f.read(run.execution.as_ref().unwrap());
    assert_eq!(result["summary"]["manifest"]["verdict"], "MATCH");
    for record in result["records"].as_array().unwrap().iter().take(2) {
        assert_eq!(record["value"]["stop"]["low"], 30);
        assert_eq!(record["value"]["stop"]["high"], 0);
    }
    let path = f._dir.path().join("request.json");
    fs::write(&path, serde_json::to_vec(&request).unwrap()).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args(["--format", "json", "compare", "--project"])
        .arg(&f.project)
        .arg("--request")
        .arg(&path)
        .args(["--limit-mode", "watchdog"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let cli: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(cli["run"]["execution"], run.execution.unwrap().as_str());
    request.cases[0].replacement.as_mut().unwrap().arguments[9] = Some(12);
    let run = f.run(request, budget());
    assert_eq!(
        f.read(&run.execution.unwrap())["summary"]["manifest"]["verdict"],
        "DIFF"
    );
}

#[test]
fn unknown_words_override_seeded_stack_and_omitted_registers_stay_unknown() {
    for (code, words, expected) in [
        (vec![0x00012503, 0x00008067], vec![None; 9], "memory"),
        (
            vec![0x00158513, 0x00008067],
            vec![Some(5)],
            "unknown-register",
        ),
    ] {
        let f = Fixture::new(&code);
        let mut request = f.request();
        request.vendor.stack.fill = Some(0xff);
        request.vendor.stack.bytes = vec![0x42; 4096];
        request.replacement = Some(request.vendor.clone());
        request.cases[0].vendor.arguments = words;
        request.cases[0].replacement = Some(request.cases[0].vendor.clone());
        let run = f.run(request, budget());
        let result = f.read(&run.execution.unwrap());
        assert_eq!(result["summary"]["manifest"]["verdict"], "INCOMPLETE");
        let stop = &result["records"][0]["value"]["stop"];
        assert_eq!(stop["reason"]["kind"], expected);
        if expected == "memory" {
            assert_eq!(stop["reason"]["address"], 0x8ff0);
        } else {
            assert_eq!(stop["reason"]["register"], 11);
        }
    }
    // A supplied unknown word is harmless when code overwrites it before use.
    let f = Fixture::new(&[0x00500513, 0x00008067]);
    let mut request = f.request();
    request.cases[0].vendor.arguments.clear();
    request.cases[0].replacement = Some(request.cases[0].vendor.clone());
    let run = f.run(request, budget());
    assert_eq!(
        f.read(&run.execution.unwrap())["summary"]["manifest"]["verdict"],
        "MATCH"
    );
}

#[test]
fn stack_alignment_capacity_and_unknown_padding_are_explicit() {
    let f = Fixture::new(&[0x00010513, 0x00008067]); // mv a0,sp; ret
    for (count, expected) in [
        (0, 0x9000),
        (8, 0x9000),
        (9, 0x8ff0),
        (12, 0x8ff0),
        (13, 0x8fe0),
        (256, 0x8c20),
    ] {
        let mut request = f.request();
        request.cases[0].vendor.arguments = vec![None; count];
        request.cases[0].replacement = Some(request.cases[0].vendor.clone());
        let run = f.run(request, budget());
        let result = f.read(&run.execution.unwrap());
        assert_eq!(result["records"][0]["value"]["stop"]["low"], expected);
        assert_eq!(result["summary"]["manifest"]["verdict"], "MATCH");
    }
    let f = Fixture::new(&[0x00412503, 0x00008067]); // Load unused padding after argument 9.
    let mut request = f.request();
    request.cases[0].vendor.arguments.push(Some(10));
    request.cases[0].replacement = Some(request.cases[0].vendor.clone());
    let run = f.run(request, budget());
    let result = f.read(&run.execution.unwrap());
    assert_eq!(result["summary"]["manifest"]["verdict"], "INCOMPLETE");
    assert_eq!(
        result["records"][0]["value"]["stop"]["reason"]["address"],
        0x8ff4
    );
}

#[test]
fn invalid_argument_geometry_is_rejected_before_publication() {
    let f = Fixture::new(&[0x00008067]);
    let prior = f.run(f.request(), budget()).execution.unwrap();
    for variant in 0..5 {
        let mut request = f.request();
        match variant {
            0 => request.cases[0].vendor.arguments.resize(257, None),
            1 => {
                request.cases[0]
                    .replacement
                    .as_mut()
                    .unwrap()
                    .arguments
                    .resize(13, None);
                let stack = &mut request.replacement.as_mut().unwrap().stack;
                stack.address = 0x8ff0;
                stack.length = 16;
            }
            2 => request.vendor.stack.address += 1,
            3 => request.vendor.stack.length = u32::MAX,
            _ => request.schema = 1,
        }
        let error = match f.app.start_execution(
            &f.project,
            request,
            &blobray_backend_riscv::RiscvExecutor,
            budget(),
        ) {
            Ok(_) => panic!("invalid argument geometry admitted"),
            Err(error) => error,
        };
        assert_eq!(error.code, ErrorCode::InvalidRequest);
    }
    assert_eq!(f.read(&prior)["summary"]["manifest"]["verdict"], "MATCH");
}

#[test]
fn compressed_andi_matches_independent_signed_masks_in_concrete_execution() {
    // One entry for each immediate: C.ANDI a0,imm; C.JR ra. Ordinary ANDI
    // entries follow in the same captured image. Expected values come from
    // signed integer masks, not from either decoder or the comparison verdict.
    let mut code = Vec::new();
    for signed in -32i32..32 {
        let bits = signed as u32 & 63;
        code.push(0x8082_0000 | 0x8901 | ((bits & 31) << 2) | ((bits & 32) << 7));
    }
    for signed in -32i32..32 {
        code.extend([((signed as u32 & 0xfff) << 20) | 0x57513, 0x00008067]);
    }
    let f = Fixture::new(&code);
    for input in [0x8013, 0xa5a5_5a5a, 0xffff_ffff] {
        let mut request = f.request();
        let template = request.cases[0].clone();
        request.cases.clear();
        for (index, signed) in (-32i32..32).enumerate() {
            let mut case = template.clone();
            case.name = format!("andi-{signed}");
            case.vendor.entry = 0x1000 + index as u32 * 4;
            case.vendor.arguments[0] = Some(input);
            let mut replacement = case.vendor.clone();
            replacement.entry = 0x1100 + index as u32 * 8;
            case.replacement = Some(replacement);
            request.cases.push(case);
        }
        let run = f.run(request, budget());
        assert_eq!(run.state, RunState::Completed, "{run:?}");
        let result = f.read(&run.execution.unwrap());
        assert_eq!(result["summary"]["manifest"]["verdict"], "MATCH");
        let outcomes: Vec<_> = result["records"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| &row["value"])
            .filter(|row| row["kind"] == "outcome")
            .collect();
        assert_eq!(outcomes.len(), 128);
        for row in outcomes {
            let signed = row["case"].as_i64().unwrap() as i32 - 32;
            assert_eq!(row["stop"]["kind"], "returned");
            assert_eq!(row["stop"]["low"], input & signed as u32);
        }
    }
}

#[test]
fn case_stack_fill_replaces_the_target_fill_for_both_sides() {
    let f = Fixture::new(&[0xffc12503, 0x00008067]); // lw a0,-4(sp); ret
    let mut request = f.request();
    request.vendor.stack.fill = None;
    request.vendor.stack.bytes.clear();
    request.replacement = Some(request.vendor.clone());
    request.cases[0].vendor.arguments.clear();
    request.cases[0].replacement = Some(request.cases[0].vendor.clone());
    let unfilled = request.cases[0].clone();
    request.cases = [0x5a, 0xa5]
        .map(|fill| ExecutionCase {
            stack_fill: Some(fill),
            ..unfilled.clone()
        })
        .to_vec();
    request.cases.push(unfilled);
    let run = f.run(request, budget());
    let result = f.read(&run.execution.unwrap());
    let returned: Vec<_> = result["records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["value"]["kind"] == "outcome")
        .map(|r| (r["value"]["case"].clone(), r["value"]["stop"].clone()))
        .collect();
    for side in 0..2 {
        assert_eq!(returned[side].1["low"], 0x5a5a_5a5a_u32);
        assert_eq!(returned[2 + side].1["low"], 0xa5a5_a5a5_u32);
        // Without a case fill, the target's unknown stack stays unknown.
        assert_eq!(returned[4 + side].1["kind"], "incomplete");
    }
    assert_eq!(result["summary"]["manifest"]["verdict"], "INCOMPLETE");
}
