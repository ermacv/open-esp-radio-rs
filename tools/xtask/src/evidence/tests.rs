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

#[test]
fn a_shard_records_a_changed_file_or_one_below_a_recorded_directory() {
    let shard = scenario_evidence::Index {
        schema: scenario_evidence::SCHEMA,
        command: scenario_evidence::COMMAND.into(),
        target: "esp32s31".into(),
        scenario: "i2c".into(),
        inputs: Default::default(),
        sources: ["crates/phy/src/lib.rs", "verification/schema"]
            .map(|path| scenario_evidence::SourceDigest {
                path: PathBuf::from(path),
                sha256: String::new(),
            })
            .to_vec(),
        dependence: scenario_evidence::Dependence::whole_closure("test"),
        entries: vec![],
        untriaged: vec![],
        functions: vec![],
        unobserved: vec![],
        observed: vec![],
        unprojected: vec![],
    };
    let records =
        |paths: &[&str]| records_any(&shard, &paths.iter().map(PathBuf::from).collect::<Vec<_>>());
    assert!(records(&["crates/phy/src/lib.rs"]));
    assert!(records(&[
        "hil/README.md",
        "verification/schema/scenario-evidence.rs"
    ]));
    assert!(!records(&["crates/phy/src/lib.rs.orig", "hil/README.md"]));
    assert!(!records(&[]));
}

#[test]
fn a_location_another_scenario_covers_leaves_the_chip_wide_untriaged_set() {
    let at = |function: &str, offset: u32| scenario_evidence::Location {
        function: function.into(),
        offset,
        kind: scenario_evidence::LocationKind::Block,
    };
    let shard = |untriaged: Vec<scenario_evidence::Location>, functions: &[&str]| {
        scenario_evidence::Index {
            schema: scenario_evidence::SCHEMA,
            command: scenario_evidence::COMMAND.into(),
            target: "esp32s31".into(),
            scenario: "test".into(),
            inputs: Default::default(),
            sources: vec![],
            dependence: scenario_evidence::Dependence::whole_closure("test"),
            entries: vec![],
            untriaged,
            functions: functions.iter().map(|f| f.to_string()).collect(),
            unobserved: vec![],
            observed: vec![],
            unprojected: vec![],
        }
    };
    let shards = [
        // Leaves `shared+0` and `shared+4` and `own+0` untriaged.
        shard(
            vec![at("shared", 0), at("shared", 4), at("own", 0)],
            &["shared", "own"],
        ),
        // Covers `shared+0`, leaves `shared+4` untriaged too.
        shard(vec![at("shared", 4)], &["shared"]),
        // Contains neither function.
        shard(vec![at("other", 0)], &["other"]),
    ];
    assert_eq!(
        untriaged_everywhere(&shards),
        BTreeSet::from([at("other", 0), at("own", 0), at("shared", 4)])
    );
}
