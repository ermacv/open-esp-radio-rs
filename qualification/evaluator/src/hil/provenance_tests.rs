use super::*;
use serde_json::{Value, json};

#[test]
fn qualification_checks_every_firmware_source_against_current_pins() {
    let root =
        std::env::temp_dir().join(format!("oer-qualification-sources-{}", std::process::id()));
    if root.exists() {
        fs::remove_dir_all(&root).unwrap();
    }
    let run = root.join("runs/run-1");
    fs::create_dir_all(&run).unwrap();
    let repository = RepositoryState {
        commit: "abc123".into(),
        dirty: false,
    };
    let manifest = json!({
        "schema": 2, "run_id": "run-1", "target": "esp32s31", "state": "completed",
        "started_unix_millis": 100, "finished_unix_millis": 200, "duration_millis": 100,
        "repository": {"commit": repository.commit, "dirty": false, "workspace_sha256": "00".repeat(32)},
    });
    write(&run.join("manifest.json"), &manifest);
    write(
        &run.join("suite.json"),
        &json!({
            "schema": 2, "run_id": "run-1", "target": "esp32s31", "outcome": "passed",
            "started_unix_millis": 100, "finished_unix_millis": 200, "duration_millis": 100,
            "counts": {"scenarios": 1, "passed": 1, "failed": 0, "broken": 0, "skipped": 0, "blocked": 0, "interrupted": 0},
            "scenarios": [{"schema": 2, "scenario": "station-reconnect", "outcome": "passed",
                "required_repetitions": 1, "failure": null,
                "repetitions": [{"schema": 2, "repetition": 1, "outcome": "passed", "failure": null}]}],
        }),
    );
    super::tests::add_current_build(&root, &run);
    let check = |expected| {
        super::tests::seal(&run);
        let index =
            HilEvidenceIndex::load(&root, Path::new("runs"), "esp32s31", &repository).unwrap();
        assert_eq!(index.summary().qualifying, usize::from(expected));
        assert_eq!(
            index.summary().current_clean_producer,
            usize::from(expected)
        );
        assert_eq!(
            index
                .evidence_for(&HilRequirement {
                    scenario: "station-reconnect".into(),
                    minimum_repetitions: 1
                })
                .is_some(),
            expected
        );
    };
    check(true);
    let path = run.join("build-provenance.json");
    let canonical: Value = read_json(&path).unwrap();
    let pin = "12".repeat(20);
    let lock = ["esp-hal", "esp-sync", "esp-bootloader-esp-idf", "embassy-net", "embassy-net-driver", "xarxa-driver"]
        .map(|name| format!("[[package]]\nname = {name:?}\nversion = \"1.0.0\"\nsource = \"git+https://example.invalid/source?rev={pin}#{pin}\"\n"))
        .join("\n");
    fs::write(root.join("Cargo.lock"), lock).unwrap();
    let mut pinned = canonical.clone();
    pinned["files"][0]["sha256"] = json!(sha256_file(&root.join("Cargo.lock")).unwrap());
    for name in ["esp-hal", "embassy", "xarxa"] {
        let mut source = pinned["sources"][0].clone();
        source["name"] = json!(name);
        source["commit"] = json!(pin);
        pinned["sources"].as_array_mut().unwrap().push(source);
    }
    write(&path, &pinned);
    check(true);
    for (field, value) in [
        ("dirty", json!(true)),
        ("commit", json!("34".repeat(20))),
        ("rebuild_status", json!("tracked-patch")),
        ("limitations", json!(["untracked-content-not-archived"])),
        ("untracked_files", json!([{"path": "untracked.rs"}])),
        ("tracked_patch_path", json!("source.patch")),
        ("name", json!("unknown-override")),
        ("name", json!("repository")),
    ] {
        for index in 1..=3 {
            let mut invalid = pinned.clone();
            invalid["sources"][index][field] = value.clone();
            write(&path, &invalid);
            check(false);
        }
    }
    for (field, value) in [
        ("source_reconstructable", json!(false)),
        ("sources", json!([])),
        ("files", json!([])),
    ] {
        let mut invalid = pinned.clone();
        invalid[field] = value;
        write(&path, &invalid);
        check(false);
    }
    write(&path, &canonical); // Recorded lock differs from the current lock.
    check(false);
    write(&path, &pinned);
    let good_manifest: Value = read_json(&run.join("manifest.json")).unwrap();
    let mut missing = good_manifest.clone();
    missing["firmware"][0]
        .as_object_mut()
        .unwrap()
        .remove("build_provenance_path");
    write(&run.join("manifest.json"), &missing);
    check(false);
    missing["firmware"] = json!([]);
    write(&run.join("manifest.json"), &missing);
    check(false);
    let mut multiple = good_manifest.clone();
    multiple["firmware"].as_array_mut().unwrap().push(json!({}));
    write(&run.join("manifest.json"), &multiple);
    check(false);
    let mut escaping = good_manifest;
    escaping["firmware"][0]["build_provenance_path"] = json!("../outside.json");
    write(&run.join("manifest.json"), &escaping);
    super::tests::seal(&run);
    let error =
        HilEvidenceIndex::load(&root, Path::new("runs"), "esp32s31", &repository).unwrap_err();
    assert!(error.to_string().contains("contained"));
    fs::remove_dir_all(root).unwrap();
}

fn write(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
}
