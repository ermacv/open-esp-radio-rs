use super::*;

#[test]
fn unreadable_and_changed_shards_are_stale() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    std::fs::create_dir_all(root.join("sources")).unwrap();
    std::fs::write(root.join("sources/lib.rs"), "one").unwrap();
    let directory = Path::new("shards");
    std::fs::create_dir_all(root.join(directory)).unwrap();
    let shard = |scenario: &str| scenario_evidence::Index {
        schema: scenario_evidence::SCHEMA,
        command: scenario_evidence::COMMAND.into(),
        target: "esp32s31".into(),
        scenario: scenario.into(),
        inputs: Default::default(),
        sources: vec![scenario_evidence::SourceDigest {
            sha256: scenario_evidence::digest_directory(root, Path::new("sources")).unwrap(),
            path: PathBuf::from("sources"),
        }],
        dependence: scenario_evidence::Dependence::whole_closure("test"),
        entries: vec![],
        untriaged: vec![],
        functions: vec![],
        unobserved: vec![],
        observed: vec![],
        unprojected: vec![],
    };
    let write = |name: &str, text: String| {
        std::fs::write(root.join(directory).join(format!("{name}.json")), text).unwrap();
    };
    write("current", serde_json::to_string(&shard("current")).unwrap());
    write("renamed", serde_json::to_string(&shard("other")).unwrap());
    write("conflicted", "<<<<<<< HEAD\n{}\n".into());
    assert_eq!(stale(root, directory).unwrap(), ["conflicted", "renamed"]);
    std::fs::write(root.join("sources/lib.rs"), "two").unwrap();
    assert_eq!(
        stale(root, directory).unwrap(),
        ["conflicted", "current", "renamed"]
    );
}

#[test]
fn a_shard_tracking_executed_files_stays_current_when_another_file_changes() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    std::fs::create_dir_all(root.join("crates/hal/src")).unwrap();
    std::fs::write(root.join("crates/hal/src/executed.rs"), "one").unwrap();
    std::fs::write(root.join("crates/hal/src/other.rs"), "one").unwrap();
    let directory = Path::new("shards");
    std::fs::create_dir_all(root.join(directory)).unwrap();
    let executed = PathBuf::from("crates/hal/src/executed.rs");
    let shard = scenario_evidence::Index {
        schema: scenario_evidence::SCHEMA,
        command: scenario_evidence::COMMAND.into(),
        target: "esp32s31".into(),
        scenario: "leaf".into(),
        inputs: Default::default(),
        sources: vec![scenario_evidence::SourceDigest {
            sha256: scenario_evidence::digest_source(root, &executed).unwrap(),
            path: executed.clone(),
        }],
        dependence: scenario_evidence::Dependence::whole_closure("test"),
        entries: vec![],
        untriaged: vec![],
        functions: vec![],
        unobserved: vec![],
        observed: vec![],
        unprojected: vec![],
    };
    std::fs::write(
        root.join(directory).join("leaf.json"),
        serde_json::to_string(&shard).unwrap(),
    )
    .unwrap();
    std::fs::write(root.join("crates/hal/src/other.rs"), "two").unwrap();
    assert!(stale(root, directory).unwrap().is_empty());
    std::fs::write(root.join(&executed), "two").unwrap();
    assert_eq!(stale(root, directory).unwrap(), ["leaf"]);
}

#[test]
fn a_shard_of_the_previous_schema_is_rejected() {
    let text = r#"{"schema":8,"command":"vendor-scenario","target":"esp32s31","scenario":"a",
        "inputs":{},"sources":[],"entries":[],"untriaged":[],"functions":[],"unobserved":[],
        "observed":[],"unprojected":[]}"#;
    assert!(serde_json::from_str::<scenario_evidence::Index>(text).is_err());
}
