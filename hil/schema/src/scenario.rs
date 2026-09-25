//! Canonical executable scenario values. Producers digest and evaluators compare
//! scenarios in this form, so both must fill the same schema-4 defaults.
use serde_json::Value;

/// Fill schema-4 defaults and drop presentation-only fields.
pub fn normalize(document: &Value) -> Value {
    let defaults: Value = serde_json::from_str(include_str!("../scenario-v4-defaults.json"))
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
        object.remove("transfer");
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
