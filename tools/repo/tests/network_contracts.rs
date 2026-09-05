mod support;

use oer_xtask::{
    checks::network::{self, Boundary},
    graph::Graph,
};
use serde_json::{Value, json};
use support::Fixture;

const REGISTRY: &str = "registry+https://github.com/rust-lang/crates.io-index";
const EMBASSY: &str = "git+https://example.invalid/embassy?rev=1111111111111111111111111111111111111111#1111111111111111111111111111111111111111";
const PLATFORM: &str = "git+https://example.invalid/esp-hal?rev=2222222222222222222222222222222222222222#2222222222222222222222222222222222222222";

fn audit(fixture: &Fixture, metadata: Value, boundary: Boundary) -> oer_xtask::Result<()> {
    network::audit(
        &Graph::from_value(metadata)?,
        &fixture.manifest,
        boundary,
        fixture.root(),
    )
}

// Retain Cargo's real alias/edge/feature topology while selecting provenance
// explicitly: these tests never depend on a remote fork or registry server.
fn source(metadata: &mut Value, name: &str, source: &str) {
    let package = metadata["packages"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|package| package["name"] == name)
        .unwrap();
    package["source"] = json!(source);
}

fn product() -> Fixture {
    let fixture = Fixture::new();
    fixture.package("adapter", "network-adapter-fixture", r#"
[dependencies]
api = { package = "embassy-net", path = "../stack" }
device = { package = "embassy-net-driver", path = "../helper" }
platform = { package = "esp-hal", path = "../driver/chips/test-radio" }
compat = { package = "open-esp-radio-embassy-net-compat", path = "../compat", optional = true }
bridge = { package = "open-esp-radio-esp32s31-wifi-embassy-compat", path = "../bridge", optional = true }
owned = { package = "open-esp-radio-embassy-net", path = "../owned", optional = true }
[features]
default = ["compat-network"]
compat-network = ["dep:compat", "dep:bridge"]
owned-network = ["dep:owned"]
"#);
    fixture.package("helper", "embassy-net-driver", "");
    fixture.write(
        "helper/Cargo.toml",
        "[package]\nname = \"embassy-net-driver\"\nversion = \"0.2.0\"\nedition = \"2024\"\n",
    );
    fixture.package("stack", "embassy-net", "");
    fixture.package("driver/chips/test-radio", "esp-hal", "");
    fixture.package("compat", "open-esp-radio-embassy-net-compat", "");
    fixture.package("bridge", "open-esp-radio-esp32s31-wifi-embassy-compat", "");
    fixture.package("owned", "open-esp-radio-embassy-net", "");
    fixture
}

#[test]
fn compat_product_accepts_platform_forks_but_not_network_source_substitution() {
    let fixture = product();
    let mut metadata = fixture.metadata();
    source(&mut metadata, "embassy-net", REGISTRY);
    source(&mut metadata, "embassy-net-driver", REGISTRY);
    source(&mut metadata, "esp-hal", PLATFORM);
    audit(&fixture, metadata.clone(), Boundary::CompatProduct).unwrap();
    for name in ["embassy-net", "embassy-net-driver"] {
        for replacement in [EMBASSY, "registry+https://example.invalid/private-index"] {
            let mut changed = metadata.clone();
            source(&mut changed, name, replacement);
            let error = audit(&fixture, changed, Boundary::CompatProduct)
                .unwrap_err()
                .to_string();
            assert!(error.contains(name), "{error}");
        }
    }
}

#[test]
fn owned_product_requires_one_pinned_embassy_contract_but_allows_platform_forks() {
    let fixture = product();
    let manifest = std::fs::read_to_string(&fixture.manifest).unwrap().replace(
        "default = [\"compat-network\"]",
        "default = [\"owned-network\"]",
    );
    std::fs::write(&fixture.manifest, manifest).unwrap();
    let mut metadata = fixture.metadata();
    source(&mut metadata, "embassy-net", EMBASSY);
    source(&mut metadata, "embassy-net-driver", EMBASSY);
    source(&mut metadata, "esp-hal", PLATFORM);
    audit(&fixture, metadata.clone(), Boundary::OwnedProduct).unwrap();
    for replacement in [
        REGISTRY,
        "git+https://example.invalid/embassy?branch=owned#1111111111111111111111111111111111111111",
        "git+https://example.invalid/embassy?rev=1111111#1111111111111111111111111111111111111111",
        "git+https://example.invalid/embassy?rev=2222222222222222222222222222222222222222#2222222222222222222222222222222222222222",
    ] {
        let mut changed = metadata.clone();
        source(&mut changed, "embassy-net-driver", replacement);
        assert!(audit(&fixture, changed, Boundary::OwnedProduct).is_err());
    }
}

#[test]
fn research_rejects_optional_and_renamed_stack_dependencies() {
    for stack in [
        "embassy-sync",
        "embassy-net-driver",
        "xarxa",
        "xarxa-driver",
    ] {
        let fixture = Fixture::new();
        fixture.package("helper", stack, "");
        fixture.package("adapter", "network-adapter-fixture", &format!(
            "[dependencies]\ninnocent_alias = {{ package = {stack:?}, path = \"../helper\", optional = true }}\n"
        ));
        let metadata = fixture.metadata();
        let graph = Graph::from_value(metadata.clone()).unwrap();
        assert_eq!(
            graph
                .reachable(&graph.root(&fixture.manifest).unwrap())
                .len(),
            1
        );
        let error = audit(&fixture, metadata, Boundary::Research)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("forbidden declared production dependency"),
            "{error}"
        );
        assert!(error.contains(stack), "{error}");
    }
}

