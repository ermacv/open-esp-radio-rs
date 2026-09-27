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
