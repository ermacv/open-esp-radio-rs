use super::*;
use serde_json::json;

#[test]
fn current_scenario_catalog_drives_requirement_repetition_bounds() {
    let root = std::env::temp_dir().join(format!(
        "open-radio-qualification-scenario-catalog-{}",
        std::process::id()
    ));
    if root.exists() {
        fs::remove_dir_all(&root).unwrap();
    }
    let catalog_directory = root.join("scenarios");
    fs::create_dir_all(&catalog_directory).unwrap();
    fs::write(
        catalog_directory.join("ble-direct-test.toml"),
        r#"schema = 4
id = "ble-direct-test"
description = "Exercise the current HIL scenario document shape"
repetitions = 3
image = "boot-smoke"
isolation = "reset"

[workload]
kind = "boot-smoke"

[criteria]
"#,
    )
    .unwrap();

    let catalog = ScenarioCatalog::load(&root, Path::new("scenarios")).unwrap();
    catalog
        .validate_requirement(&HilRequirement {
            scenario: "ble-direct-test".to_owned(),
            minimum_repetitions: 3,
        })
        .unwrap();
    let error = catalog
        .validate_requirement(&HilRequirement {
            scenario: "ble-direct-test".to_owned(),
            minimum_repetitions: 4,
        })
        .unwrap_err();
    assert!(error.to_string().contains("declares 3"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn scenario_catalog_rejects_non_current_schema() {
    let root = std::env::temp_dir().join(format!(
        "open-radio-qualification-scenario-schema-{}",
        std::process::id()
    ));
    if root.exists() {
        fs::remove_dir_all(&root).unwrap();
    }
    let catalog_directory = root.join("scenarios");
    fs::create_dir_all(&catalog_directory).unwrap();
    fs::write(
        catalog_directory.join("future-scenario.toml"),
        "schema = 5\nid = \"future-scenario\"\nrepetitions = 1\n",
    )
    .unwrap();

    let error = ScenarioCatalog::load(&root, Path::new("scenarios")).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("invalid HIL scenario catalog entry")
    );

    fs::remove_dir_all(root).unwrap();
}

pub(super) fn seal(run: &Path) {
    let mut names = vec!["manifest.json", "suite.json"];
    for name in ["plan.json", "build-provenance.json"] {
        if run.join(name).is_file() {
            names.push(name);
        }
    }
    let files = names
        .into_iter()
        .map(|name| {
            let path = run.join(name);
            json!({
                "path": name,
                "size_bytes": fs::metadata(&path).unwrap().len(),
                "sha256": sha256_file(&path).unwrap(),
            })
        })
        .collect::<Vec<_>>();
    fs::write(
        run.join("integrity.json"),
        serde_json::to_vec_pretty(&json!({
            "schema": 2,
            "run_id": "run-1",
            "files": files,
        }))
        .unwrap(),
    )
    .unwrap();
}

#[test]
fn rejects_parent_paths_in_integrity_entries() {
    assert!(!safe_relative(Path::new("../suite.json")));
    assert!(!safe_relative(Path::new("/suite.json")));
    assert!(safe_relative(Path::new("scenarios/smoke/uart.log")));
}

#[test]
fn digest_requires_canonical_lowercase_hex() {
    assert!(valid_sha256(&"ab".repeat(32)));
    assert!(!valid_sha256(&"AB".repeat(32)));
    assert!(!valid_sha256("abc"));
}

#[test]
fn current_sealed_run_qualifies_and_tampering_fails_closed() {
    let root = std::env::temp_dir().join(format!(
        "open-radio-qualification-hil-{}",
        std::process::id()
    ));
    if root.exists() {
        fs::remove_dir_all(&root).unwrap();
    }
    let run = root.join("runs/run-1");
    fs::create_dir_all(&run).unwrap();
    let digest = "00".repeat(32);
    fs::write(
        run.join("manifest.json"),
        serde_json::to_vec_pretty(&json!({
            "schema": 2,
            "run_id": "run-1",
            "target": "esp32s31",
            "state": "completed",
            "started_unix_millis": 100,
            "finished_unix_millis": 200,
            "duration_millis": 100,
            "repository": {
                "commit": "abc123",
                "dirty": false,
                "workspace_sha256": digest,
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let mut suite = json!({
        "schema": 2,
        "run_id": "run-1",
        "target": "esp32s31",
        "outcome": "passed",
        "started_unix_millis": 100,
        "finished_unix_millis": 200,
        "duration_millis": 100,
        "counts": {
            "scenarios": 1,
            "passed": 1,
            "failed": 0,
            "broken": 0,
            "skipped": 0,
            "blocked": 0,
            "interrupted": 0,
        },
        "scenarios": [{
            "schema": 2,
            "scenario": "station-reconnect",
            "outcome": "passed",
            "required_repetitions": 2,
            "repetitions": [
                {"schema": 2, "repetition": 1, "outcome": "passed", "failure": null},
                {"schema": 2, "repetition": 2, "outcome": "passed", "failure": null}
            ],
            "failure": null,
        }]
    });
    fs::write(
        run.join("suite.json"),
        serde_json::to_vec_pretty(&suite).unwrap(),
    )
    .unwrap();
    add_current_build(&root, &run);
    seal(&run);

    let repository = RepositoryState {
        commit: "abc123".to_owned(),
        dirty: false,
    };
    let index = HilEvidenceIndex::load(&root, Path::new("runs"), "esp32s31", &repository).unwrap();
    assert!(
        index
            .evidence_for(&HilRequirement {
                scenario: "station-reconnect".to_owned(),
                minimum_repetitions: 2,
            })
            .is_some()
    );

    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(run.join("manifest.json")).unwrap()).unwrap();
    manifest["firmware"] = json!([{
        "replayed_from": {
            "source_run_id": "older-run"
        }
    }]);
    fs::write(
        run.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
    seal(&run);
    let replayed =
        HilEvidenceIndex::load(&root, Path::new("runs"), "esp32s31", &repository).unwrap();
    assert_eq!(replayed.summary().current_clean_producer, 0);
    assert_eq!(replayed.summary().qualifying, 0);

    manifest["firmware"] = json!([]);
    fs::write(
        run.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
    fs::write(
        run.join("plan.json"),
        serde_json::to_vec_pretty(&json!({
            "firmware": {
                "source": "replay",
                "source_run_id": "older-run"
            }
        }))
        .unwrap(),
    )
    .unwrap();
    seal(&run);
    let planned_replay =
        HilEvidenceIndex::load(&root, Path::new("runs"), "esp32s31", &repository).unwrap();
    assert_eq!(planned_replay.summary().current_clean_producer, 0);
    assert_eq!(planned_replay.summary().qualifying, 0);

    fs::remove_file(run.join("plan.json")).unwrap();
    seal(&run);

    fs::write(run.join("suite.json"), b"{}").unwrap();
    assert!(HilEvidenceIndex::load(&root, Path::new("runs"), "esp32s31", &repository,).is_err());

    suite["counts"]["passed"] = json!(0);
    fs::write(
        run.join("suite.json"),
        serde_json::to_vec_pretty(&suite).unwrap(),
    )
    .unwrap();
    seal(&run);
    assert!(HilEvidenceIndex::load(&root, Path::new("runs"), "esp32s31", &repository,).is_err());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn unsealed_running_run_is_mutable_state_not_evidence() {
    let root = std::env::temp_dir().join(format!(
        "open-radio-qualification-hil-running-{}",
        std::process::id()
    ));
    if root.exists() {
        fs::remove_dir_all(&root).unwrap();
    }
    let run = root.join("runs/run-1");
    fs::create_dir_all(&run).unwrap();
    fs::write(
        run.join("manifest.json"),
        serde_json::to_vec_pretty(&json!({
            "schema": 2,
            "run_id": "run-1",
            "target": "esp32s31",
            "state": "running",
            "started_unix_millis": 100,
            "finished_unix_millis": null,
            "duration_millis": null,
            "repository": {
                "commit": "abc123",
                "dirty": false,
                "workspace_sha256": "00".repeat(32),
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let repository = RepositoryState {
        commit: "abc123".to_owned(),
        dirty: false,
    };
    let index = HilEvidenceIndex::load(&root, Path::new("runs"), "esp32s31", &repository).unwrap();
    assert_eq!(index.summary().directories, 1);
    assert_eq!(index.summary().bundles, 1);
    assert_eq!(index.summary().incomplete, 0);
    assert_eq!(index.summary().completed, 0);
    assert_eq!(index.summary().qualifying, 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn manifestless_generated_run_is_incomplete_not_an_error() {
    let root = std::env::temp_dir().join(format!(
        "open-radio-qualification-hil-incomplete-{}",
        std::process::id()
    ));
    if root.exists() {
        fs::remove_dir_all(&root).unwrap();
    }
    let run = root.join("runs/run-1");
    fs::create_dir_all(&run).unwrap();
    fs::write(run.join("result.json"), b"{}\n").unwrap();

    let repository = RepositoryState {
        commit: "abc123".to_owned(),
        dirty: false,
    };
    let index = HilEvidenceIndex::load(&root, Path::new("runs"), "esp32s31", &repository).unwrap();
    assert_eq!(index.summary().directories, 1);
    assert_eq!(index.summary().bundles, 0);
    assert_eq!(index.summary().incomplete, 1);
    assert_eq!(index.summary().completed, 0);
    assert_eq!(index.summary().qualifying, 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn malformed_existing_manifest_still_fails_closed() {
    let root = std::env::temp_dir().join(format!(
        "open-radio-qualification-hil-malformed-{}",
        std::process::id()
    ));
    if root.exists() {
        fs::remove_dir_all(&root).unwrap();
    }
    let run = root.join("runs/run-1");
    fs::create_dir_all(&run).unwrap();
    fs::write(run.join("manifest.json"), b"not-json\n").unwrap();

    let repository = RepositoryState {
        commit: "abc123".to_owned(),
        dirty: false,
    };
    let error =
        HilEvidenceIndex::load(&root, Path::new("runs"), "esp32s31", &repository).unwrap_err();
    assert!(error.to_string().contains("cannot parse HIL evidence"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn unsealed_completed_run_still_fails_closed() {
    let root = std::env::temp_dir().join(format!(
        "open-radio-qualification-hil-unsealed-completed-{}",
        std::process::id()
    ));
    if root.exists() {
        fs::remove_dir_all(&root).unwrap();
    }
    let run = root.join("runs/run-1");
    fs::create_dir_all(&run).unwrap();
    fs::write(
        run.join("manifest.json"),
        serde_json::to_vec_pretty(&json!({
            "schema": 2,
            "run_id": "run-1",
            "target": "esp32s31",
            "state": "completed",
            "started_unix_millis": 100,
            "finished_unix_millis": 200,
            "duration_millis": 100,
            "repository": {
                "commit": "abc123",
                "dirty": false,
                "workspace_sha256": "00".repeat(32),
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let repository = RepositoryState {
        commit: "abc123".to_owned(),
        dirty: false,
    };
    let error =
        HilEvidenceIndex::load(&root, Path::new("runs"), "esp32s31", &repository).unwrap_err();
    assert!(error.to_string().contains("integrity.json"));
    fs::remove_dir_all(root).unwrap();
}

pub(super) fn add_current_build(root: &Path, run: &Path) {
    fs::write(root.join("Cargo.lock"), "version = 4\npackage = []\n").unwrap();
    let mut manifest: serde_json::Value = read_json(&run.join("manifest.json")).unwrap();
    manifest["firmware"] = json!([{
        "build_id": "ab".repeat(32),
        "build_provenance_path": "build-provenance.json",
    }]);
    fs::write(
        run.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    let provenance = json!({
        "schema": 1,
        "build_type": "open-esp-radio-hil-firmware/v1",
        "build_id": "ab".repeat(32),
        "source_reconstructable": true,
        "sources": [{
            "name": "repository",
            "commit": manifest["repository"]["commit"],
            "workspace_sha256": manifest["repository"]["workspace_sha256"],
            "dirty": false,
            "rebuild_status": "clean-commit",
            "limitations": [],
            "untracked_files": [],
            "tracked_patch_path": null,
        }],
        "files": [{
            "name": "workspace-lock",
            "path": "Cargo.lock",
            "sha256": sha256_file(&root.join("Cargo.lock")).unwrap(),
        }],
    });
    fs::write(
        run.join("build-provenance.json"),
        serde_json::to_vec(&provenance).unwrap(),
    )
    .unwrap();
}
