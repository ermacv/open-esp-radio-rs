//! Cross-process producer/evaluator/planner contract; firmware and observations
//! are synthetic host inputs, never hardware qualification.
use super::*;
use crate::{
    campaign::Plan,
    image::{Artifacts, Integration},
    scenario::Catalog,
};
use serde_json::{Value, json};
use std::process::Command;

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
fn write(root: &Path, path: &str, text: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}
fn snapshot(root: &Path) -> crate::image::snapshot::Snapshot {
    crate::image::snapshot::test_capture(root)
}
fn map(root: &Path) -> Value {
    let output = Command::new("cargo")
        .current_dir(root)
        .args([
            "qualification",
            "status",
            "--manifest",
            "program.toml",
            "--json-report",
            "target/map.json",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&fs::read(root.join("target/map.json")).unwrap()).unwrap()
}
fn find_decision(value: &Value) -> Option<&Value> {
    if value.get("scenario") == Some(&json!("station-ap-loss")) && value.get("property").is_some() {
        return Some(value);
    }
    match value {
        Value::Object(m) => m.values().find_map(find_decision),
        Value::Array(a) => a.iter().find_map(find_decision),
        _ => None,
    }
}
#[test]
fn producer_evaluator_and_resumed_plan_transfer_wifi_across_ble_but_reject_phy_change() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    write_test_build_materials(root);
    write(
        root,
        "hil/schema/observer-inputs.json",
        r#"{"schema":3,"data":[],"timing":{"station-ap-loss":false,"boot-smoke":false},"workload_domains":{"station-ap-loss":"common","boot-smoke":"common"},"dependencies":{"common":[]},"build":{"profile":"debug","opt_level":"0","debug":"true"}}"#,
    );
    write(
        root,
        "hil/host/runner/src/session/reboot.rs",
        include_str!("../../../session/reboot.rs"),
    );
    write(root, "hil/targets/esp32s31/Cargo.toml", "[workspace]\n");
    write(
        root,
        "Cargo.toml",
        "[workspace]\nresolver = '3'\nmembers = [\"wifi\",\"ble\",\"hil/host/runner\"]\n",
    );
    write(
        root,
        "wifi/Cargo.toml",
        "[package]\nname = \"wifi\"\nversion = \"0.1.0\"\n",
    );
    write(
        root,
        "ble/Cargo.toml",
        "[package]\nname = \"ble\"\nversion = \"0.1.0\"\n",
    );
    let lock = "version = 4\n[[package]]\nname = \"wifi\"\nversion = \"0.1.0\"\n[[package]]\nname = \"ble\"\nversion = \"0.1.0\"\n";
    write(
        root,
        "hil/host/runner/Cargo.toml",
        "[package]\nname = 'oer-hil-runner'\nversion = '0.1.0'\nedition = '2024'\n",
    );
    let registry: Value =
        serde_json::from_slice(&fs::read(root.join("hil/schema/observer-inputs.json")).unwrap())
            .unwrap();
    let configuration = oer_hil_schema::observer::required_configuration(root, &registry).unwrap();
    write(
        root,
        "hil/host/runner/src/main.rs",
        &format!(
            "fn main() {{ println!(\"{{}}\", {:?}); }}\n",
            serde_json::to_string(&json!({"compiler":configuration["compiler"],"environment":configuration["environment"]})).unwrap()
        ),
    );
    let host_lock = format!("{lock}\n[[package]]\nname = 'oer-hil-runner'\nversion = '0.1.0'\n");
    write(root, "Cargo.lock", &host_lock);
    write(root, "hil/targets/esp32s31/Cargo.lock", lock);
    write(root, ".gitignore", "target/\n");
    write(root, "wifi/src/lib.rs", "fn wifi() {}\n");
    write(root, "phy.rs", "fn phy() {}\n");
    write(root, "ble/src/lib.rs", "fn ble() {}\n");
    let repository = crate::repository_root().unwrap();
    let alias = format!(
        "[alias]\nqualification = [\"run\",\"--quiet\",\"--manifest-path\",{:?},\"-p\",\"oer-qualification\",\"--\"]\n",
        repository.join("Cargo.toml").to_str().unwrap()
    );
    write(root, ".cargo/config.toml", &alias);
    write(
        root,
        "scenarios/station-ap-loss.toml",
        include_str!("../../../../../../scenarios/ieee80211/station/station-ap-loss.toml"),
    );
    write(
        root,
        "program.toml",
        r#"schema = 4
target = "wifi-test"
required-capabilities = ["wifi", "base"]
[verification]
evidence-index = "target/vendor.json"
[hil]
target = "esp32s31"
catalog = "scenarios"
runs = "target/hil/esp32s31/runs"
[[capabilities]]
id = "wifi"
title = "Wi-Fi fixture"
depends-on = ["base"]
scope = "Host contract regression only"
implementation = "complete"
host = "covered"
async = "bounded"
vendor-not-applicable = "host-contract-test"
hil-requirements = [{ scenario = "station-ap-loss", minimum-repetitions = 3 }]
[[capabilities.source-contracts]]
id = "wifi-owner"
composition = "production"
scope = "Wi-Fi and shared PHY owners"
limits = "Synthetic firmware, no hardware claim"
source-paths = ["wifi/src/lib.rs", "phy.rs"]
[[capabilities]]
id = "base"
title = "Independent boot requirement"
scope = "Dependency context, not implicit execution"
implementation = "complete"
host = "covered"
async = "bounded"
vendor-not-applicable = "host-contract-test"
hil-requirements = [{ scenario = "boot-smoke", minimum-repetitions = 1 }]
[[capabilities.source-contracts]]
id = "phy-owner"
composition = "production"
scope = "Shared PHY"
limits = "Host contract fixture"
source-paths = ["phy.rs"]
"#,
    );
    write(
        root,
        "scenarios/boot-smoke.toml",
        include_str!("../../../../../../scenarios/system/boot-smoke.toml"),
    );
    git(root, &["init", "-q"]);
    git(root, &["config", "user.name", "Workflow Test"]);
    git(root, &["config", "user.email", "workflow@example.invalid"]);
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "fixture"]);
    // Fresh dirty snapshot is the exact subject, and needs no self-review.
    write(root, "ble/src/lib.rs", "fn ble_a() {}\n");
    let captured = snapshot(root);
    let catalog = Catalog::load(&root.join("scenarios")).unwrap();
    let pending = Plan::from_qualification(
        root,
        &catalog,
        "program.toml".into(),
        Some("wifi".into()),
        Integration::UpstreamXarxa,
    )
    .unwrap();
    assert_eq!(pending.resolve(&catalog).unwrap().0.len(), 1);
    let scenario = catalog.get("station-ap-loss").unwrap();
    let output = root.join("target/artifacts");
    fs::create_dir_all(&output).unwrap();
    for name in [
        "application.bin",
        "runtime.elf",
        "runtime.bin",
        "bootstrap.elf",
    ] {
        fs::write(output.join(name), format!("synthetic {name} A")).unwrap();
    }
    write(root, "target/bootstrap.lock", "unchanged bootstrap lock");
    let artifacts = Artifacts {
        network: Integration::UpstreamXarxa,
        output: output.clone(),
        application_image: output.join("application.bin"),
        runtime_elf: output.join("runtime.elf"),
        runtime_bin: output.join("runtime.bin"),
        bootstrap_elf: output.join("bootstrap.elf"),
        effective_embedded_lock: root.join("hil/targets/esp32s31/Cargo.lock"),
        effective_bootstrap_lock: root.join("target/bootstrap.lock"),
        environment: crate::evidence::build::BuildEnvironment::synthetic(),
    };
    let directory = root.join("target/hil/esp32s31/runs/observed-a");
    fs::create_dir_all(&directory).unwrap();
    let mut run = session(&directory);
    run.manifest.runner = runner_provenance().unwrap();
    // The firmware and observer in this cross-process fixture are synthetic.
    // Keep the required compiler environment, with the fixture's Cargo composition.
    let observer = run.manifest.runner.observer.as_mut().unwrap();
    observer["build"] = json!({
        "schema": 2,
        // The runner package's sources are always observer inputs.
        "inputs": {
            "hil/host/runner/src/main.rs":
                sha256_file(&root.join("hil/host/runner/src/main.rs")).unwrap(),
            "hil/host/runner/src/session/reboot.rs":
                sha256_file(&root.join("hil/host/runner/src/session/reboot.rs")).unwrap(),
        },
        "compiler": configuration["compiler"],
        "environment": configuration["environment"],
    });
    let manifests = ["Cargo.toml", "hil/host/runner/Cargo.toml"]
        .into_iter()
        .map(|path| {
            (
                path,
                toml::from_str::<Value>(&fs::read_to_string(root.join(path)).unwrap()).unwrap(),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    observer["build"]["resolved"] = json!({"nodes":[{"depth":0,"package":{"name":"oer-hil-runner","version":"0.1.0"},"features":[]}],"manifests":manifests,"cargo_config":{}});
    let compilation = oer_hil_schema::compile::compile(root).unwrap();
    super::super::observer_artifacts::apply(
        &mut observer["build"]["resolved"],
        &compilation.artifacts,
    )
    .unwrap();
    observer["build"]["resolved"]["selected_profile"] = json!(compilation.profile);
    observer["build_sha256"] = json!(format!(
        "{:x}",
        <sha2::Sha256 as sha2::Digest>::digest(serde_json::to_vec(&observer["build"]).unwrap())
    ));
    // Evaluation reads an explicitly prepared observer receipt; it must not
    // compile or execute the observer as a side effect of a status query.
    write(
        root,
        "target/hil/current-observer.json",
        &serde_json::to_string(&json!({"build": observer["build"]})).unwrap(),
    );
    drop(compilation);
    run.repository_root = root.into();
    run.target_directory = root.join("target/hil/esp32s31");
    run.bind_source_snapshot(captured.directory()).unwrap();
    run.record_firmware(scenario.image, &artifacts).unwrap();
    let result = ScenarioResult::from_repetitions(
        scenario.id.clone(),
        scenario.image,
        scenario.repetitions,
        (1..=scenario.repetitions)
            .map(|repetition| RepetitionResult {
                schema: RUN_SCHEMA,
                repetition,
                outcome: Outcome::Passed,
                started_unix_millis: 1,
                duration_millis: 1,
                artifact_directory: PathBuf::from(format!(
                    "scenarios/{}/repetition-{repetition:03}",
                    scenario.id
                )),
                attachments: vec![],
                measurements: vec![],
                failure: None,
            })
            .collect(),
    );
    run.seal_scenario(scenario, &result).unwrap();
    // The enclosing campaign is still open: its independent seal is sufficient.
    let refreshed = pending.refresh(root, &catalog).unwrap();
    assert!(refreshed.resolve(&catalog).unwrap().0.is_empty());
    let report = map(root);
    let decision = find_decision(&report).unwrap();
    assert_eq!(decision["status"], "satisfied", "{decision:#}");
    let observed = &decision["observations"][0];
    let source_hash = run.manifest.firmware[0].application_sha256.clone();
    let mut inputs = decision["property"]["current_inputs"]
        .as_array()
        .unwrap()
        .clone();
    inputs.retain(|input| !matches!(input["path"].as_str(), Some("Cargo.toml" | "Cargo.lock")));
    write(root, "ble/src/lib.rs", "fn ble_b() {}\n");
    let changed = pending.refresh(root, &catalog).unwrap();
    let changed_json = serde_json::to_value(&changed).unwrap();
    assert_eq!(
        changed_json["qualification"]["selection"]["obligations"][0]["action"],
        "review"
    );
    write(
        root,
        "ble/Cargo.toml",
        "[package]\nname = \"ble\"\nversion = \"0.2.0\"\n",
    );
    let lock_b = lock.replace(
        "name = \"ble\"\nversion = \"0.1.0\"",
        "name = \"ble\"\nversion = \"0.2.0\"",
    );
    write(
        root,
        "Cargo.lock",
        &host_lock.replace(
            "name = \"ble\"\nversion = \"0.1.0\"",
            "name = \"ble\"\nversion = \"0.2.0\"",
        ),
    );
    write(root, "hil/targets/esp32s31/Cargo.lock", &lock_b);
    let captured_b = snapshot(root);
    fs::write(
        &artifacts.application_image,
        b"synthetic firmware B with BLE change",
    )
    .unwrap();
    let build = crate::evidence::build_record::publish(
        root,
        captured_b.directory(),
        scenario.image,
        &artifacts,
    )
    .unwrap();
    assert!(!build.join("suite.json").exists());
    assert_eq!(
        crate::evidence::build_record::publish(
            root,
            captured_b.directory(),
            scenario.image,
            &artifacts
        )
        .unwrap(),
        build
    );
    let build_id = sha256_file(&build.join("integrity.json")).unwrap();
    let review = json!({"schema":1,"id":"wifi-a-to-b","capability":"wifi","scenario":scenario.id,"property-sha256":decision["property"]["sha256"],"kind":"unchanged-functional-contract","reviewer":"host-contract-test","reason":"BLE-only source change; Wi-Fi and PHY owners unchanged in A and B",
        "source":{"id":observed["observation_id"],"image":scenario.image.id(),"application-sha256":source_hash},
        "destination":{"id":build_id,"build-record":build.strip_prefix(root).unwrap(),"image":scenario.image.id(),"application-sha256":sha256_file(&artifacts.application_image).unwrap()},"inputs":inputs,"dependency-roots":["wifi"],"failures":[]});
    write(
        root,
        "target/review.toml",
        &toml::to_string_pretty(&review).unwrap(),
    );
    let program = fs::read_to_string(root.join("program.toml"))
        .unwrap()
        .replacen(
            "[[capabilities.source-contracts]]",
            "hil-reviews = [\"target/review.toml\"]\n[[capabilities.source-contracts]]",
            1,
        );
    write(root, "program.toml", &program);
    let continued = pending.refresh(root, &catalog).unwrap();
    let continued_json = serde_json::to_value(&continued).unwrap();
    assert_eq!(
        continued_json["qualification"]["selection"]["obligations"][0]["action"],
        "satisfied",
        "{}",
        map(root)
    );
    assert!(continued.resolve(&catalog).unwrap().0.is_empty());
    assert!(
        continued
            .refresh(root, &catalog)
            .unwrap()
            .resolve(&catalog)
            .unwrap()
            .0
            .is_empty()
    );
    assert_eq!(
        fs::read_dir(root.join("target/hil/esp32s31/runs"))
            .unwrap()
            .count(),
        1
    );
    write(root, "phy.rs", "fn changed_phy_ownership() {}\n");
    let reconsidered = pending.refresh(root, &catalog).unwrap();
    let reconsidered = serde_json::to_value(reconsidered).unwrap();
    assert_eq!(
        reconsidered["qualification"]["selection"]["obligations"][0]["action"],
        "review"
    );
}
