use super::*;
fn flow(f: &Fixture, query: &FlowQuery) -> serde_json::Value {
    let path = f.dir.path().join("flow-query.json");
    fs::write(&path, serde_json::to_vec(query).unwrap()).unwrap();
    cli(f, &["flow", "--request", path.to_str().unwrap()])
}
#[test]
fn selected_flow_paths_review_exact_hops_and_reopen_without_sources() {
    let f = fixture(true, true);
    let image = prepared(&f);
    let publication = analyze(&f, Some(image));
    let scope = NavigationScope {
        revision: f.revision.clone(),
        publications: vec![publication],
        analyses: vec![],
        knowledge: None,
    };
    let selection = navigation::navigate(
        &f,
        &NavigationQuery {
            scope: scope.clone(),
            filter: NavigationFilter::Functions { function: None },
        },
    );
    let functions = selection["records"].as_array().unwrap();
    let root: NavigationFunction = serde_json::from_value(
        functions
            .iter()
            .find(|r| r["value"]["extent"]["start"] == 0x10000000u32)
            .unwrap()["value"]["function"]
            .clone(),
    )
    .unwrap();
    let target: NavigationFunction = serde_json::from_value(
        functions
            .iter()
            .find(|r| r["value"]["function"]["analysis"] != root.analysis.as_str())
            .unwrap()["value"]["function"]
            .clone(),
    )
    .unwrap();
    let mut request = FlowQuery {
        scope,
        root: root.analysis.clone(),
        goal: FlowGoal::Function {
            analysis: target.analysis.clone(),
        },
        max_depth: 8,
    };
    let result = flow(&f, &request);
    assert_eq!(
        result["summary"]["summary"]["target_reached"], true,
        "{result}"
    );
    assert_eq!(result["summary"]["summary"]["reached_analyses"], 2);
    let hop: FlowHop = serde_json::from_value(
        result["records"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["value"]["kind"] == "function" && !r["value"]["parent"].is_null())
            .unwrap()["value"]["parent"]
            .clone(),
    )
    .unwrap();
    assert_eq!(hop.caller, root.analysis);
    assert_eq!(hop.callee, target.analysis);
    request.max_depth = 0;
    let limited = flow(&f, &request);
    assert_eq!(limited["summary"]["summary"]["target_reached"], false);
    assert!(
        limited["records"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["value"]["reason"] == "depth")
    );
    request.max_depth = 8;
    let proposal = KnowledgeProposal {
        subject: "fixture.path".to_owned().try_into().unwrap(),
        occurrence: KnowledgeOccurrence {
            revision: f.revision.clone(),
            source: root.location.source.clone(),
            object: root.location.selector.object().clone(),
            symbol: root.location.selector.symbol().cloned(),
        },
        claim: KnowledgeClaim::Path {
            path: Box::new(ReviewedPath {
                hops: vec![hop.clone()],
                purpose: "fixture structural path".into(),
                applicability: "captured inputs only".into(),
            }),
        },
        evidence: vec![EvidenceRef::Analysis {
            analysis: root.analysis.clone(),
            record: Some(hop.record),
        }],
        note: None,
    };
    let change = |proposal| KnowledgeChange {
        expected_base: None,
        actor: "fixture".into(),
        reason: "verify exact saved hop".into(),
        action: KnowledgeAction::Propose { proposal },
    };
    for fault in 0..3 {
        let mut bad = proposal.clone();
        let KnowledgeClaim::Path { path } = &mut bad.claim else {
            panic!()
        };
        match fault {
            0 => path.hops[0].record = u64::MAX,
            1 => path.hops[0].callee = root.analysis.clone(),
            _ => bad.occurrence.source = FunctionSource::Input { input: 0 },
        }
        match f.app.start_knowledge(&f.project, &change(bad), budget()) {
            Ok(handle) => {
                let failed = handle.wait();
                assert_eq!(failed.state, RunState::Failed, "{failed:?}");
                assert!(failed.knowledge.is_none());
            }
            Err(error) => assert_eq!(error.code, ErrorCode::InvalidRequest),
        }
        assert!(
            cli(&f, &["knowledge", "show"])["records"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
    // A second selected interpretation at the same entry makes the first hop ambiguous.
    let target_extent: CodeRange = serde_json::from_value(
        functions
            .iter()
            .find(|r| r["value"]["function"]["analysis"] == target.analysis.as_str())
            .unwrap()["value"]["extent"]
            .clone(),
    )
    .unwrap();
    let alternate = f
        .app
        .start_analyze_function(
            &f.project,
            FunctionRequest {
                revision: Some(f.revision.clone()),
                source: target.location.source.clone(),
                selector: target.location.selector.clone(),
                research: None,
                extent: Some(target_extent),
            },
            budget(),
        )
        .unwrap()
        .wait();
    assert_eq!(alternate.state, RunState::Completed, "{alternate:?}");
    let alternate = alternate.analysis.unwrap();
    assert_ne!(alternate, target.analysis);
    let mut ambiguous = proposal.clone();
    let KnowledgeClaim::Path { path } = &mut ambiguous.claim else {
        panic!()
    };
    path.hops.push(FlowHop {
        caller: target.analysis.clone(),
        record: 0,
        callee: alternate.clone(),
    });
    let refused = f
        .app
        .start_knowledge(&f.project, &change(ambiguous), budget())
        .unwrap()
        .wait();
    assert_eq!(refused.state, RunState::Failed, "{refused:?}");
    assert!(refused.error.unwrap().message.contains("ambiguous"));
    let mut ambiguous_query = request.clone();
    ambiguous_query.scope.analyses.push(alternate);
    let ambiguous_result = flow(&f, &ambiguous_query);
    assert_eq!(
        ambiguous_result["summary"]["summary"]["target_reached"],
        false
    );
    assert!(
        ambiguous_result["records"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["value"]["reason"] == "ambiguous-call")
    );
    let proposed = f
        .app
        .start_knowledge(&f.project, &change(proposal.clone()), budget())
        .unwrap()
        .wait();
    assert_eq!(proposed.state, RunState::Completed, "{proposed:?}");
    let entries = cli(&f, &["knowledge", "show"]);
    let assertion = entries["records"][0]["value"]["id"].as_str().unwrap();
    for path in fs::read_dir(f.dir.path()).unwrap() {
        let path = path.unwrap().path();
        if path.is_file() && matches!(path.extension().and_then(|s| s.to_str()), Some("o" | "a")) {
            fs::remove_file(path).unwrap();
        }
    }
    let accepted = cli(
        &f,
        &[
            "knowledge",
            "accept",
            "--base",
            proposed.knowledge.unwrap().as_str(),
            "--assertion",
            assertion,
            "--actor",
            "fixture",
            "--reason",
            "conditional structural path",
        ],
    );
    let revision = accepted["run"]["knowledge"].as_str().unwrap();
    assert_eq!(
        cli(&f, &["knowledge", "show", "--revision", revision])["records"][0]["value"]["proposal"],
        serde_json::to_value(proposal).unwrap()
    );
    let exported = f.dir.path().join("reviewed-path.json");
    cli(
        &f,
        &[
            "knowledge",
            "export",
            "--revision",
            revision,
            "--output",
            exported.to_str().unwrap(),
        ],
    );
    assert!(fs::metadata(exported).unwrap().len() > 0);
    assert_eq!(flow(&f, &request), result);
    let output = f.dir.path().join("flow-export.json");
    cli(
        &f,
        &[
            "flow",
            "--request",
            f.dir.path().join("flow-query.json").to_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
        ],
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&fs::read(output).unwrap()).unwrap(),
        result
    );
    let mut low = budget();
    low.max_work_units = Some(1);
    assert!(
        f.app
            .query(
                &f.project,
                app::ReadQuery::Flow {
                    request: request.clone()
                },
                low
            )
            .is_err()
    );
    request.goal = FlowGoal::Effects {
        profile: FlowEffectProfile::All,
        address: None,
    };
    let effects = flow(&f, &request);
    assert_eq!(effects["summary"]["summary"]["facts_passes"], 4);
    assert!(
        effects["records"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["value"]["kind"] == "call")
    );
}