#[test]
fn research_rejects_transitive_build_stack_but_allows_dev_only_stack() {
    let fixture = Fixture::new();
    fixture.package("helper", "embassy-sync", "");
    fixture.package(
        "adapter",
        "network-adapter-fixture",
        "[dev-dependencies]\nexecutor = { package = \"embassy-sync\", path = \"../helper\" }\n",
    );
    audit(&fixture, fixture.metadata(), Boundary::Research).unwrap();
    fixture.package("adapter", "network-adapter-fixture", "[dependencies]\nfacade = { package = \"device-registers\", path = \"../driver/chips/test-radio\" }\n");
    fixture.package("driver/chips/test-radio", "device-registers", "[build-dependencies]\ngenerator = { package = \"embassy-sync\", path = \"../../../helper\" }\n");
    let error = audit(&fixture, fixture.metadata(), Boundary::Research)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("network-adapter-fixture -> device-registers -> embassy-sync"),
        "{error}"
    );
}

#[test]
fn isolated_all_features_reveals_optional_transitive_stack_without_lock_drift() {
    let fixture = Fixture::new();
    fixture.package(
        "adapter",
        "network-adapter-fixture",
        r#"
[dependencies]
helper = { package = "packet-helper", path = "../helper", optional = true }
[features]
worker = ["helper/executor"]
"#,
    );
    fixture.package("helper", "packet-helper", "[dependencies]\nexecutor = { package = \"embassy-sync\", path = \"../driver/chips/test-radio\", optional = true }\n");
    fixture.package("driver/chips/test-radio", "embassy-sync", "");
    fixture.metadata();
    let lock = std::fs::read(fixture.root().join("Cargo.lock")).unwrap();
    for (flags, accepted) in [(vec![], true), (vec!["--all-features".into()], false)] {
        let graph =
            oer_xtask::cargo::isolated_graph(&fixture.context, &fixture.manifest, &flags, None)
                .unwrap();
        let result = network::audit(
            &graph,
            &fixture.manifest,
            Boundary::Research,
            fixture.root(),
        );
        assert_eq!(result.is_ok(), accepted);
        if !accepted {
            let error = result.unwrap_err().to_string();
            assert!(
                error.contains("network-adapter-fixture -> packet-helper -> embassy-sync"),
                "{error}"
            );
            assert!(
                graph
                    .node(&graph.root(&fixture.manifest).unwrap())
                    .features
                    .iter()
                    .any(|feature| feature.as_str() == "helper")
            );
        }
    }
    assert_eq!(
        std::fs::read(fixture.root().join("Cargo.lock")).unwrap(),
        lock
    );
}
