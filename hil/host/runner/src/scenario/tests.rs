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

#[test]
fn a_scenario_is_marked_unsupported_exactly_when_no_current_image_serves_it() {
    let root = crate::repository_root().unwrap();
    let manifest: toml::Table = toml::from_str(
        &std::fs::read_to_string(root.join("hil/targets/esp32s31/runtime/Cargo.toml")).unwrap(),
    )
    .unwrap();
    let features = manifest["features"].as_table().unwrap();
    // The firmware builds a class only while it declares every feature of
    // the class's recipe.
    let built = |class: ImageClass| {
        class
            .runtime_features()
            .split(',')
            .all(|feature| features.contains_key(feature))
    };
    for scenario in catalog().all() {
        let class = scenario.image();
        let served = if class == ImageClass::BootSmoke {
            built(class)
        } else {
            class
                .capabilities_on("esp32s31")
                .is_some_and(|reported| scenario.family.served_by(&reported))
        };
        assert_eq!(
            served,
            scenario.header.unsupported.is_none(),
            "scenario `{}` on image `{}`: served = {served}",
            scenario.id(),
            class.id()
        );
    }
}

#[test]
fn an_explicit_selection_refuses_an_unsupported_scenario_and_a_tag_skips_it() {
    let directory = tempfile::tempdir().unwrap();
    for (id, unsupported) in [("served", ""), ("unserved", "unsupported = 'no image'\n")] {
        std::fs::write(
            directory.path().join(format!("{id}.toml")),
            format!(
                "schema = 5\nid = '{id}'\ndescription = 'd'\ntags = ['shared']\n{unsupported}\n[bluetooth]\nkind = 'gatt'\n"
            ),
        )
        .unwrap();
    }
    let catalog = Catalog::load(directory.path()).unwrap();
    let unsupported = catalog
        .all()
        .iter()
        .find(|scenario| scenario.header.unsupported.is_some())
        .expect("an unsupported scenario");
    let explicit = crate::cli::Selection {
        scenario: Some(unsupported.id().to_owned()),
        tag: Vec::new(),
    };
    assert!(explicit.resolve(&catalog).is_err());
    let tagged = crate::cli::Selection {
        scenario: None,
        tag: unsupported.header.tags.clone(),
    };
    let selected = tagged.resolve(&catalog).unwrap_or_default();
    assert!(
        selected
            .iter()
            .all(|scenario| scenario.id() != unsupported.id())
    );
    assert!(
        crate::execution::orchestration::named_scenarios(&catalog, &[unsupported.id().to_owned()])
            .is_err()
    );
}
