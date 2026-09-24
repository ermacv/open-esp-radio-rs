use super::*;
fn navigate(f: &Fixture, request: &NavigationQuery) -> serde_json::Value {
    let path = f.dir.path().join("navigation-request.json");
    fs::write(&path, serde_json::to_vec(request).unwrap()).unwrap();
    cli(f, &["navigate", "--request", path.to_str().unwrap()])
}
#[test]
fn selected_navigation_keeps_call_evidence_deduplicates_scope_and_never_starts_analysis() {
    let f = fixture(true, true);
    let image = prepared(&f);
    let publication = analyze(&f, Some(image));
    let mut request = NavigationQuery {
        scope: NavigationScope {
            revision: f.revision.clone(),
            publications: vec![publication.clone(), publication],
            analyses: vec![],
            knowledge: None,
        },
        filter: NavigationFilter::Functions { function: None },
    };
    let original = navigate(&f, &request);
    let functions: Vec<_> = original["records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["value"]["kind"] == "function")
        .collect();
    assert_eq!(functions.len(), 2, "{original}");
    assert_eq!(original["summary"]["summary"]["selected_analyses"], 2);
    assert_eq!(original["summary"]["summary"]["analyses_read"], 0);
    let entry = functions
        .iter()
        .find(|r| r["value"]["extent"]["start"] == 0x10000000u32)
        .unwrap();
    let entry: NavigationFunction =
        serde_json::from_value(entry["value"]["function"].clone()).unwrap();
    let helper: NavigationFunction = serde_json::from_value(
        functions
            .iter()
            .find(|r| r["value"]["function"]["analysis"] != entry.analysis.as_str())
            .unwrap()["value"]["function"]
            .clone(),
    )
    .unwrap();
    request.scope.analyses.push(entry.analysis.clone());
    request.filter = NavigationFilter::Calls {
        function: Some(entry.location.clone()),
        direction: CallDirection::Callees,
    };
    let outgoing = navigate(&f, &request);
    assert_eq!(outgoing["summary"]["summary"]["analyses_read"], 2);
    let calls: Vec<_> = outgoing["records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["value"]["kind"] == "call")
        .collect();
    assert_eq!(calls.len(), 1, "{outgoing}");
    assert_eq!(
        calls[0]["value"]["candidates"][0]["analysis"],
        helper.analysis.as_str()
    );
    assert_eq!(calls[0]["value"]["focus_match"], true);
    assert!(calls[0]["value"]["issue"].is_null());
    request.filter = NavigationFilter::Calls {
        function: Some(helper.location),
        direction: CallDirection::Callers,
    };
    let incoming = navigate(&f, &request);
    assert_eq!(
        incoming["records"][0]["value"]["caller"]["analysis"],
        entry.analysis.as_str()
    );
    assert_eq!(
        incoming["records"][0]["value"]["record"],
        calls[0]["value"]["record"]
    );
    let database = f.project.join(".blobray-next/project.sqlite3");
    let before = fs::read(&database).unwrap();
    for path in fs::read_dir(f.dir.path()).unwrap() {
        let path = path.unwrap().path();
        if path.is_file() && matches!(path.extension().and_then(|s| s.to_str()), Some("o" | "a")) {
            fs::remove_file(path).unwrap();
        }
    }
    assert_eq!(navigate(&f, &request), incoming);
    let output = f.dir.path().join("navigation-export.json");
    cli(
        &f,
        &[
            "navigate",
            "--request",
            f.dir
                .path()
                .join("navigation-request.json")
                .to_str()
                .unwrap(),
            "--output",
            output.to_str().unwrap(),
        ],
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&fs::read(output).unwrap()).unwrap(),
        incoming
    );
    for limited in [true, false] {
        let mut limits = budget();
        if limited {
            limits.max_work_units = Some(1);
        } else {
            limits.working_memory_bytes = Some(4 * 1024 * 1024);
        }
        assert!(
            f.app
                .query(
                    &f.project,
                    app::ReadQuery::Navigate {
                        request: request.clone()
                    },
                    limits
                )
                .is_err()
        );
    }
    let mut wrong = request.clone();
    wrong.scope.revision = ArtifactId::of_bytes(b"another revision")
        .as_str()
        .parse()
        .unwrap();
    assert!(
        f.app
            .query(
                &f.project,
                app::ReadQuery::Navigate { request: wrong },
                budget()
            )
            .is_err()
    );
    assert_eq!(fs::read(database).unwrap(), before);
}
