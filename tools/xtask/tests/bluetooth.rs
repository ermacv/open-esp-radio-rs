mod support;

use oer_xtask::{cargo, checks::architecture, graph::Graph};
use support::Fixture;

#[test]
fn optional_wifi_is_rejected_only_when_selected_by_the_consumer() {
    let fixture = Fixture::new();
    fixture.package("helper", "oer-ieee80211-sta", "");
    fixture.package(
        "adapter",
        "bluetooth-consumer-fixture",
        "[dependencies]\nnetwork = { package = \"oer-ieee80211-sta\", path = \"../helper\", optional = true }\n[features]\nbluetooth = []\naggregate = [\"dep:network\"]\n",
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
            architecture::reject_wifi_in_bluetooth(&graph, &fixture.manifest).is_ok(),
            accepted
        );
    }
}

#[test]
fn renamed_transitive_build_dependency_cannot_hide_wifi() {
    let fixture = Fixture::new();
    fixture.package(
        "crates/hardware/test-radio",
        "oer-esp32s31-ieee80211-ap",
        "",
    );
    fixture.package(
        "helper", "packet-helper",
        "[build-dependencies]\ngenerator = { package = \"oer-esp32s31-ieee80211-ap\", path = \"../crates/hardware/test-radio\" }\n",
    );
    fixture.package(
        "adapter",
        "bluetooth-consumer-fixture",
        "[dependencies]\nqueue = { package = \"packet-helper\", path = \"../helper\" }\n",
    );
    let graph = Graph::from_value(fixture.metadata()).unwrap();
    let error = architecture::reject_wifi_in_bluetooth(&graph, &fixture.manifest)
        .unwrap_err()
        .to_string();
    assert!(error.contains("oer-esp32s31-ieee80211-ap"), "{error}");
}

#[test]
fn development_only_wifi_does_not_enter_the_production_closure() {
    let fixture = Fixture::new();
    fixture.package("helper", "oer-ieee80211-sta", "");
    fixture.package(
        "adapter",
        "bluetooth-consumer-fixture",
        "[dev-dependencies]\npeer = { package = \"oer-ieee80211-sta\", path = \"../helper\" }\n",
    );
    let graph = Graph::from_value(fixture.metadata()).unwrap();
    architecture::reject_wifi_in_bluetooth(&graph, &fixture.manifest).unwrap();
}
