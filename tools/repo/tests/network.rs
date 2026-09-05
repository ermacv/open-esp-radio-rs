mod support;
use oer_xtask::{
    cargo,
    checks::network::{self, Boundary},
    graph::Graph,
    process,
};
use serde_json::{Value, json};
use std::fs;
use support::Fixture;

fn audit(f: &Fixture, value: Value, boundary: Boundary) -> oer_xtask::Result<()> {
    network::audit(&Graph::from_value(value)?, &f.manifest, boundary, f.root())
}
fn root_id(f: &Fixture, value: &Value) -> String {
    value["packages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["manifest_path"].as_str() == f.manifest.to_str())
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned()
}
fn node<'a>(value: &'a mut Value, id: &str) -> &'a mut Value {
    value["resolve"]["nodes"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|node| node["id"] == id)
        .unwrap()
}

#[test]
fn renamed_transitive_build_dependency_cannot_hide_chip_ownership() {
    let f = Fixture::new();
    f.package(
        "adapter",
        "network-adapter-fixture",
        "[dependencies]\nqueue = { package = \"packet-helper\", path = \"../helper\" }\n",
    );
    f.package("helper","packet-helper","[build-dependencies]\ngenerator = { package = \"device-registers\", path = \"../driver/chips/test-radio\" }\n");
    assert!(
        audit(&f, f.metadata(), Boundary::Owned)
            .unwrap_err()
            .to_string()
            .contains("network-adapter-fixture -> packet-helper -> device-registers")
    );
}
#[test]
fn dev_only_dependency_does_not_confer_production_ownership() {
    let f = Fixture::new();
    f.package("adapter","network-adapter-fixture","[dev-dependencies]\nfixture = { package = \"device-registers\", path = \"../driver/chips/test-radio\" }\n");
    let data = f.metadata();
    assert!(
        data["packages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["name"] == "device-registers")
    );
    audit(&f, data, Boundary::Owned).unwrap();
}
#[test]
fn normal_edge_stays_forbidden_when_also_dev() {
    let f = Fixture::new();
    let dep =
        "fixture = { package = \"device-registers\", path = \"../driver/chips/test-radio\" }\n";
    f.package(
        "adapter",
        "network-adapter-fixture",
        &format!("[dependencies]\n{dep}[dev-dependencies]\n{dep}"),
    );
    assert!(audit(&f, f.metadata(), Boundary::Owned).is_err());
}
#[test]
fn unreachable_workspace_member_is_not_a_production_dependency() {
    let f = Fixture::new();
    audit(&f, f.metadata(), Boundary::Owned).unwrap();
}
#[test]
fn malformed_resolve_fails_closed() {
    let f = Fixture::new();
    let valid = f.metadata();
    let id = root_id(&f, &valid);
    for case in 0..7 {
        let mut data = valid.clone();
        match case {
            0 => {
                data.as_object_mut().unwrap().remove("resolve");
            }
            1 => data["resolve"] = Value::Null,
            2 => data["resolve"]["nodes"] = json!([]),
            3 => {
                let duplicate = data["packages"][0].clone();
                data["packages"].as_array_mut().unwrap().push(duplicate);
            }
            4 => {
                let duplicate = data["resolve"]["nodes"][0].clone();
                data["resolve"]["nodes"]
                    .as_array_mut()
                    .unwrap()
                    .push(duplicate);
            }
            5 => {
                node(&mut data, &id).as_object_mut().unwrap().remove("deps");
            }
            _ => {
                node(&mut data, &id)
                    .as_object_mut()
                    .unwrap()
                    .remove("features");
            }
        }
        assert!(
            audit(&f, data, Boundary::Owned).is_err(),
            "accepted mutation {case}"
        );
    }
}
#[test]
fn missing_edge_metadata_cannot_erase_forbidden_dependency() {
    let f = Fixture::new();
    f.package("adapter","network-adapter-fixture","[build-dependencies]\nfixture = { package = \"device-registers\", path = \"../driver/chips/test-radio\" }\n");
    let valid = f.metadata();
    let id = root_id(&f, &valid);
    for case in 0..4 {
        let mut data = valid.clone();
        let edge = &mut node(&mut data, &id)["deps"][0];
        match case {
            0 => {
                edge.as_object_mut().unwrap().remove("dep_kinds");
            }
            1 => edge["dep_kinds"] = json!([]),
            2 => edge["dep_kinds"] = json!([{"kind":"unknown","target":null}]),
            _ => edge["pkg"] = json!("absent"),
        };
        assert!(audit(&f, data, Boundary::Owned).is_err());
    }
}
#[test]
fn cyclic_graph_terminates_and_rejects_reachable_chip() {
    let f = Fixture::new();
    f.package(
        "adapter",
        "network-adapter-fixture",
        "[dependencies]\nhelper = { package = \"packet-helper\", path = \"../helper\" }\n",
    );
    f.package("helper","packet-helper","[build-dependencies]\nfixture = { package = \"device-registers\", path = \"../driver/chips/test-radio\" }\n");
    let mut data = f.metadata();
    let root = root_id(&f, &data);
    let chip = data["packages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "device-registers")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    node(&mut data, &chip)["dependencies"]
        .as_array_mut()
        .unwrap()
        .push(json!(root));
    node(&mut data, &chip)["deps"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name":"loop","pkg":root,"dep_kinds":[{"kind":null,"target":null}]}));
    assert!(
        audit(&f, data, Boundary::Owned)
            .unwrap_err()
            .to_string()
            .contains("physical radio ownership")
    );
}
#[test]
fn disabled_optional_declaration_still_violates_leaf_boundary() {
    let f = Fixture::new();
    f.package("adapter","network-adapter-fixture","[dependencies]\nfixture = { package = \"device-registers\", path = \"../driver/chips/test-radio\", optional = true }\n");
    let data = f.metadata();
    let graph = Graph::from_value(data.clone()).unwrap();
    assert_eq!(graph.reachable(&graph.root(&f.manifest).unwrap()).len(), 1);
    for boundary in [Boundary::Neutral, Boundary::Compat, Boundary::Owned] {
        assert!(
            audit(&f, data.clone(), boundary)
                .unwrap_err()
                .to_string()
                .contains("forbidden declared production dependency")
        );
    }
}
#[test]
fn isolated_consumer_does_not_unify_unrelated_member_features() {
    let f = Fixture::new();
    f.write("Cargo.toml","[workspace]\nmembers = [\"adapter\",\"helper\",\"consumer\",\"driver/chips/test-radio\"]\nresolver = \"3\"\n");
    f.package("adapter","network-adapter-fixture","[dependencies]\nhelper = { package = \"packet-helper\", path = \"../helper\", default-features = false }\n[features]\ndefault = [\"plain\"]\nplain = []\n");
    f.package("helper","packet-helper","[dependencies]\nchip = { package = \"device-registers\", path = \"../driver/chips/test-radio\", optional = true }\n[features]\nhardware = [\"dep:chip\"]\n");
    f.package("consumer","unrelated-consumer","[dependencies]\nhelper = { package = \"packet-helper\", path = \"../helper\", features = [\"hardware\"] }\n");
    f.metadata();
    let before = fs::read(f.root().join("Cargo.lock")).unwrap();
    let graph = cargo::isolated_graph(&f.context, &f.manifest, &[], None).unwrap();
    assert!(
        graph
            .node(&graph.root(&f.manifest).unwrap())
            .features
            .iter()
            .any(|v| v.as_str() == "plain")
    );
    network::audit(&graph, &f.manifest, Boundary::Owned, f.root()).unwrap();
    assert_eq!(before, fs::read(f.root().join("Cargo.lock")).unwrap());
    f.package("adapter","network-adapter-fixture","[dependencies]\nhelper = { package = \"packet-helper\", path = \"../helper\", features = [\"hardware\"] }\n");
    let graph = cargo::isolated_graph(&f.context, &f.manifest, &[], None).unwrap();
    assert!(network::audit(&graph, &f.manifest, Boundary::Owned, f.root()).is_err());
}
#[test]
fn relative_patch_preserves_chip_identity() {
    let f = Fixture::new();
    f.package(
        "adapter",
        "network-adapter-fixture",
        "[dependencies]\napi = { package = \"device-registers\", version = \"0.1\" }\n",
    );
    let mut text = fs::read_to_string(f.root().join("Cargo.toml")).unwrap();
    text.push_str("[patch.crates-io]\ndevice-registers = { path = \"driver/chips/test-radio\" }\n");
    f.write("Cargo.toml", &text);
    f.metadata();
    let graph = cargo::isolated_graph(&f.context, &f.manifest, &[], None).unwrap();
    assert!(network::audit(&graph, &f.manifest, Boundary::Owned, f.root()).is_err());
}
#[test]
fn unused_patch_is_removed_only_from_temporary_consumer() {
    let f = Fixture::new();
    f.package("unused-override", "unused-override", "");
    let mut text = fs::read_to_string(f.root().join("Cargo.toml")).unwrap();
    text.push_str("[patch.\"https://example.invalid/unused\"]\nunused-override = { path = \"unused-override\" }\n");
    f.write("Cargo.toml", &text);
    f.metadata();
    let before = fs::read(f.root().join("Cargo.lock")).unwrap();
    let graph = cargo::isolated_graph(&f.context, &f.manifest, &[], None).unwrap();
    network::audit(&graph, &f.manifest, Boundary::Owned, f.root()).unwrap();
    assert_eq!(
        text,
        fs::read_to_string(f.root().join("Cargo.toml")).unwrap()
    );
    assert_eq!(before, fs::read(f.root().join("Cargo.lock")).unwrap());
}
#[test]
fn isolated_resolution_rejects_origin_pin_drift() {
    let f = Fixture::new();
    f.package(
        "adapter",
        "network-adapter-fixture",
        "[dependencies]\nhelper = { package = \"packet-helper\", path = \"../helper\" }\n",
    );
    f.metadata();
    let before = fs::read(f.root().join("Cargo.lock")).unwrap();
    let path = f.root().join("helper/Cargo.toml");
    fs::write(
        &path,
        fs::read_to_string(&path).unwrap().replace("0.1.0", "0.2.0"),
    )
    .unwrap();
    assert!(
        cargo::isolated_graph(&f.context, &f.manifest, &[], None)
            .unwrap_err()
            .to_string()
            .contains("drifted from origin lock")
    );
    assert_eq!(before, fs::read(f.root().join("Cargo.lock")).unwrap());
}
#[test]
fn released_driver_requires_registry_source_and_version() {
    let f = Fixture::new();
    f.package(
        "adapter",
        "network-adapter-fixture",
        "[dependencies]\napi = { package = \"packet-helper\", path = \"../helper\" }\n",
    );
    let mut data = f.metadata();
    let id = root_id(&f, &data);
    let registry = "registry+https://github.com/rust-lang/crates.io-index";
    let p = data["packages"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|p| p["name"] == "packet-helper")
        .unwrap();
    p["name"] = json!("embassy-net-driver");
    p["version"] = json!("0.2.0");
    p["source"] = json!(registry);
    let declaration = &mut data["packages"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|p| p["id"] == id)
        .unwrap()["dependencies"][0];
    declaration["name"] = json!("embassy-net-driver");
    declaration["source"] = json!(registry);
    audit(&f, data.clone(), Boundary::Compat).unwrap();
    for (source, version) in [
        ("git+https://example.invalid/driver#fork", "0.2.0"),
        (registry, "0.3.0"),
    ] {
        let mut changed = data.clone();
        let p = changed["packages"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|p| p["name"] == "embassy-net-driver")
            .unwrap();
        p["source"] = json!(source);
        p["version"] = json!(version);
        assert!(audit(&f, changed, Boundary::Compat).is_err());
    }
}
#[test]
fn malformed_cargo_output_fails_at_the_executable_boundary() {
    let f = Fixture::new();
    let executable = f
        .root()
        .join(format!("cargo-stub{}", std::env::consts::EXE_SUFFIX));
    process::run(
        f.context
            .command("rustc")
            .arg(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/fixtures/malformed_cargo.rs"),
            )
            .args(["--edition", "2024", "--crate-name", "malformed_cargo", "-o"])
            .arg(&executable),
    )
    .unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_oer-xtask"))
        .args(["check", "network", "--dependencies-only"])
        .env("CARGO", &executable)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("workspace root missing"));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("boundaries are clean"));
}
