use super::*;
use crate::scenario::test_family::{self, TestFamily};

pub(super) fn catalog() -> Catalog<TestFamily> {
    test_family::catalog()
}

const EXPERIMENT: &str = "experiment";
const CONTROL: &str = "baseline";

#[test]
fn selecting_an_experiment_adds_only_its_control() {
    let catalog = catalog();
    let experiment = catalog.get(EXPERIMENT).unwrap();
    let plan = Plan::create(&catalog, &[experiment], Integration::UpstreamXarxa).unwrap();
    let (selected, _) = plan.resolve(&catalog).unwrap();
    assert_eq!(
        selected.iter().map(|s| s.id()).collect::<Vec<_>>(),
        [CONTROL, EXPERIMENT]
    );
    assert_eq!(
        plan.scenarios[0].reasons,
        vec![Reason::ControlFor {
            experiment: EXPERIMENT.into()
        }]
    );
    assert!(plan.requirements.station_network);
    let roundtrip: Plan = serde_json::from_slice(&serde_json::to_vec(&plan).unwrap()).unwrap();
    assert_eq!(plan, roundtrip);
}

#[test]
fn selecting_control_directly_does_not_duplicate_it() {
    let catalog = catalog();
    let experiment = catalog.get(EXPERIMENT).unwrap();
    let control = catalog.get(CONTROL).unwrap();
    let plan = Plan::create(&catalog, &[control, experiment], Integration::UpstreamXarxa).unwrap();
    assert_eq!(plan.scenarios.len(), 2);
    assert_eq!(plan.scenarios[0].reasons.len(), 2);
    assert!(Plan::create(&catalog, &[control, control], Integration::UpstreamXarxa).is_err());
}

#[test]
fn named_check_selection_is_scoped_and_does_not_promote_controls() {
    let catalog = catalog();
    let selected = catalog
        .all()
        .iter()
        .filter(|scenario| scenario.header.tags.iter().any(|tag| tag == "he20"))
        .collect::<Vec<_>>();
    let plan = Plan::create_for_checks(
        &catalog,
        &selected,
        Integration::UpstreamXarxa,
        &["wifi.maintenance.same-link".into()],
    )
    .unwrap();
    let (resolved, _) = plan.resolve(&catalog).unwrap();
    assert_eq!(resolved.len(), 2);
    assert_eq!(plan.requested, [EXPERIMENT]);
    assert!(
        !plan.scenarios[0]
            .supported_checks
            .contains(&"wifi.maintenance.same-link".into())
    );
    assert!(
        matches!(&plan.scenarios[1].reasons[1], Reason::ProvidesChecks { checks } if checks == &["wifi.maintenance.same-link"])
    );
    for checks in [
        vec!["not-a-check".into()],
        vec![
            "wifi.maintenance.same-link".into(),
            "udp.rx.maximum-silence".into(),
        ],
    ] {
        assert!(
            Plan::create_for_checks(&catalog, &selected, Integration::UpstreamXarxa, &checks)
                .is_err()
        );
    }
}

#[test]
fn ordinary_selection_does_not_expand_and_modified_plans_fail_closed() {
    let catalog = catalog();
    let plan = Plan::create(
        &catalog,
        &[catalog.get("boot-smoke").unwrap()],
        Integration::UpstreamXarxa,
    )
    .unwrap();
    assert_eq!(plan.scenarios.len(), 1);
    let mut changed = plan.clone();
    changed.scenarios[0].repetitions += 1;
    assert!(changed.resolve(&catalog).is_err());
    let mut changed = plan.clone();
    changed.scenarios[0].scenario_sha256 = "00".repeat(32);
    assert!(changed.resolve(&catalog).is_err());
    let mut changed = plan.clone();
    changed.schema = 1;
    assert!(changed.resolve(&catalog).is_err());
    let mut changed = plan;
    changed.scenarios.clear();
    assert!(changed.resolve(&catalog).is_err());
}

#[test]
fn procedure_identity_ignores_annotations_but_tracks_execution() {
    let catalog = catalog();
    let mut scenario = catalog.get("boot-smoke").unwrap().clone();
    let original = procedure(&scenario).unwrap();
    scenario.header.description.push_str(" Clarified wording.");
    scenario.header.tags.push("documentation".into());
    assert_eq!(original, procedure(&scenario).unwrap());
    let plan = Plan::create(
        &catalog,
        &[catalog.get("boot-smoke").unwrap()],
        Integration::UpstreamXarxa,
    )
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("boot-smoke.toml");
    std::fs::write(&path, toml::to_string(&scenario).unwrap()).unwrap();
    let annotated = Catalog::<TestFamily>::load(directory.path()).unwrap();
    assert!(plan.resolve(&annotated).is_ok());
    scenario.header.repetitions += 1;
    std::fs::write(&path, toml::to_string(&scenario).unwrap()).unwrap();
    let changed = Catalog::<TestFamily>::load(directory.path()).unwrap();
    assert!(plan.resolve(&changed).is_err());
    assert_ne!(original, procedure(&scenario).unwrap());
}
