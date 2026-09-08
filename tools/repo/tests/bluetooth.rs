mod support;

use oer_xtask::{cargo, checks::bluetooth, graph::Graph};
use support::Fixture;

#[test]
fn optional_wifi_is_rejected_only_when_selected_by_the_consumer() {
    let fixture = Fixture::new();
    fixture.package("helper", "oer-wifi-sta", "");
    fixture.package(
        "adapter",
        "bluetooth-consumer-fixture",
        "[dependencies]\nnetwork = { package = \"oer-wifi-sta\", path = \"../helper\", optional = true }\n[features]\nbluetooth = []\naggregate = [\"dep:network\"]\n",
    );
    fixture.metadata();
    for (profile, accepted) in [("bluetooth", true), ("aggregate", false)] {
        let graph = cargo::isolated_graph(
            &fixture.context,
            &fixture.manifest,
            &[
                "--no-default-features".into(),
                "--features".into(),
                profile.into(),
            ],
            None,
        )
        .unwrap();
        assert_eq!(
            bluetooth::audit(&graph, &fixture.manifest).is_ok(),
            accepted
        );
    }
}

#[test]
fn renamed_transitive_build_dependency_cannot_hide_wifi() {
    let fixture = Fixture::new();
    fixture.package("crates/hardware/test-radio", "oer-esp32s31-wifi-ap", "");
    fixture.package(
        "helper", "packet-helper",
        "[build-dependencies]\ngenerator = { package = \"oer-esp32s31-wifi-ap\", path = \"../crates/hardware/test-radio\" }\n",
    );
    fixture.package(
        "adapter",
        "bluetooth-consumer-fixture",
        "[dependencies]\nqueue = { package = \"packet-helper\", path = \"../helper\" }\n",
    );
    let graph = Graph::from_value(fixture.metadata()).unwrap();
    let error = bluetooth::audit(&graph, &fixture.manifest)
        .unwrap_err()
        .to_string();
    assert!(error.contains("oer-esp32s31-wifi-ap"), "{error}");
}

#[test]
fn development_only_wifi_does_not_enter_the_production_closure() {
    let fixture = Fixture::new();
    fixture.package("helper", "oer-wifi-sta", "");
    fixture.package(
        "adapter",
        "bluetooth-consumer-fixture",
        "[dev-dependencies]\npeer = { package = \"oer-wifi-sta\", path = \"../helper\" }\n",
    );
    let graph = Graph::from_value(fixture.metadata()).unwrap();
    bluetooth::audit(&graph, &fixture.manifest).unwrap();
}
