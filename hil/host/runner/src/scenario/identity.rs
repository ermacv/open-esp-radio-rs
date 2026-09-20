//! Canonical executable scenario values. Defaults are a shared data contract;
//! parsing, validation and evidence decisions remain with their respective owner.
use serde_json::Value;

pub(crate) fn normalize(document: &Value) -> Value {
    let defaults: Value =
        serde_json::from_str(include_str!("../../../../schema/scenario-v4-defaults.json"))
            .expect("compiled scenario defaults must be valid JSON");
    let mut value = document.clone();
    fill(&mut value, &defaults["root"]);
    fill(&mut value["link"], &defaults["link"]);
    if let Some(kind) = value["workload"]["kind"].as_str().map(str::to_owned) {
        fill(&mut value["workload"], &defaults["workloads"][kind]);
    }
    if let Some(kind) = value["workload"]["traffic"]["kind"]
        .as_str()
        .map(str::to_owned)
    {
        fill(
            &mut value["workload"]["traffic"],
            &defaults["ap_traffic"][kind],
        );
    }
    if let Some(object) = value.as_object_mut() {
        object.remove("description");
        object.remove("tags");
    }
    value
}
fn fill(value: &mut Value, defaults: &Value) {
    if let (Some(object), Some(defaults)) = (value.as_object_mut(), defaults.as_object()) {
        for (key, default) in defaults {
            let current = object.entry(key.clone()).or_insert_with(|| default.clone());
            fill(current, default);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_catalog_scenario_has_the_same_identity_before_and_after_typed_defaults() {
        let root = crate::repository_root().unwrap();
        let catalog = crate::scenario::Catalog::load(&root.join("hil/scenarios")).unwrap();
        for scenario in catalog.all() {
            let raw: Value =
                toml::from_str(&std::fs::read_to_string(&scenario.source).unwrap()).unwrap();
            assert_eq!(
                normalize(&raw),
                normalize(&serde_json::to_value(scenario).unwrap()),
                "{}",
                scenario.id
            );
        }
    }
}
