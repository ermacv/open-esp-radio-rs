use super::*;

fn declaration(id: &str) -> CapabilityDocument {
    toml_edit::de::from_str(&format!(
        r#"
id = "{id}"
title = "Test scope"
scope = "One bounded operation"
implementation = "complete"
host = "covered"
async = "bounded"
hil-requirements = [{{ scenario = "exchange", minimum-repetitions = 2 }}]
"#
    ))
    .unwrap()
}

#[test]
fn static_map_preserves_implementation_without_inventing_missing_evidence() {
    let mut catalog = CatalogView::default();
    catalog
        .capabilities
        .insert("exchange".into(), declaration("exchange"));
    let map = ProjectMap::from_catalog(&catalog, None).unwrap();
    let row = &map.entries[0];
    assert_eq!(row.implementation, "complete");
    assert_eq!(row.host_declaration.as_deref(), Some("covered"));
    assert_eq!(row.knowledge_status, "not-linked");
    assert!(row.evidence.is_none());
    assert!(map.next.iter().any(|a| a.kind == WorkKind::InspectEvidence));
    assert!(
        !map.next
            .iter()
            .any(|a| matches!(a.kind, WorkKind::Experiment | WorkKind::Recheck))
    );
    let json = serde_json::to_value(map).unwrap();
    assert!(json.get("ready").is_none());
    assert!(json["entries"][0].get("ready").is_none());
}

#[test]
fn focused_map_follows_declared_dependencies_without_including_other_roles() {
    let mut catalog = CatalogView::default();
    for id in ["shared-phy", "ble", "wifi"] {
        let mut d = declaration(id);
        if id != "shared-phy" {
            d.depends_on.push("shared-phy".into());
        }
        catalog.capabilities.insert(id.into(), d);
    }
    let map = ProjectMap::from_catalog(&catalog, Some("ble")).unwrap();
    assert_eq!(
        map.entries
            .iter()
            .map(|e| e.id.as_str())
            .collect::<Vec<_>>(),
        ["ble", "shared-phy"]
    );
    assert!(map.next.iter().all(|a| a.entry != "wifi"));
    assert!(ProjectMap::from_catalog(&catalog, Some("missing")).is_err());
}

#[test]
fn existing_gap_classification_distinguishes_measurement_from_implementation() {
    let mut d = declaration("ble");
    d.gaps = toml_edit::de::from_str::<CapabilityDocument>(
        r#"
id = "ble"
title = "BLE"
scope = "One connection"
implementation = "incomplete"
host = "incomplete"
async = "bounded"
gaps = [
 {axis = "implementation", id = "shutdown"},
 {axis = "hil", id = "timing"},
 {axis = "hil", id = "unknown"},
]
"#,
    )
    .unwrap()
    .gaps;
    d.development = toml_edit::de::from_str(r#"
knowledge = ["registers/contract.toml"]
gap-work = [{gap = "timing", kind = "measurement-method", reason = "Need a suitable independent observer."}]
"#).unwrap();
    let mut catalog = CatalogView::default();
    catalog.capabilities.insert(d.id.clone(), d);
    let map = ProjectMap::from_catalog(&catalog, None).unwrap();
    assert_eq!(map.entries[0].knowledge_status, "linked-not-assessed");
    let work = |subject| map.next.iter().find(|a| a.subject == subject).unwrap().kind;
    assert_eq!(work("shutdown"), WorkKind::Implement);
    assert_eq!(work("timing"), WorkKind::MeasurementMethod);
    assert_eq!(work("unknown"), WorkKind::ReviewGap);
}

#[test]
fn source_facts_survive_without_any_qualification_program() {
    let mut catalog = CatalogView::default();
    let fact = toml_edit::de::from_str(
        r#"
id = "dma-publication"
status = "diagnostic"
level = "lower-primitive"
[source-contract]
id = "dma-publication"
composition = "diagnostic"
scope = "One descriptor publication"
limits = "Completion not understood"
source-paths = ["crates/dma.rs"]
"#,
    )
    .unwrap();
    catalog.source_facts.insert("dma-publication".into(), fact);
    let map = ProjectMap::from_catalog(&catalog, None).unwrap();
    assert_eq!(map.entries.len(), 1);
    assert_eq!(map.entries[0].kind, "source-fact");
    assert_eq!(map.entries[0].implementation, "DIAGNOSTIC");
    assert!(map.entries[0].evidence.is_none());
}

#[test]
fn map_order_is_independent_of_catalog_input_order() {
    let mut a = CatalogView::default();
    let mut b = CatalogView::default();
    for id in ["a", "b"] {
        a.capabilities.insert(id.into(), declaration(id));
    }
    for id in ["b", "a"] {
        b.capabilities.insert(id.into(), declaration(id));
    }
    assert_eq!(
        serde_json::to_value(ProjectMap::from_catalog(&a, None).unwrap()).unwrap(),
        serde_json::to_value(ProjectMap::from_catalog(&b, None).unwrap()).unwrap()
    );
}

#[test]
fn inventory_markdown_links_resolve_against_their_original_owner() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut entry = Entry::new(
        "test",
        "inventory",
        "Test",
        "See [contract](contract.md).",
        "PARTIAL",
    );
    entry.documentation_base = Some(PathBuf::from("docs/FEATURES.md"));
    let mut map = ProjectMap::from_catalog(&CatalogView::default(), None).unwrap();
    map.entries.push(entry);
    let text = map.markdown(&root.join("target/map"), root).unwrap();
    assert!(text.contains("[contract](../../docs/contract.md)"));
    assert!(!text.contains("[contract](contract.md)"));
}
