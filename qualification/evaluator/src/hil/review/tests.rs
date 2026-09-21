use super::*;
use crate::hil::decision::tests::{Fixture, scenario, write};
use serde_json::{Value, json};

fn declaration() -> CapabilityDocument {
    toml_edit::de::from_str(
        r#"
id = "att"
title = "ATT exchange"
scope = "One established connection"
implementation = "complete"
host = "covered"
async = "bounded"
vendor-not-applicable = "host-owned-policy"
hil-requirements = [{ scenario = "exchange", minimum-repetitions = 1 }]
hil-reviews = ["review.toml"]
[[source-contracts]]
id = "att-policy"
composition = "production"
scope = "ATT permission policy"
limits = "One peer"
source-paths = ["driver.rs"]
"#,
    )
    .unwrap()
}
fn requirement() -> HilRequirement {
    HilRequirement {
        scenario: "exchange".into(),
        checks: vec![],
        minimum_repetitions: 1,
    }
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[derive(Deserialize, Serialize)]
struct CapturedFile {
    path: PathBuf,
    size_bytes: u64,
    sha256: String,
    mode: u32,
}
#[derive(Deserialize, Serialize)]
struct CapturedSource {
    name: String,
    commit: String,
    dirty: bool,
    files: Vec<CapturedFile>,
}
#[derive(Deserialize, Serialize)]
struct CapturedManifest {
    schema: u16,
    sources: Vec<CapturedSource>,
}

fn run(fixture: &Fixture, id: &str, outcome: &str, time: u64, application: &[u8]) -> PathBuf {
    let run = fixture.write_run(id, vec![scenario("exchange", &[outcome])]);
    fs::write(run.join("application.bin"), application).unwrap();
    fs::create_dir_all(run.join("scenarios/exchange")).unwrap();
    write(
        &run.join("scenarios/exchange/scenario.json"),
        &json!({"id":"exchange","image":"correctness"}),
    );
    let directory = run.join("source/snapshot");
    fs::create_dir_all(&directory).unwrap();
    let mut archive = tar::Builder::new(fs::File::create(directory.join("sources.tar")).unwrap());
    let mut files = Vec::new();
    for path in [
        "driver.rs",
        "Cargo.lock",
        "ble.rs",
        "phy.rs",
        "observer.rs",
        "hil/schema/observer-inputs.json",
    ] {
        if !fixture.0.join(path).exists() {
            continue;
        }
        let bytes = fs::read(fixture.0.join(path)).unwrap();
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        archive
            .append_data(&mut header, format!("repository/{path}"), bytes.as_slice())
            .unwrap();
        files.push(CapturedFile {
            path: path.into(),
            size_bytes: bytes.len() as u64,
            sha256: digest(&bytes),
            mode: 0o644,
        });
    }
    archive.finish().unwrap();
    let source = CapturedSource {
        name: "repository".into(),
        commit: "ab".repeat(20),
        dirty: true,
        files,
    };
    let workspace = digest(&serde_json::to_vec(&source).unwrap());
    let captured = CapturedManifest {
        schema: 1,
        sources: vec![source],
    };
    fs::write(
        directory.join("manifest.json"),
        serde_json::to_vec_pretty(&captured).unwrap(),
    )
    .unwrap();
    write(
        &directory.join("snapshot.json"),
        &json!({"schema":1,"snapshot_id":digest(&serde_json::to_vec(&captured).unwrap()),"archive_sha256":sha256_file(&directory.join("sources.tar")).unwrap(),"files":captured.sources[0].files.len()}),
    );
    let mut manifest: Value = read_json(&run.join("manifest.json")).unwrap();
    manifest["started_unix_millis"] = json!(time);
    manifest["finished_unix_millis"] = json!(time + 100);
    manifest["repository"] =
        json!({"commit":"ab".repeat(20),"dirty":true,"workspace_sha256":workspace});
    manifest["firmware"][0]["image"] = json!("correctness");
    manifest["firmware"][0]["application_path"] = json!("application.bin");
    manifest["firmware"][0]["application_size_bytes"] = json!(application.len());
    manifest["firmware"][0]["application_sha256"] = json!(digest(application));
    write(&run.join("manifest.json"), &manifest);
    let mut suite: Value = read_json(&run.join("suite.json")).unwrap();
    suite["started_unix_millis"] = json!(time);
    suite["finished_unix_millis"] = json!(time + 100);
    write(&run.join("suite.json"), &suite);
    let mut build: Value = read_json(&run.join("build-provenance.json")).unwrap();
    build["parameters"] = json!({"image":"correctness","runtime_profile":"test","runtime_features":"test","target":"test","network":"upstream-xarxa"});
    build["environment"] = json!({"tools":[
        {"name":"rustc","version":"test"}, {"name":"cargo","version":"test"},
        {"name":"llvm-objcopy","version":"test"}, {"name":"llvm-nm","version":"test"}, {"name":"espflash","version":"test"}],
        "inherited_rustflags":null,"inherited_encoded_rustflags":null,"cargo_incremental":"0","source_date_epoch":null});
    for name in ["embedded-lock", "bootstrap-lock"] {
        fs::write(run.join(format!("{name}.lock")), b"effective lock").unwrap();
        build["files"].as_array_mut().unwrap().push(json!({"name":name,"path":format!("{name}/Cargo.lock"),
            "archive_path":format!("{name}.lock"),"size_bytes":14,"sha256":digest(b"effective lock")}));
    }
    build["subjects"] =
        json!([{"role":"application","size_bytes":application.len(),"sha256":digest(application)}]);
    build["sources"][0]["commit"] = manifest["repository"]["commit"].clone();
    build["sources"][0]["workspace_sha256"] = manifest["repository"]["workspace_sha256"].clone();
    build["sources"][0]["dirty"] = json!(true);
    build["sources"][0]["rebuild_status"] = json!("source-snapshot");
    write(&run.join("build-provenance.json"), &build);
    crate::hil::tests::seal(&run);
    run
}

fn reference(index: &HilEvidenceIndex, run: &str) -> ObservationRef {
    let (scenario, observation) = index
        .scenarios
        .iter()
        .flat_map(|(scenario, records)| records.iter().map(move |record| (scenario, record)))
        .find(|(_, record)| record.run_id == run)
        .unwrap();
    ObservationRef {
        build_record: None,
        id: observation.observation_id(scenario).unwrap(),
        image: "correctness".into(),
        application_sha256: observation.subject.as_ref().unwrap().firmware[0]
            .application
            .as_ref()
            .unwrap()
            .sha256
            .clone(),
    }
}
fn record(fixture: &Fixture, index: &HilEvidenceIndex, catalog: &ScenarioCatalog) -> Document {
    let d = declaration();
    let binding = property(&d, &BTreeMap::new(), &requirement(), catalog, &fixture.0).unwrap();
    Document {
        schema: 1,
        id: "att-transfer".into(),
        capability: d.id,
        scenario: "exchange".into(),
        property_sha256: binding.sha256,
        kind: Kind::UnchangedFunctionalContract,
        reviewer: "test-reviewer".into(),
        reason: "Reviewed unchanged ATT owner and contract on these two builds".into(),
        source: reference(index, "old"),
        observer_provenance: vec![],
        destination: reference(index, "new"),
        inputs: binding.current_inputs,
        dependency_roots: vec![],
        failures: vec![],
    }
}
fn save(fixture: &Fixture, review: &Document) {
    fs::write(
        fixture.0.join("review.toml"),
        toml_edit::ser::to_string_pretty(review).unwrap(),
    )
    .unwrap();
}
fn setup() -> Fixture {
    let fixture = Fixture::new();
    fs::write(fixture.0.join("driver.rs"), "fn permission() {}\n").unwrap();
    run(&fixture, "old", "passed", 100, b"old image");
    run(&fixture, "new", "passed", 200, b"new image");
    fixture
}
fn evaluated(
    fixture: &Fixture,
    index: &HilEvidenceIndex,
    catalog: &ScenarioCatalog,
) -> (EvidenceStatus, Vec<ReviewDecision>) {
    let (reviewed, decisions) =
        apply(&fixture.0, &declaration(), &BTreeMap::new(), index, catalog).unwrap();
    (
        reviewed.decision_for(&requirement(), catalog).status,
        decisions,
    )
}

#[test]
fn explicit_review_promotes_only_bound_property_and_keeps_original_exclusions() {
    let fixture = setup();
    let index = fixture.load().unwrap();
    let catalog = ScenarioCatalog::default();
    assert_eq!(
        index.decision_for(&requirement(), &catalog).status,
        EvidenceStatus::Missing
    );
    save(&fixture, &record(&fixture, &index, &catalog));
    validate(&fixture.0, &declaration(), &catalog).unwrap();
    let mut model_catalog = catalog.clone();
    model_catalog.repetitions.insert("exchange".into(), 1);
    crate::model::tests::assert_reviewed_hil(&fixture.0, declaration(), &index, &model_catalog);
    let (reviewed, decisions) = apply(
        &fixture.0,
        &declaration(),
        &BTreeMap::new(),
        &index,
        &catalog,
    )
    .unwrap();
    assert_eq!(decisions[0].status, "applied");
    assert_eq!(
        reviewed.decision_for(&requirement(), &catalog).status,
        EvidenceStatus::Satisfied
    );
    let original = serde_json::to_value(index.decision_for(&requirement(), &catalog)).unwrap();
    let current = serde_json::to_value(reviewed.decision_for(&requirement(), &catalog)).unwrap();
    let old = |v: &Value| {
        v["observations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["run_id"] == "old")
            .unwrap()
            .clone()
    };
    assert_eq!(old(&original)["exclusions"], old(&current)["exclusions"]);
    assert_eq!(
        old(&original)["observation_id"],
        old(&current)["observation_id"]
    );
    assert_eq!(old(&current)["applicable"], true);
    assert_eq!(
        index.decision_for(&requirement(), &catalog).status,
        EvidenceStatus::Missing
    );
    let mut other = declaration();
    other.id = "other".into();
    other.hil_reviews.clear();
    let (other, _) = apply(&fixture.0, &other, &BTreeMap::new(), &index, &catalog).unwrap();
    assert_eq!(
        other.decision_for(&requirement(), &catalog).status,
        EvidenceStatus::Missing
    );
}

#[test]
fn stale_contract_build_owner_and_property_bindings_never_promote() {
    let fixture = setup();
    let index = fixture.load().unwrap();
    let catalog = ScenarioCatalog::default();
    for (field, status) in [
        ("property", "property-changed"),
        ("image", "build-binding-mismatch"),
        ("owner", "current-input-changed"),
        ("missing-owner", "owner-bindings-incomplete"),
        ("source", "source-observation-missing"),
    ] {
        let mut review = record(&fixture, &index, &catalog);
        match field {
            "property" => review.property_sha256 = "11".repeat(32),
            "image" => review.destination.application_sha256 = "11".repeat(32),
            "owner" => review.inputs[0].sha256 = "11".repeat(32),
            "missing-owner" => {
                review.inputs.pop();
            }
            "source" => review.source.id = "11".repeat(32),
            _ => unreachable!(),
        }
        save(&fixture, &review);
        let (result, decisions) = evaluated(&fixture, &index, &catalog);
        assert_eq!(decisions[0].status, status);
        assert_eq!(result, EvidenceStatus::Missing);
    }
    save(&fixture, &record(&fixture, &index, &catalog));
    fs::write(fixture.0.join("driver.rs"), "changed owner").unwrap();
    assert_eq!(
        evaluated(&fixture, &index, &catalog).1[0].status,
        "current-input-changed"
    );
}

#[test]
fn failures_need_individual_dispositions_and_new_failure_reopens_result() {
    let fixture = setup();
    run(&fixture, "failure", "failed", 150, b"old image");
    let index = fixture.load().unwrap();
    let catalog = ScenarioCatalog::default();
    let mut review = record(&fixture, &index, &catalog);
    save(&fixture, &review);
    assert_eq!(
        evaluated(&fixture, &index, &catalog).0,
        EvidenceStatus::UnresolvedFailure
    );
    review.failures.push(FailureResolution {
        observation: reference(&index, "failure").id,
        disposition: Disposition::Fixed,
        reason: "Fix confirmed by the later complete exchange on the destination application"
            .into(),
        resolving_observation: Some(reference(&index, "new").id),
    });
    save(&fixture, &review);
    assert_eq!(
        evaluated(&fixture, &index, &catalog).0,
        EvidenceStatus::Satisfied
    );
    run(&fixture, "regression", "failed", 300, b"new image");
    let index = fixture.load().unwrap();
    assert_eq!(
        evaluated(&fixture, &index, &catalog).0,
        EvidenceStatus::UnresolvedFailure
    );
    review.failures.push(FailureResolution {
        observation: reference(&index, "regression").id,
        disposition: Disposition::Fixed,
        reason: "Invalid attempt to close new regression with an older PASS".into(),
        resolving_observation: Some(reference(&index, "new").id),
    });
    save(&fixture, &review);
    assert_eq!(
        evaluated(&fixture, &index, &catalog).1[0].status,
        "failure-resolution-not-established"
    );
}

#[test]
fn source_snapshot_must_establish_the_reviewed_owner_bytes() {
    let fixture = setup();
    fs::write(fixture.0.join("driver.rs"), "new driver contract").unwrap();
    run(&fixture, "new", "passed", 200, b"new image");
    let index = fixture.load().unwrap();
    let catalog = ScenarioCatalog::default();
    save(&fixture, &record(&fixture, &index, &catalog));
    assert_eq!(
        evaluated(&fixture, &index, &catalog).1[0].status,
        "source-contract-changed-or-unavailable"
    );
}

#[test]
fn identical_image_policy_requires_the_actual_application_bytes() {
    let fixture = setup();
    let index = fixture.load().unwrap();
    let catalog = ScenarioCatalog::default();
    let mut review = record(&fixture, &index, &catalog);
    review.kind = Kind::IdenticalImage;
    save(&fixture, &review);
    assert_eq!(
        evaluated(&fixture, &index, &catalog).1[0].status,
        "identical-image-required"
    );
}

#[test]
fn missing_control_cannot_be_spliced_from_another_invocation() {
    let fixture = setup();
    let index = fixture.load().unwrap();
    let mut catalog = ScenarioCatalog::default();
    catalog.controls.insert("exchange".into(), "control".into());
    let mut decision = index.decision_for(&requirement(), &catalog);
    decision
        .attach_reviews(&fixture.0, &declaration(), &BTreeMap::new(), &catalog, &[])
        .unwrap();
    let json = serde_json::to_value(decision).unwrap();
    assert!(json["control"]["property"]["sha256"].is_string());
    assert_ne!(
        json["control"]["property"]["sha256"],
        json["property"]["sha256"]
    );

    save(&fixture, &record(&fixture, &index, &catalog));
    assert_eq!(
        evaluated(&fixture, &index, &catalog).1[0].status,
        "source-control-not-passed"
    );
}

mod guards;

#[test]
fn identical_snapshot_is_direct_evidence_regardless_of_commit_or_dirty_state() {
    for change in [
        "none",
        "dirty",
        "included-untracked",
        "extra-untracked",
        "current-bytes",
        "extra-current-file",
        "missing-current-file",
        "archive",
        "source-identity",
        "application-binding",
    ] {
        let fixture = Fixture::new();
        fs::write(fixture.0.join("driver.rs"), "fn permission() {}\n").unwrap();
        if change == "included-untracked" {
            fs::write(fixture.0.join("ble.rs"), "captured untracked input").unwrap();
        }
        let dirty = matches!(change, "dirty" | "included-untracked");
        let commit = if dirty {
            "ab".repeat(20)
        } else {
            "current".into()
        };
        let run = run(&fixture, "fresh", "passed", 100, b"fresh firmware");
        let directory = run.join("source/snapshot");
        let mut captured: CapturedManifest =
            serde_json::from_slice(&fs::read(directory.join("manifest.json")).unwrap()).unwrap();
        captured.sources[0].dirty = dirty;
        captured.sources[0].commit = commit.clone();
        let workspace = digest(&serde_json::to_vec(&captured.sources[0]).unwrap());
        fs::write(
            directory.join("manifest.json"),
            serde_json::to_vec(&captured).unwrap(),
        )
        .unwrap();
        let mut snapshot: Value = read_json(&directory.join("snapshot.json")).unwrap();
        snapshot["snapshot_id"] = json!(digest(&serde_json::to_vec(&captured).unwrap()));
        write(&directory.join("snapshot.json"), &snapshot);
        let mut manifest: Value = read_json(&run.join("manifest.json")).unwrap();
        manifest["repository"] =
            json!({"commit":commit,"dirty":dirty,"workspace_sha256":workspace});
        write(&run.join("manifest.json"), &manifest);
        let mut build: Value = read_json(&run.join("build-provenance.json")).unwrap();
        build["sources"][0]["commit"] = json!(commit);
        build["sources"][0]["dirty"] = json!(dirty);
        build["sources"][0]["workspace_sha256"] = json!(workspace);
        write(&run.join("build-provenance.json"), &build);
        for args in [vec!["init", "-q"], vec!["add", "driver.rs", "Cargo.lock"]] {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(&fixture.0)
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        fs::write(fixture.0.join(".git/info/exclude"), "runs/\n").unwrap();
        match change {
            "current-bytes" => fs::write(fixture.0.join("driver.rs"), "changed").unwrap(),
            "extra-current-file" => {
                fs::write(fixture.0.join("added.rs"), "added").unwrap();
                assert!(
                    Command::new("git")
                        .arg("-C")
                        .arg(&fixture.0)
                        .args(["add", "added.rs"])
                        .status()
                        .unwrap()
                        .success()
                );
            }
            "missing-current-file" => fs::remove_file(fixture.0.join("driver.rs")).unwrap(),
            "archive" => fs::write(directory.join("sources.tar"), b"invalid archive").unwrap(),
            "source-identity" => {
                build["sources"][0]["workspace_sha256"] = json!("00".repeat(32));
                write(&run.join("build-provenance.json"), &build);
            }
            "application-binding" => {
                build["subjects"][0]["sha256"] = json!("00".repeat(32));
                write(&run.join("build-provenance.json"), &build);
            }
            "extra-untracked" => fs::write(fixture.0.join("extra.rs"), "not captured").unwrap(),
            "none" | "dirty" | "included-untracked" => {}
            _ => unreachable!(),
        }
        // Reseal to exercise source binding, not only the outer integrity seal.
        crate::hil::tests::seal(&run);
        let index = HilEvidenceIndex::load(
            &fixture.0,
            Path::new("runs"),
            "esp32s31",
            &crate::hil::RepositoryState {
                commit: "current".into(),
                dirty,
            },
        )
        .unwrap();
        let decision = index.decision_for(&requirement(), &ScenarioCatalog::default());
        assert_eq!(
            decision.status == EvidenceStatus::Satisfied,
            matches!(change, "none" | "dirty" | "included-untracked"),
            "{change}"
        );
    }
}
