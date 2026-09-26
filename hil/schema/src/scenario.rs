//! Canonical executable scenario values. Producers digest and evaluators
//! compare scenarios in this form, so both fill the same schema-5 defaults.
//!
//! The defaults document mirrors the scenario tree. A `$kind` entry maps a
//! table's `kind` discriminator to the defaults of that variant, and a key
//! prefixed with `?` names an optional table whose defaults apply only when
//! the table is present. Absent optional values and explicit nulls are the
//! same value, so nulls are removed before defaults are filled.
use serde_json::{Map, Value};

/// Remove presentation-only fields, drop nulls and fill schema-5 defaults.
pub fn normalize(document: &Value) -> Value {
    let defaults: Value = serde_json::from_str(include_str!("../scenario-v5-defaults.json"))
        .expect("compiled scenario defaults must be valid JSON");
    let mut value = document.clone();
    remove_nulls(&mut value);
    fill(&mut value, &defaults);
    if let Some(object) = value.as_object_mut() {
        object.remove("description");
        object.remove("tags");
        object.remove("transfer");
    }
    value
}

fn remove_nulls(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.retain(|_, value| !value.is_null());
            object.values_mut().for_each(remove_nulls);
        }
        Value::Array(values) => values.iter_mut().for_each(remove_nulls),
        _ => {}
    }
}

fn fill(value: &mut Value, defaults: &Value) {
    let (Some(object), Some(defaults)) = (value.as_object_mut(), defaults.as_object()) else {
        return;
    };
    for (key, default) in defaults {
        if key == "$kind" {
            continue;
        }
        if let Some(optional) = key.strip_prefix('?') {
            if let Some(present) = object.get_mut(optional) {
                fill(present, default);
            }
            continue;
        }
        let current = object
            .entry(key.clone())
            .or_insert_with(|| empty_like(default));
        fill(current, default);
    }
    let variant = object
        .get("kind")
        .and_then(Value::as_str)
        .and_then(|kind| defaults.get("$kind")?.get(kind));
    if let Some(variant) = variant {
        fill(value, variant);
    }
}

/// The value a missing entry starts from: scalars and arrays are the default
/// itself, while tables are filled key by key so markers are never copied.
fn empty_like(default: &Value) -> Value {
    match default {
        Value::Object(_) => Value::Object(Map::new()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn variant_and_optional_defaults_apply_only_where_selected() {
        let control = json!({"schema": 5, "id": "x", "description": "d", "tags": ["a"],
            "wifi": {"image": "correctness", "workload": {"kind": "station-udp",
            "link": {"phy": "ht40", "minimum_mcs": null}, "offer": {"rx_bps": 1000}}}});
        let normalized = normalize(&control);
        assert_eq!(normalized["repetitions"], 1);
        assert!(normalized.get("description").is_none() && normalized.get("tags").is_none());
        let workload = &normalized["wifi"]["workload"];
        assert_eq!(
            workload["link"],
            json!({"phy": "ht40", "guard_interval": "any"})
        );
        assert_eq!(workload["criteria"]["exact_delivery"], false);
        assert!(workload.get("maintenance").is_none());
        assert_eq!(normalized["wifi"]["datapath"]["l1_cache_counters"], false);
        let mut experiment = control.clone();
        experiment["wifi"]["workload"]["maintenance"] = json!({"operation": "calibration"});
        let normalized = normalize(&experiment);
        assert_eq!(
            normalized["wifi"]["workload"]["maintenance"]["require_post_maintenance_echo"],
            false
        );
        let access_point = normalize(&json!({"wifi": {"workload": {"kind": "access-point",
            "clients": {"kind": "openwrt"}, "traffic": {"kind": "icmp"}}}}));
        let workload = &access_point["wifi"]["workload"];
        assert!(workload.get("link").is_none());
        assert_eq!(workload["clients"]["fixed_guard_interval"], false);
        assert_eq!(workload["traffic"]["maximum_lost"], 0);
        let laptop = normalize(&json!({"wifi": {"workload": {"kind": "access-point",
            "traffic": {"kind": "none"}}}}));
        assert_eq!(
            laptop["wifi"]["workload"]["clients"],
            json!({"kind": "laptop"})
        );
    }
}
