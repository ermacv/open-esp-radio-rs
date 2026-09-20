use super::*;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    run: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "oer-attempt-consumer-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let run = root.join("runs/run-1");
        fs::create_dir_all(run.join("scenarios/boot-smoke")).unwrap();
        fs::create_dir_all(run.join("firmware/boot-smoke")).unwrap();
        fs::create_dir_all(run.join("attempts")).unwrap();
        write(
            &run.join("manifest.json"),
            &json!({
                "schema":2,"run_id":"run-1","target":"esp32s31","state":"running",
                "started_unix_millis":100,"finished_unix_millis":null,"duration_millis":null,
                "repository":{"commit":"current","dirty":false,"workspace_sha256":"00".repeat(32)}
            }),
        );
        crate::hil::tests::add_current_build(&root, &run);
        fs::rename(
            run.join("build-provenance.json"),
            run.join("firmware/boot-smoke/build-provenance.json"),
        )
        .unwrap();
        let mut manifest: Value = read_json(&run.join("manifest.json")).unwrap();
        manifest["firmware"][0]["image"] = json!("boot-smoke");
        let application = run.join("firmware/boot-smoke/application.bin");
        fs::write(&application, b"synthetic firmware").unwrap();
        manifest["firmware"][0]["application_path"] = json!("firmware/boot-smoke/application.bin");
        manifest["firmware"][0]["application_size_bytes"] =
            json!(fs::metadata(&application).unwrap().len());
        manifest["firmware"][0]["application_sha256"] = json!(sha256_file(&application).unwrap());
        manifest["firmware"][0]["build_provenance_path"] =
            json!("firmware/boot-smoke/build-provenance.json");
        write(&run.join("manifest.json"), &manifest);
        manifest["state"] = json!("completed");
        manifest["finished_unix_millis"] = json!(200);
        manifest["duration_millis"] = json!(100);
        let result = json!({"schema":2,"scenario":"boot-smoke","image":"boot-smoke","outcome":"passed","required_repetitions":1,"failure":null,
            "repetitions":[{"schema":2,"repetition":1,"outcome":"passed","measurements":[],"failure":null}]});
        write(
            &run.join("scenarios/boot-smoke/scenario.json"),
            &json!({"id":"boot-smoke","image":"boot-smoke","repetitions":1}),
        );
        write(&run.join("scenarios/boot-smoke/result.json"), &result);
        let files = collect_inventory(&run, false).unwrap().into_iter()
            .filter(|(p,_)| p.starts_with("scenarios") || p.starts_with("firmware"))
            .map(|(p,size)| json!({"path":p,"size_bytes":size,"sha256":sha256_file(&run.join(&p)).unwrap()})).collect::<Vec<_>>();
        write(
            &run.join("attempts/boot-smoke.json"),
            &json!({"schema":1,"manifest":manifest,"files":files,
            "suite":{"schema":2,"run_id":"run-1","target":"esp32s31","outcome":"passed",
                "started_unix_millis":100,"finished_unix_millis":200,"duration_millis":100,
                "counts":{"scenarios":1,"passed":1,"failed":0,"broken":0,"blocked":0,"interrupted":0,"skipped":0},
                "scenarios":[result]}}),
        );
        Self { root, run }
    }

    fn load(&self) -> Result<HilEvidenceIndex> {
        HilEvidenceIndex::load(
            &self.root,
            Path::new("runs"),
            "esp32s31",
            &RepositoryState {
                commit: "current".into(),
                dirty: false,
            },
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}

fn write(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

fn requirement() -> HilRequirement {
    HilRequirement {
        scenario: "boot-smoke".into(),
        checks: vec![],
        minimum_repetitions: 1,
    }
}

#[test]
fn sealed_attempt_survives_running_interrupted_and_completed_campaign_without_double_counting() {
    let fixture = Fixture::new();
    for state in ["running", "interrupted", "completed"] {
        let mut parent: Value = read_json(&fixture.run.join("manifest.json")).unwrap();
        parent["state"] = json!(state);
        write(&fixture.run.join("manifest.json"), &parent);
        let index = fixture.load().unwrap();
        assert_eq!(index.summary.sealed_attempts, 1);
        assert_eq!(index.scenarios["boot-smoke"].len(), 1);
        assert!(
            index
                .evidence_for(&requirement(), &ScenarioCatalog::default())
                .is_some()
        );
    }
    // Neither later output nor another image belongs to the closed experiment.
    fs::create_dir_all(fixture.run.join("scenarios/next")).unwrap();
    fs::create_dir_all(fixture.run.join("firmware/another-image")).unwrap();
    fs::write(fixture.run.join("scenarios/next/partial.log"), "partial").unwrap();
    assert!(
        fixture
            .load()
            .unwrap()
            .evidence_for(&requirement(), &ScenarioCatalog::default())
            .is_some()
    );
}

#[test]
fn uncommitted_publication_is_not_evidence() {
    let fixture = Fixture::new();
    fs::rename(
        fixture.run.join("attempts/boot-smoke.json"),
        fixture.run.join("attempts/.boot-smoke.json.tmp-1-0"),
    )
    .unwrap();
    let index = fixture.load().unwrap();
    assert_eq!(index.summary.sealed_attempts, 0);
    assert!(
        index
            .evidence_for(&requirement(), &ScenarioCatalog::default())
            .is_none()
    );
}

#[test]
fn later_image_preparation_does_not_rebind_a_preflight_blocked_attempt() {
    let fixture = Fixture::new();
    let path = fixture.run.join("attempts/boot-smoke.json");
    let mut seal: Value = read_json(&path).unwrap();
    seal["manifest"]["firmware"] = json!([]);
    seal["suite"]["outcome"] = json!("failed");
    seal["suite"]["counts"]["passed"] = json!(0);
    seal["suite"]["counts"]["blocked"] = json!(1);
    seal["suite"]["scenarios"][0]["outcome"] = json!("blocked");
    seal["suite"]["scenarios"][0]["repetitions"] = json!([]);
    seal["suite"]["scenarios"][0]["failure"] =
        json!({"kind":"precondition","message":"not available"});
    let result = fixture.run.join("scenarios/boot-smoke/result.json");
    write(&result, &seal["suite"]["scenarios"][0]);
    seal["files"]
        .as_array_mut()
        .unwrap()
        .retain(|file| !file["path"].as_str().unwrap().starts_with("firmware/"));
    for file in seal["files"].as_array_mut().unwrap() {
        if file["path"] == "scenarios/boot-smoke/result.json" {
            file["size_bytes"] = json!(fs::metadata(&result).unwrap().len());
            file["sha256"] = json!(sha256_file(&result).unwrap());
        }
    }
    write(&path, &seal);
    // Another selected scenario can prepare this class after preflight blocked
    // the first one. Its firmware must not enter the earlier attempt's subject.
    let index = fixture.load().unwrap();
    assert_eq!(index.summary.sealed_attempts, 1);
    assert_eq!(index.scenarios["boot-smoke"][0].outcome, Outcome::Blocked);
    assert!(
        index.scenarios["boot-smoke"][0]
            .exclusions
            .contains(&decision::Exclusion::SourceBindingNotEstablished)
    );
}

#[test]
fn altered_removed_added_or_unsealed_material_fails_closed() {
    for change in [
        "alter",
        "remove",
        "add",
        "omit",
        "wrong-subject",
        "wrong-bytes",
        "escape",
    ] {
        let fixture = Fixture::new();
        let material = fixture
            .run
            .join("firmware/boot-smoke/build-provenance.json");
        let seal_path = fixture.run.join("attempts/boot-smoke.json");
        let mut seal: Value = read_json(&seal_path).unwrap();
        match change {
            "alter" => fs::write(material, "{}\n").unwrap(),
            "remove" => fs::remove_file(material).unwrap(),
            "add" => fs::write(
                fixture.run.join("scenarios/boot-smoke/unaccounted.log"),
                "x",
            )
            .unwrap(),
            "omit" => {
                seal["files"].as_array_mut().unwrap().pop();
                write(&seal_path, &seal);
            }
            "wrong-subject" => {
                seal["manifest"]["firmware"][0]["image"] = json!("bluetooth-gatt");
                write(&seal_path, &seal);
            }
            "wrong-bytes" => {
                seal["manifest"]["firmware"][0]["application_sha256"] = json!("00".repeat(32));
                write(&seal_path, &seal);
            }
            "escape" => {
                seal["manifest"]["firmware"][0]["build_provenance_path"] =
                    json!("../unsealed.json");
                write(&seal_path, &seal);
            }
            _ => unreachable!(),
        }
        assert!(fixture.load().is_err(), "{change}");
    }
}

#[test]
fn sealed_failure_is_indexed_even_when_campaign_never_finishes() {
    let fixture = Fixture::new();
    let path = fixture.run.join("attempts/boot-smoke.json");
    let mut seal: Value = read_json(&path).unwrap();
    seal["suite"]["outcome"] = json!("failed");
    seal["suite"]["counts"]["passed"] = json!(0);
    seal["suite"]["counts"]["failed"] = json!(1);
    seal["suite"]["scenarios"][0]["outcome"] = json!("failed");
    seal["suite"]["scenarios"][0]["repetitions"][0]["outcome"] = json!("failed");
    seal["suite"]["scenarios"][0]["repetitions"][0]["failure"] =
        json!({"kind":"scenario","message":"failure"});
    let result = fixture.run.join("scenarios/boot-smoke/result.json");
    write(&result, &seal["suite"]["scenarios"][0]);
    for file in seal["files"].as_array_mut().unwrap() {
        if file["path"] == "scenarios/boot-smoke/result.json" {
            file["size_bytes"] = json!(fs::metadata(&result).unwrap().len());
            file["sha256"] = json!(sha256_file(&result).unwrap());
        }
    }
    write(&path, &seal);
    assert_eq!(
        fixture
            .load()
            .unwrap()
            .decision_for(&requirement(), &ScenarioCatalog::default())
            .status,
        EvidenceStatus::UnresolvedFailure
    );
}

#[cfg(unix)]
#[test]
fn symlinked_material_root_is_rejected() {
    let fixture = Fixture::new();
    fs::rename(
        fixture.run.join("firmware"),
        fixture.root.join("saved-firmware"),
    )
    .unwrap();
    std::os::unix::fs::symlink(
        fixture.root.join("saved-firmware"),
        fixture.run.join("firmware"),
    )
    .unwrap();
    assert!(fixture.load().is_err());
}
