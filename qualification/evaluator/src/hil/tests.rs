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
        r#"schema = 5
id = "ble-direct-test"
description = "Exercise the current HIL scenario document shape"
role = "investigation"
repetitions = 3

[system]
kind = "boot-smoke"
"#,
    )
    .unwrap();

    let catalog = ScenarioCatalog::load(&root, Path::new("scenarios")).unwrap();
    catalog
        .validate_requirement(&HilRequirement {
            scenario: "ble-direct-test".to_owned(),
            checks: vec![],
            minimum_repetitions: 3,
        })
        .unwrap();
    let error = catalog
        .validate_requirement(&HilRequirement {
            scenario: "ble-direct-test".to_owned(),
            checks: vec![],
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
        "schema = 6\nid = \"future-scenario\"\nrole = \"investigation\"\nrepetitions = 1\n[system]\nkind = \"boot-smoke\"\n",
    )
    .unwrap();

    let error = ScenarioCatalog::load(&root, Path::new("scenarios")).unwrap_err();
    assert!(
        error.to_string().contains("future-scenario.toml"),
        "{error}"
    );

    fs::remove_dir_all(root).unwrap();
}

/// Fill the run bundle documents of `run` that a test wrote with only the
/// fields it cares about: every field the bundle's typed format requires and
/// the test left out gets a neutral value, and the documents take the
/// format's schema. A firmware entry without an application gets an empty
/// one, archived in the run.
pub(super) fn complete(run: &Path) {
    fn fill(value: &mut serde_json::Value, defaults: serde_json::Value) {
        if let (Some(object), serde_json::Value::Object(defaults)) =
            (value.as_object_mut(), defaults)
        {
            for (key, default) in defaults {
                object.entry(key).or_insert(default);
            }
        }
    }
    let schema = json!(oer_hil_run_bundle_format::run::RUN_SCHEMA);
    let empty = oer_durable::sha256_bytes(b"");
    let manifest_path = run.join("manifest.json");
    if let Ok(bytes) = fs::read(&manifest_path)
        && let Ok(mut manifest) = serde_json::from_slice::<serde_json::Value>(&bytes)
        && manifest.is_object()
    {
        manifest["schema"] = schema.clone();
        fill(
            &mut manifest,
            json!({
                "finished_unix_millis": null, "duration_millis": null, "invocation": [],
                "runner": {}, "cell": {"cell_id": "cell", "device_id": "dut",
                "serial_device": "/dev/ttyACM0"}, "firmware": [], "messages_used": [],
            }),
        );
        fill(
            &mut manifest["runner"],
            json!({"package": "oer-hil-runner", "version": "1", "messages_lock_sha256": "00",
                   "host_os": "linux", "host_arch": "x86_64", "tools": []}),
        );
        for artifact in manifest["firmware"].as_array_mut().unwrap() {
            let image = artifact["image"]
                .as_str()
                .unwrap_or("boot-smoke")
                .to_owned();
            if artifact.get("application_path").is_none() {
                let path = PathBuf::from("firmware")
                    .join(&image)
                    .join("application.bin");
                fs::create_dir_all(run.join(path.parent().unwrap())).unwrap();
                fs::write(run.join(&path), b"").unwrap();
                fill(
                    artifact,
                    json!({"application_path": path, "application_size_bytes": 0,
                           "application_sha256": empty}),
                );
            }
            fill(
                artifact,
                json!({"image": image, "runtime_elf_sha256": empty}),
            );
            if let Some(origin) = artifact.get_mut("replayed_from") {
                fill(
                    origin,
                    json!({"source_integrity_sha256": empty, "firmware_repository":
                           {"commit": "older", "dirty": false, "workspace_sha256": empty}}),
                );
            }
        }
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
    }
    let suite_path = run.join("suite.json");
    if let Ok(bytes) = fs::read(&suite_path)
        && let Ok(mut suite) = serde_json::from_slice::<serde_json::Value>(&bytes)
        && suite.get("scenarios").is_some()
    {
        suite["schema"] = schema.clone();
        for scenario in suite["scenarios"].as_array_mut().unwrap() {
            scenario["schema"] = schema.clone();
            fill(scenario, json!({"image": "boot-smoke", "failure": null}));
            let id = scenario["scenario"].as_str().unwrap_or_default().to_owned();
            for repetition in scenario["repetitions"].as_array_mut().unwrap() {
                repetition["schema"] = schema.clone();
                let number = repetition["repetition"].as_u64().unwrap_or_default();
                fill(
                    repetition,
                    json!({"started_unix_millis": 0, "duration_millis": 0,
                           "artifact_directory": format!("scenarios/{id}/repetition-{number:03}"),
                           "attachments": [], "measurements": [], "failure": null}),
                );
            }
        }
        fs::write(&suite_path, serde_json::to_vec_pretty(&suite).unwrap()).unwrap();
    }
    let plan_path = run.join("plan.json");
    if let Ok(bytes) = fs::read(&plan_path)
        && let Ok(mut plan) = serde_json::from_slice::<serde_json::Value>(&bytes)
    {
        let id = run.file_name().unwrap().to_string_lossy().into_owned();
        plan["schema"] = schema.clone();
        fill(
            &mut plan,
            json!({"run_id": id, "selection": "test", "entries": []}),
        );
        if plan["firmware"]["source"] == "replay" {
            fill(
                &mut plan["firmware"],
                json!({"image": "boot-smoke", "application_sha256": empty}),
            );
        }
        fs::write(&plan_path, serde_json::to_vec_pretty(&plan).unwrap()).unwrap();
    }
}

