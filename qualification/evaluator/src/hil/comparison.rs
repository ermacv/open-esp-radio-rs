//! Independent validation of controlled experiment inputs.
//!
//! This consumer deliberately does not import runner types. A comparison is
//! neither an inherited Wi-Fi qualification nor a non-regression verdict.

use super::*;

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum Relation {
    WifiPhyMaintenance { control: String },
}

pub(super) fn validate(
    documents: &BTreeMap<String, serde_json::Value>,
) -> Result<BTreeMap<String, String>> {
    let mut controls = BTreeMap::new();
    for (id, experiment) in documents {
        let Some(relation) = experiment.get("comparison").filter(|v| !v.is_null()) else {
            continue;
        };
        let Relation::WifiPhyMaintenance { control } = serde_json::from_value(relation.clone())?;
        let baseline = documents
            .get(&control)
            .ok_or_else(|| format!("{id}: missing comparison control {control}"))?;
        if control == *id || baseline.get("comparison").is_some_and(|v| !v.is_null()) {
            return Err(format!("{id}: comparison control must be independent").into());
        }
        let mut normalized = procedure::normalize(experiment);
        let workload = normalized
            .get_mut("workload")
            .and_then(serde_json::Value::as_object_mut)
            .ok_or_else(|| format!("{id}: comparison needs a workload"))?;
        if workload.get("kind").and_then(serde_json::Value::as_str) != Some("udp")
            || workload
                .get("direction")
                .and_then(serde_json::Value::as_str)
                != Some("rx")
            || workload
                .remove("station_pause")
                .and_then(|value| value.as_str().map(str::to_owned))
                .is_none()
            || baseline
                .pointer("/workload/station_pause")
                .is_some_and(|v| !v.is_null())
            || experiment.get("link").is_none_or(|v| v.is_null())
        {
            return Err(format!(
                "{id}: comparison requires station UDP RX with and without maintenance"
            )
            .into());
        }
        let mut baseline = procedure::normalize(baseline);
        baseline["workload"]
            .as_object_mut()
            .unwrap()
            .remove("station_pause");
        for value in [&mut normalized, &mut baseline] {
            let fields = value.as_object_mut().ok_or("scenario must be an object")?;
            for field in ["id", "description", "tags", "comparison"] {
                fields.remove(field);
            }
        }
        if normalized != baseline {
            return Err(format!("{id}: control {control} differs beyond maintenance").into());
        }
        controls.insert(id.clone(), control);
    }
    Ok(controls)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_pair_is_valid_but_cross_phy_or_changed_load_is_not() {
        let control: serde_json::Value = toml_edit::de::from_str(include_str!("../../../../hil/scenarios/ieee80211/station/diagnostic-station-phy-baseline-high-load-delivery-rx.toml")).unwrap();
        let experiment: serde_json::Value = toml_edit::de::from_str(include_str!("../../../../hil/scenarios/ieee80211/station/diagnostic-station-phy-combined-high-load-delivery-rx.toml")).unwrap();
        let control_id = control["id"].as_str().unwrap().to_owned();
        let experiment_id = experiment["id"].as_str().unwrap().to_owned();
        let documents = BTreeMap::from([
            (control_id.clone(), control),
            (experiment_id.clone(), experiment),
        ]);
        assert_eq!(validate(&documents).unwrap()[&experiment_id], control_id);
        let mut changed = documents.clone();
        changed.get_mut(&control_id).unwrap()["link"]["phy"] = "he20".into();
        assert!(validate(&changed).is_err());
        let mut changed = documents.clone();
        changed.get_mut(&control_id).unwrap()["workload"]["payload_bytes"] = 512.into();
        assert!(validate(&changed).is_err());
        let mut changed = documents;
        changed.remove(&control_id);
        assert!(validate(&changed).is_err());
    }

    #[test]
    fn old_or_unrelated_control_runs_cannot_qualify_an_experiment() {
        let requirement = HilRequirement {
            scenario: "experiment".into(),
            checks: vec![],
            minimum_repetitions: 3,
        };
        let mut index = HilEvidenceIndex::synthetic(&[("experiment", 3), ("control", 3)]);
        let catalog = ScenarioCatalog {
            controls: BTreeMap::from([("experiment".into(), "control".into())]),
            ..ScenarioCatalog::default()
        };
        assert!(index.evidence_for(&requirement, &catalog).is_none());
        index.scenarios.get_mut("control").unwrap()[0].run_id = "synthetic-experiment".into();
        assert!(
            index
                .evidence_for(&requirement, &catalog)
                .unwrap()
                .contains(":control=control")
        );
        index.scenarios.get_mut("control").unwrap()[0].repetitions = 1;
        assert!(index.evidence_for(&requirement, &catalog).is_none());
    }
}
