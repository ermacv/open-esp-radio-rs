use super::*;
use hil_core::{image::ImageClass, scenario::identity::normalize};

fn catalog() -> Catalog {
    Catalog::load(&crate::repository_root().unwrap().join("hil/scenarios")).unwrap()
}

#[test]
fn every_catalog_scenario_has_the_same_identity_before_and_after_typed_defaults() {
    let catalog = catalog();
    for scenario in catalog.all() {
        let raw: toml::Table =
            toml::from_str(&std::fs::read_to_string(scenario.source()).unwrap()).unwrap();
        let typed = serde_json::to_value(scenario).unwrap();
        assert_eq!(
            normalize(&serde_json::to_value(raw).unwrap()),
            normalize(&typed),
            "{}",
            scenario.id()
        );
        let snapshot = Scenario::from_json(&serde_json::to_vec(&typed).unwrap()).unwrap();
        assert_eq!(snapshot.family, scenario.family, "{}", scenario.id());
    }
}

#[test]
fn a_document_names_exactly_one_known_family() {
    let header = "schema = 5\nid = 'x'\ndescription = 'd'\n";
    let parse = |family: &str| {
        Scenario::from_toml(&format!("{header}{family}"), std::path::Path::new("x.toml"))
    };
    parse("[system]\nkind = 'boot-smoke'\n").unwrap();
    for family in [
        "",
        "[system]\nkind = 'boot-smoke'\n[bluetooth]\nkind = 'gatt'\n",
        "[thread]\nkind = 'boot-smoke'\n",
        "image = 'boot-smoke'\n[system]\nkind = 'boot-smoke'\n",
    ] {
        assert!(parse(family).is_err(), "accepted {family:?}");
    }
}

#[test]
fn only_wifi_defines_a_controlled_comparison() {
    let catalog = catalog();
    let experiment = catalog
        .get("diagnostic-station-phy-combined-high-load-delivery-rx")
        .unwrap();
    let control = catalog.get(experiment.control().unwrap()).unwrap();
    experiment.family.validate_control(&control.family).unwrap();
    let boot = catalog.get("boot-smoke").unwrap();
    assert!(boot.family.validate_control(&boot.family).is_err());
    let experiments = catalog
        .all()
        .iter()
        .filter(|scenario| scenario.control().is_some())
        .count();
    assert!(experiments > 0);
}

#[test]
fn every_image_class_is_reached_by_a_family_plan() {
    let catalog = catalog();
    for class in ImageClass::ALL {
        if matches!(
            class,
            ImageClass::DiagnosticMacIrq | ImageClass::DiagnosticTxWait
        ) && !catalog
            .all()
            .iter()
            .any(|scenario| scenario.image() == class)
        {
            continue;
        }
        assert!(
            catalog
                .all()
                .iter()
                .any(|scenario| scenario.image() == class),
            "no scenario runs {}",
            class.id()
        );
    }
}

#[test]
fn selection_requirements_union_every_family() {
    let catalog = catalog();
    let selected = [
        catalog.get("bluetooth-dtm-bidirectional").unwrap(),
        catalog.get("station-ap-loss").unwrap(),
        catalog.get("boot-smoke").unwrap(),
    ];
    let required = requirements(&selected);
    assert!(required.bluetooth_adapter && required.station_network && required.station_control);
    assert!(requirements(&[catalog.get("boot-smoke").unwrap()]) == Requirements::default());
}