/// [`complete`] the run's documents, then seal it as its writer does.
pub(super) fn seal(run: &Path) {
    complete(run);
    let manifest: serde_json::Value = read_json(&run.join("manifest.json")).unwrap();
    oer_hil_run_bundle_format::run::write_integrity_index(
        run,
        manifest["run_id"].as_str().unwrap(),
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
            "target": "chip-a",
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
        "target": "chip-a",
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
    let index = HilEvidenceIndex::load(
        &root,
        Path::new("runs"),
        Path::new("evidence"),
        "chip-a",
        &repository,
    )
    .unwrap();
    assert!(
        index
            .evidence_for(
                &HilRequirement {
                    scenario: "station-reconnect".to_owned(),
                    checks: vec![],
                    minimum_repetitions: 2,
                },
                &ScenarioCatalog::default()
            )
            .is_some()
    );

    let stale_repository = RepositoryState {
        commit: "different-commit".to_owned(),
        dirty: false,
    };
    let stale = HilEvidenceIndex::load(
        &root,
        Path::new("runs"),
        Path::new("evidence"),
        "chip-a",
        &stale_repository,
    )
    .unwrap();
    assert_eq!(stale.summary().qualifying, 0);
    // Every chip's runs share the store: another chip's run is neither
    // evidence nor an invalid bundle of this program.
    let other_chip = HilEvidenceIndex::load(
        &root,
        Path::new("runs"),
        Path::new("evidence"),
        "other-target",
        &repository,
    )
    .unwrap();
    assert_eq!(other_chip.summary().qualifying, 0);
    assert!(other_chip.summary().invalid.is_empty());

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
    let replayed = HilEvidenceIndex::load(
        &root,
        Path::new("runs"),
        Path::new("evidence"),
        "chip-a",
        &repository,
    )
    .unwrap();
    assert_eq!(replayed.summary().current_source_producer, 0);
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
    let planned_replay = HilEvidenceIndex::load(
        &root,
        Path::new("runs"),
        Path::new("evidence"),
        "chip-a",
        &repository,
    )
    .unwrap();
    assert_eq!(planned_replay.summary().current_source_producer, 0);
    assert_eq!(planned_replay.summary().qualifying, 0);

    fs::remove_file(run.join("plan.json")).unwrap();
    seal(&run);

    fs::write(run.join("suite.json"), b"{}").unwrap();
    assert!(
        HilEvidenceIndex::rejection(HilEvidenceIndex::load(
            &root,
            Path::new("runs"),
            Path::new("evidence"),
            "chip-a",
            &repository,
        ))
        .is_some()
    );

    suite["counts"]["passed"] = json!(0);
    fs::write(
        run.join("suite.json"),
        serde_json::to_vec_pretty(&suite).unwrap(),
    )
    .unwrap();
    seal(&run);
    assert!(
        HilEvidenceIndex::rejection(HilEvidenceIndex::load(
            &root,
            Path::new("runs"),
            Path::new("evidence"),
            "chip-a",
            &repository,
        ))
        .is_some()
    );
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
            "target": "chip-a",
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
    let index = HilEvidenceIndex::load(
        &root,
        Path::new("runs"),
        Path::new("evidence"),
        "chip-a",
        &repository,
    )
    .unwrap();
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
    let index = HilEvidenceIndex::load(
        &root,
        Path::new("runs"),
        Path::new("evidence"),
        "chip-a",
        &repository,
    )
    .unwrap();
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
    let error = HilEvidenceIndex::rejection(HilEvidenceIndex::load(
        &root,
        Path::new("runs"),
        Path::new("evidence"),
        "chip-a",
        &repository,
    ))
    .unwrap();
    assert!(error.contains("cannot parse HIL evidence"));
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
            "target": "chip-a",
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

    complete(&run);
    let repository = RepositoryState {
        commit: "abc123".to_owned(),
        dirty: false,
    };
    let error = HilEvidenceIndex::rejection(HilEvidenceIndex::load(
        &root,
        Path::new("runs"),
        Path::new("evidence"),
        "chip-a",
        &repository,
    ))
    .unwrap();
    assert!(error.contains("integrity.json"));
    fs::remove_dir_all(root).unwrap();
}

pub(super) fn add_current_build(root: &Path, run: &Path) {
    fs::create_dir_all(root.join("hil/host/runner/src")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = '3'\nmembers = ['hil/host/runner']\n",
    )
    .unwrap();
    fs::write(
        root.join("hil/host/runner/Cargo.toml"),
        "[package]\nname = 'oer-hil-runner'\nversion = '0.1.0'\nedition = '2024'\n",
    )
    .unwrap();
    fs::write(root.join("hil/host/runner/src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(
        root.join("Cargo.lock"),
        "version = 4\n[[package]]\nname = 'oer-hil-runner'\nversion = '0.1.0'\n",
    )
    .unwrap();
    let mut manifest: serde_json::Value = read_json(&run.join("manifest.json")).unwrap();
    fs::create_dir_all(root.join("hil/schema")).unwrap();
    fs::write(root.join("observer.rs"), b"test observer").unwrap();
    let registry: serde_json::Value =
        serde_json::from_str(include_str!("../../../../hil/schema/observer-inputs.json")).unwrap();
    fs::write(
        root.join("hil/schema/observer-inputs.json"),
        serde_json::to_vec(&json!({"schema":4,"data":["observer.rs"],"timing":registry["timing"],"dependencies":{"common":[],"wifi":[],"bluetooth":[],"system":[],"ieee802154":[],"coexistence":[],"phy":[]},"build":{"profile":"debug","opt_level":"0","debug":"true"}}))
            .unwrap(),
    )
    .unwrap();
    let registry = read_json(&root.join("hil/schema/observer-inputs.json")).unwrap();
    let configuration = observer::required_configuration(root, &registry).unwrap();
    fs::write(
        root.join("hil/host/runner/src/main.rs"),
        format!(
            "fn main() {{ println!(\"{{}}\", {:?}); }}\n",
            serde_json::to_string(&configuration).unwrap()
        ),
    )
    .unwrap();
    let resolved = prepare_observer(root);
    let build = json!({"schema":oer_hil_run_bundle_format::observer::BUILD_SCHEMA,"inputs":{
        "observer.rs":crate::digests().sha256_file(&root.join("observer.rs")).unwrap(),
        "hil/host/runner/src/main.rs":crate::digests().sha256_file(&root.join("hil/host/runner/src/main.rs")).unwrap(),
    },"compiler":configuration["compiler"],"environment":configuration["environment"],"resolved":resolved});
    // The run's observer build is stored beside the directory of runs.
    let embedded = json!({"schema":1,"executable_sha256":"aa".repeat(32),"build_sha256":format!("{:x}",Sha256::digest(serde_json::to_vec(&build).unwrap())),"build":build});
    let store = fs::canonicalize(run)
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_owned();
    manifest["runner"] = json!({"observer": oer_hil_run_bundle_format::observer::store::detach(&embedded, &store).unwrap()});

    manifest["firmware"] = json!([{
        "build_id": "ab".repeat(32),
        "build_provenance_path": "build-provenance.json",
    }]);
    fs::write(
        run.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    let lock = root.join("Cargo.lock");
    let provenance = json!({
        "schema": 1,
        "build_type": "open-esp-radio-hil-firmware/v1",
        "build_id": "ab".repeat(32),
        "parameters": {
            "image": "boot-smoke", "network": null, "runtime_profile": "release",
            "target": "riscv32imafc-unknown-none-elf", "runtime_features": "",
        },
        "source_reconstructable": true,
        "sources": [{
            "name": "repository",
            "checkout_path": root,
            "remote": null,
            "commit": manifest["repository"]["commit"],
            "workspace_sha256": manifest["repository"]["workspace_sha256"],
            "dirty": false,
            "rebuild_status": "clean-commit",
            "limitations": [],
            "untracked_files": [],
            "tracked_patch_path": null,
            "tracked_patch_size_bytes": null,
            "tracked_patch_sha256": null,
        }],
        "files": [{
            "name": "workspace-lock",
            "path": "Cargo.lock",
            "archive_path": null,
            "size_bytes": fs::metadata(&lock).unwrap().len(),
            "sha256": crate::digests().sha256_file(&lock).unwrap(),
        }],
        "environment": {
            "tools": [], "inherited_rustflags": null, "inherited_encoded_rustflags": null,
            "cargo_incremental": "0", "source_date_epoch": null,
        },
        "subjects": [],
        "reproducibility": "unverified",
    });
    fs::write(
        run.join("build-provenance.json"),
        serde_json::to_vec(&provenance).unwrap(),
    )
    .unwrap();
}

pub(super) fn prepare_observer(root: &Path) -> serde_json::Value {
    let registry = read_json(&root.join("hil/schema/observer-inputs.json")).unwrap();
    let configuration = observer::required_configuration(root, &registry).unwrap();
    use oer_hil_observer::resolve;
    let mut resolved = resolve::resolve(
        root,
        configuration["environment"]["TARGET"].as_str().unwrap(),
    )
    .unwrap();
    for node in resolved["packages"].as_array_mut().unwrap() {
        node["units"] = json!([{"kind":["lib"],"profile":{"opt_level":"0"}}]);
    }
    resolved["compilation"] = json!("cargo-compiler-artifacts-v1");
    resolved["selected_profile"] = json!("dev");
    resolved["configuration"] = configuration.clone();
    fs::create_dir_all(root.join("target/hil")).unwrap();
    fs::write(root.join("target/hil/current-observer.json"), serde_json::to_vec(&json!({"build":{"schema":oer_hil_run_bundle_format::observer::BUILD_SCHEMA,"resolved":resolved,"compiler":configuration["compiler"],"environment":configuration["environment"]}})).unwrap()).unwrap();
    resolved
}

#[test]
fn a_recorded_shard_qualifies_while_its_sources_are_unchanged() {
    let root = std::env::temp_dir().join(format!(
        "open-radio-qualification-hil-shard-{}",
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
            "schema": 2, "run_id": "run-1", "target": "chip-a", "state": "completed",
            "started_unix_millis": 100, "finished_unix_millis": 200, "duration_millis": 100,
            "repository": {"commit": "abc123", "dirty": false, "workspace_sha256": "00".repeat(32)}
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        run.join("suite.json"),
        serde_json::to_vec_pretty(&json!({
            "schema": 2, "run_id": "run-1", "target": "chip-a", "outcome": "passed",
            "started_unix_millis": 100, "finished_unix_millis": 200, "duration_millis": 100,
            "counts": {"scenarios": 1, "passed": 1, "failed": 0, "broken": 0, "skipped": 0,
                "blocked": 0, "interrupted": 0},
            "scenarios": [{
                "schema": 2, "scenario": "station-reconnect", "outcome": "passed",
                "required_repetitions": 2,
                "repetitions": [
                    {"schema": 2, "repetition": 1, "outcome": "passed", "failure": null,
                        "measurements": [{"name": "received", "value": 40, "unit": "count", "semantics": 1, "better": null,
                            "threshold": null, "verdict": null}]},
                    {"schema": 2, "repetition": 2, "outcome": "passed", "failure": null}
                ],
                "failure": null,
            }]
        }))
        .unwrap(),
    )
    .unwrap();
    add_current_build(&root, &run);
    seal(&run);
    fs::create_dir_all(root.join("firmware/src")).unwrap();
    fs::write(root.join("firmware/src/lib.rs"), "one").unwrap();
    let requirement = HilRequirement {
        scenario: "station-reconnect".to_owned(),
        checks: vec![],
        minimum_repetitions: 2,
    };
    let (runs, evidence) = (Path::new("runs"), Path::new("evidence"));
    let repository = RepositoryState {
        commit: "abc123".to_owned(),
        dirty: false,
    };
    let index = HilEvidenceIndex::load(&root, runs, evidence, "chip-a", &repository).unwrap();
    let recorded = shard::distill(
        &root,
        &index,
        evidence,
        "chip-a",
        &[PathBuf::from("firmware")],
        None,
    )
    .unwrap();
    assert_eq!(recorded, ["station-reconnect"]);
    // The shard names its observer build, stored once beside it.
    let shard: serde_json::Value =
        read_json(&root.join("evidence/station-reconnect.json")).unwrap();
    let observer = &shard["subject"]["observer"];
    assert!(observer.get("build").is_none());
    let build = oer_hil_run_bundle_format::observer::store::path(
        &root.join(evidence),
        observer["build_sha256"].as_str().unwrap(),
    )
    .unwrap();
    assert!(build.is_file());

    // Without the run bundle and at another commit, the shard still
    // qualifies, with its measurements.
    fs::remove_dir_all(root.join("runs")).unwrap();
    let later = RepositoryState {
        commit: "later-commit".to_owned(),
        dirty: false,
    };
    let index = HilEvidenceIndex::load(&root, runs, evidence, "chip-a", &later).unwrap();
    assert_eq!(index.summary().shards, 1);
    assert_eq!(index.summary().current_shards, 1);
    assert!(
        index
            .evidence_for(&requirement, &ScenarioCatalog::default())
            .is_some()
    );
    assert_eq!(
        index.scenarios["station-reconnect"][0].measurements[0],
        [
            json!({"name": "received", "value": 40, "unit": "count", "semantics": 1, "better": null, "threshold": null,
            "verdict": null})
        ]
    );

    // A changed firmware source makes it stale.
    fs::write(root.join("firmware/src/lib.rs"), "two").unwrap();
    let index = HilEvidenceIndex::load(&root, runs, evidence, "chip-a", &later).unwrap();
    assert_eq!(index.summary().current_shards, 0);
    assert!(
        index
            .evidence_for(&requirement, &ScenarioCatalog::default())
            .is_none()
    );

    // A shard whose observer build is missing fails closed.
    fs::write(root.join("firmware/src/lib.rs"), "one").unwrap();
    let stored = fs::read(&build).unwrap();
    fs::remove_file(&build).unwrap();
    assert!(HilEvidenceIndex::load(&root, runs, evidence, "chip-a", &later).is_err());
    fs::write(&build, stored).unwrap();
    assert!(HilEvidenceIndex::load(&root, runs, evidence, "chip-a", &later).is_ok());

    // A shard binding a source that no longer exists fails closed instead
    // of reading as stale.
    fs::rename(root.join("firmware"), root.join("moved")).unwrap();
    let error = HilEvidenceIndex::load(&root, runs, evidence, "chip-a", &later)
        .expect_err("a missing bound source")
        .to_string();
    assert!(error.contains("does not exist"), "{error}");
    fs::rename(root.join("moved"), root.join("firmware")).unwrap();

    // A shard names its own scenario, and only shards live in the directory.
    fs::write(root.join("evidence/notes.txt"), "").unwrap();
    assert!(HilEvidenceIndex::load(&root, runs, evidence, "chip-a", &later).is_err());
    fs::remove_dir_all(root).unwrap();
}

/// The current observer's side of an assessment depends on the workload
/// alone and a recorded build is shared by the runs that name it: however
/// many runs a store holds, the current graph is projected once per
/// workload, a workload's inputs are hashed once, and each recorded build
/// is read and projected once per workload.
#[test]
fn observer_assessment_work_does_not_grow_with_the_runs() {
    let root = std::env::temp_dir().join(format!(
        "open-radio-qualification-hil-observer-work-{}",
        std::process::id()
    ));
    if root.exists() {
        fs::remove_dir_all(&root).unwrap();
    }
    let runs = ["run-1", "run-2", "run-3"];
    for (number, run_id) in runs.into_iter().enumerate() {
        let run = root.join("runs").join(run_id);
        fs::create_dir_all(&run).unwrap();
        fs::write(
            run.join("manifest.json"),
            serde_json::to_vec_pretty(&json!({
                "schema": 2, "run_id": run_id, "target": "chip-a", "state": "completed",
                "started_unix_millis": 100, "finished_unix_millis": 200, "duration_millis": 100,
                "repository": {"commit": "abc123", "dirty": false, "workspace_sha256": "00".repeat(32)}
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(
            run.join("suite.json"),
            serde_json::to_vec_pretty(&json!({
                "schema": 2, "run_id": run_id, "target": "chip-a", "outcome": "passed",
                "started_unix_millis": 100, "finished_unix_millis": 200, "duration_millis": 100,
                "counts": {"scenarios": 1, "passed": 1, "failed": 0, "broken": 0, "skipped": 0,
                    "blocked": 0, "interrupted": 0},
                "scenarios": [{
                    "schema": 2, "scenario": "station-reconnect", "outcome": "passed",
                    "required_repetitions": 1,
                    "repetitions": [{"schema": 2, "repetition": 1, "outcome": "passed", "failure": null}],
                    "failure": null,
                }]
            }))
            .unwrap(),
        )
        .unwrap();
        // Every run names the one observer build the first stores.
        if number == 0 {
            add_current_build(&root, &run);
        } else {
            let first: serde_json::Value =
                read_json(&root.join("runs").join(runs[0]).join("manifest.json")).unwrap();
            let mut manifest: serde_json::Value = read_json(&run.join("manifest.json")).unwrap();
            manifest["runner"] = first["runner"].clone();
            manifest["firmware"] = json!([{
                "build_id": "ab".repeat(32),
                "build_provenance_path": "build-provenance.json",
            }]);
            fs::write(
                run.join("manifest.json"),
                serde_json::to_vec(&manifest).unwrap(),
            )
            .unwrap();
            fs::copy(
                root.join("runs")
                    .join(runs[0])
                    .join("build-provenance.json"),
                run.join("build-provenance.json"),
            )
            .unwrap();
        }
        seal(&run);
    }
    let registry = read_json(&root.join("hil/schema/observer-inputs.json")).unwrap();
    let workloads = observer::build_inputs::workloads(&registry).unwrap().len() + 1;
    let repository = RepositoryState {
        commit: "abc123".to_owned(),
        dirty: false,
    };
    observer::work::take();
    let index = HilEvidenceIndex::load(
        &root,
        Path::new("runs"),
        Path::new("evidence"),
        "chip-a",
        &repository,
    )
    .unwrap();
    assert_eq!(
        index.summary().qualifying,
        runs.len(),
        "{:?}",
        index.summary()
    );
    assert_eq!(
        observer::work::take(),
        observer::work::Work {
            current_projections: workloads,
            input_hashes: 1,
            build_loads: 1,
            recorded_projections: 1,
        }
    );
    // Observations something else already excludes read no observer build,
    // so time follows the runs that can still apply, not the store; runs
    // named for recording are still assessed.
    let other = RepositoryState {
        commit: "other-commit".to_owned(),
        dirty: false,
    };
    let index = HilEvidenceIndex::load(
        &root,
        Path::new("runs"),
        Path::new("evidence"),
        "chip-a",
        &other,
    )
    .unwrap();
    assert_eq!(index.summary().qualifying, 0);
    assert_eq!(observer::work::take().build_loads, 0);
    assert!(
        index.scenarios["station-reconnect"]
            .iter()
            .all(|o| o.exclusions == [decision::Exclusion::DifferentCommit])
    );
    let named = runs.map(str::to_owned).into_iter().collect::<BTreeSet<_>>();
    let index = HilEvidenceIndex::load_selected(
        &root,
        Path::new("runs"),
        Path::new("evidence"),
        "chip-a",
        &other,
        Some(&named),
    )
    .unwrap();
    assert_eq!(observer::work::take().build_loads, 1);
    assert_eq!(index.summary().qualifying, 0);
    fs::remove_dir_all(root).unwrap();
}

/// A run's manifest names its observer build by digest: a manifest that
/// embeds the build, a missing build or one that does not hash to its name
/// fails closed.
#[test]
fn a_referenced_observer_build_is_the_only_stored_form() {
    use oer_hil_run_bundle_format::observer::store as observer_store;
    let root = std::env::temp_dir().join(format!(
        "open-radio-qualification-hil-observer-{}",
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
            "schema": 2, "run_id": "run-1", "target": "chip-a", "state": "completed",
            "started_unix_millis": 100, "finished_unix_millis": 200, "duration_millis": 100,
            "repository": {"commit": "abc123", "dirty": false, "workspace_sha256": "00".repeat(32)}
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        run.join("suite.json"),
        serde_json::to_vec_pretty(&json!({
            "schema": 2, "run_id": "run-1", "target": "chip-a", "outcome": "passed",
            "started_unix_millis": 100, "finished_unix_millis": 200, "duration_millis": 100,
            "counts": {"scenarios": 1, "passed": 1, "failed": 0, "broken": 0, "skipped": 0,
                "blocked": 0, "interrupted": 0},
            "scenarios": [{
                "schema": 2, "scenario": "station-reconnect", "outcome": "passed",
                "required_repetitions": 1,
                "repetitions": [{"schema": 2, "repetition": 1, "outcome": "passed", "failure": null}],
                "failure": null,
            }]
        }))
        .unwrap(),
    )
    .unwrap();
    add_current_build(&root, &run);
    seal(&run);
    let requirement = HilRequirement {
        scenario: "station-reconnect".to_owned(),
        checks: vec![],
        minimum_repetitions: 1,
    };
    let repository = RepositoryState {
        commit: "abc123".to_owned(),
        dirty: false,
    };
    let evaluate = || {
        HilEvidenceIndex::load(
            &root,
            Path::new("runs"),
            Path::new("evidence"),
            "chip-a",
            &repository,
        )
        .map(|index| {
            let evidence = index.evidence_for(&requirement, &ScenarioCatalog::default());
            format!("{:?} {evidence:?}", index.summary())
        })
    };
    let referenced = evaluate().unwrap();
    assert!(referenced.contains("Some("), "the referenced run qualifies");

    let mut manifest: serde_json::Value = read_json(&run.join("manifest.json")).unwrap();
    let reference = manifest["runner"]["observer"].clone();
    manifest["runner"]["observer"] =
        observer_store::attach(&reference, &root).expect("the stored build");
    fs::write(
        run.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    seal(&run);
    assert!(evaluate().is_err(), "a manifest embedding its build");
    manifest["runner"]["observer"] = reference.clone();
    fs::write(
        run.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    seal(&run);
    let build =
        observer_store::path(&root, observer_store::build_digest(&reference).unwrap()).unwrap();
    let stored = fs::read(&build).unwrap();
    fs::write(&build, b"{}").unwrap();
    assert!(
        evaluate().is_err(),
        "a build that does not hash to its name"
    );
    fs::remove_file(&build).unwrap();
    assert!(evaluate().is_err(), "a missing build");
    fs::write(&build, stored).unwrap();
    assert_eq!(evaluate().unwrap(), referenced);
    fs::remove_dir_all(root).unwrap();
}

/// A run that archived its source snapshot records the same shard from any
/// checkout, bound to the snapshot's sources; recording one run leaves the
/// shards of scenarios only other runs observed untouched.
#[test]
fn a_recorded_run_binds_its_own_snapshot_and_touches_only_its_scenarios() {
    let root = std::env::temp_dir().join(format!(
        "open-radio-qualification-hil-scoped-{}",
        std::process::id()
    ));
    if root.exists() {
        fs::remove_dir_all(&root).unwrap();
    }
    let observe = |id: &str, scenario: &str, started: u64| {
        let run = root.join("runs").join(id);
        fs::create_dir_all(&run).unwrap();
        fs::write(
            run.join("manifest.json"),
            serde_json::to_vec_pretty(&json!({
                "schema": 2, "run_id": id, "target": "chip-a", "state": "completed",
                "started_unix_millis": started, "finished_unix_millis": started + 100,
                "duration_millis": 100,
                "repository": {"commit": "abc123", "dirty": false, "workspace_sha256": "00".repeat(32)}
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(
            run.join("suite.json"),
            serde_json::to_vec_pretty(&json!({
                "schema": 2, "run_id": id, "target": "chip-a", "outcome": "passed",
                "started_unix_millis": started, "finished_unix_millis": started + 100,
                "duration_millis": 100,
                "counts": {"scenarios": 1, "passed": 1, "failed": 0, "broken": 0, "skipped": 0,
                    "blocked": 0, "interrupted": 0},
                "scenarios": [{
                    "schema": 2, "scenario": scenario, "outcome": "passed",
                    "required_repetitions": 1,
                    "repetitions": [{"schema": 2, "repetition": 1, "outcome": "passed", "failure": null}],
                    "failure": null,
                }]
            }))
            .unwrap(),
        )
        .unwrap();
        add_current_build(&root, &run);
        // The run's snapshot holds its own firmware source.
        let snapshot = run.join("source/snapshot");
        fs::create_dir_all(&snapshot).unwrap();
        let mut archive =
            tar::Builder::new(fs::File::create(snapshot.join("sources.tar")).unwrap());
        let bytes = format!("{id} source");
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        archive
            .append_data(
                &mut header,
                "repository/firmware/src/lib.rs",
                bytes.as_bytes(),
            )
            .unwrap();
        archive.finish().unwrap();
        drop(archive);
        seal(&run);
    };
    observe("run-1", "station-reconnect", 100);
    observe("run-2", "station-roam", 200);
    fs::create_dir_all(root.join("firmware/src")).unwrap();
    let (runs, evidence) = (Path::new("runs"), Path::new("evidence"));
    let repository = RepositoryState {
        commit: "abc123".to_owned(),
        dirty: false,
    };
    let record = |run: &str| {
        let only = BTreeSet::from([run.to_owned()]);
        let index = HilEvidenceIndex::load_selected(
            &root,
            runs,
            evidence,
            "chip-a",
            &repository,
            Some(&only),
        )
        .unwrap();
        shard::distill(
            &root,
            &index,
            evidence,
            "chip-a",
            &[PathBuf::from("firmware")],
            Some(&only),
        )
        .unwrap()
    };
    let shard = |scenario: &str| fs::read(root.join(format!("evidence/{scenario}.json"))).ok();

    fs::write(root.join("firmware/src/lib.rs"), "checkout one").unwrap();
    assert_eq!(record("run-2"), ["station-roam"]);
    let roam = shard("station-roam").unwrap();
    assert_eq!(record("run-1"), ["station-reconnect"]);
    let first = shard("station-reconnect").unwrap();
    assert_eq!(
        shard("station-roam").unwrap(),
        roam,
        "another run's shard is untouched"
    );

    // Another checkout content records the same bytes: the snapshot binds.
    fs::write(root.join("firmware/src/lib.rs"), "checkout two").unwrap();
    assert_eq!(record("run-1"), ["station-reconnect"]);
    assert_eq!(shard("station-reconnect").unwrap(), first);
    let recorded: serde_json::Value = serde_json::from_slice(&first).unwrap();
    let snapshot_digest = {
        let name = "firmware/src/lib.rs";
        let bytes = b"run-1 source";
        let mut hash = Sha256::new();
        hash.update((name.len() as u64).to_le_bytes());
        hash.update(name.as_bytes());
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
        format!("{:x}", hash.finalize())
    };
    assert_eq!(recorded["sources"][0]["path"], "firmware");
    assert_eq!(recorded["sources"][0]["sha256"], snapshot_digest);

    // Each requested run explains what became of each scenario it observed.
    let explain = |run: &str, commit: &str| {
        let only = BTreeSet::from([run.to_owned()]);
        let repository = RepositoryState {
            commit: commit.to_owned(),
            dirty: false,
        };
        let index = HilEvidenceIndex::load_selected(
            &root,
            runs,
            evidence,
            "chip-a",
            &repository,
            Some(&only),
        )
        .unwrap();
        let sources = [PathBuf::from("firmware")];
        let recorded =
            shard::distill(&root, &index, evidence, "chip-a", &sources, Some(&only)).unwrap();
        shard::explain(&root, &index, &only, &recorded, evidence, &sources).unwrap()
    };
    assert_eq!(
        explain("run-1", "abc123"),
        [(
            "run-1".to_owned(),
            "station-reconnect".to_owned(),
            shard::RunVerdict::Recorded
        )]
    );
    // At another commit, the run names the sources that differ from its snapshot.
    let verdicts = explain("run-1", "def456");
    assert_eq!(verdicts.len(), 1);
    let verdict = &verdicts[0].2;
    assert_eq!(verdict.id(), "different-commit");
    assert_eq!(
        verdict.detail(),
        "run inputs differ from the checkout in: firmware"
    );
    assert_eq!(
        explain("missing-run", "abc123")[0].2,
        shard::RunVerdict::NotFound
    );
    fs::remove_dir_all(root).unwrap();
}

fn no_views(
    _: &oer_hil_run_bundle_format::run::SuiteResult,
    _: &oer_hil_run_bundle_format::run::RunManifest,
) -> oer_hil_run_bundle::run::Views {
    oer_hil_run_bundle::run::Views::default()
}

/// The evaluator admits exactly what the runner's writer seals, through the
/// run bundle's one reader, and still refuses it once a sealed file changes.
#[test]
fn a_bundle_the_writer_seals_is_admitted_through_the_shared_reader() {
    use oer_hil_run_bundle::run::test_support::session;
    use oer_hil_run_bundle_format::run::RepetitionResult;
    use oer_hil_run_bundle_format::run::ScenarioResult;
    let root = tempfile::tempdir().unwrap();
    let run = root.path().join("runs/1700000000000-00000abc");
    fs::create_dir_all(run.join("scenarios/boot-smoke/repetition-001")).unwrap();
    let scenario = ScenarioResult::from_repetitions(
        String::from("boot-smoke"),
        oer_hil_schema::image::ImageClass::BootSmoke,
        1,
        vec![RepetitionResult {
            schema: oer_hil_run_bundle_format::run::RUN_SCHEMA,
            repetition: 1,
            outcome: Outcome::Passed,
            started_unix_millis: 1,
            duration_millis: 1,
            artifact_directory: PathBuf::from("scenarios/boot-smoke/repetition-001"),
            attachments: Vec::new(),
            measurements: Vec::new(),
            failure: None,
        }],
    );
    session(&run).finish(vec![scenario], no_views).unwrap();
    let load = || {
        HilEvidenceIndex::load(
            root.path(),
            Path::new("runs"),
            Path::new("evidence"),
            "chip-a",
            &RepositoryState {
                commit: String::new(),
                dirty: false,
            },
        )
    };
    let index = load().unwrap();
    assert!(
        index.summary().invalid.is_empty(),
        "{:?}",
        index.summary().invalid
    );
    assert_eq!((index.summary().bundles, index.summary().completed), (1, 1));
    assert_eq!(index.scenarios["boot-smoke"].len(), 1);
    let suite = run.join("suite.json");
    let mut bytes = fs::read(&suite).unwrap();
    bytes.push(b'\n');
    fs::write(&suite, bytes).unwrap();
    let rejected = HilEvidenceIndex::rejection(load()).unwrap();
    assert!(rejected.contains("sealed file inventory"), "{rejected}");
}

/// A run of another schema is counted by its schema, not reported invalid
/// one by one; another chip's run of another schema is not counted at all.
#[test]
fn runs_of_another_schema_are_counted_by_schema() {
    let root = std::env::temp_dir().join(format!(
        "open-radio-qualification-hil-unsupported-{}",
        std::process::id()
    ));
    if root.exists() {
        fs::remove_dir_all(&root).unwrap();
    }
    let raw = |id: &str, schema: u64, target: &str| {
        let run = root.join("runs").join(id);
        fs::create_dir_all(&run).unwrap();
        fs::write(
            run.join("manifest.json"),
            serde_json::to_vec(&json!({"schema": schema, "run_id": id, "target": target})).unwrap(),
        )
        .unwrap();
    };
    raw("old-a", 3, "chip-a");
    raw("old-b", 3, "chip-a");
    raw("newer", u64::from(RUN_SCHEMA) + 1, "chip-a");
    raw("other-chip", 3, "chip-b");
    let index = HilEvidenceIndex::load(
        &root,
        Path::new("runs"),
        Path::new("evidence"),
        "chip-a",
        &RepositoryState {
            commit: "abc123".to_owned(),
            dirty: false,
        },
    )
    .unwrap();
    let summary = index.summary();
    assert!(summary.invalid.is_empty(), "{:?}", summary.invalid);
    assert_eq!(
        summary.unsupported,
        BTreeMap::from([(3, 2), (u64::from(RUN_SCHEMA) + 1, 1)])
    );
    assert_eq!((summary.directories, summary.bundles), (3, 3));
    fs::remove_dir_all(root).unwrap();
}
