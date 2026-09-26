//! Independent validation of controlled experiment inputs.
//!
//! This consumer deliberately does not import runner types. A comparison is
//! neither an inherited Wi-Fi qualification nor a non-regression verdict.

use super::*;

/// The one supported experiment: station UDP RX with a PHY maintenance
/// operation against the same scenario without it. Every other executable
/// value, including image, data path, link, observers and criteria, is equal.
pub(super) fn validate(
    documents: &BTreeMap<String, serde_json::Value>,
) -> Result<BTreeMap<String, String>> {
    let mut controls = BTreeMap::new();
    for (id, experiment) in documents {
        let Some(control) = experiment.get("control").filter(|v| !v.is_null()) else {
            continue;
        };
        let control = control
            .as_str()
            .ok_or_else(|| format!("{id}: control must be a scenario id"))?
            .to_owned();
        let baseline = documents
            .get(&control)
            .ok_or_else(|| format!("{id}: missing comparison control {control}"))?;
        if control == *id || baseline.get("control").is_some_and(|v| !v.is_null()) {
            return Err(format!("{id}: comparison control must be independent").into());
        }
        let mut normalized = procedure::normalize(experiment);
        let mut baseline = procedure::normalize(baseline);
        let maintenance = normalized
            .pointer_mut("/wifi/workload")
            .and_then(serde_json::Value::as_object_mut)
            .and_then(|workload| workload.remove("maintenance"));
        if !checks::receive_only_station_udp(&normalized)
            || maintenance.is_none()
            || baseline.pointer("/wifi/workload/maintenance").is_some()
        {
            return Err(format!(
                "{id}: comparison requires station UDP RX with and without maintenance"
            )
            .into());
        }
        for value in [&mut normalized, &mut baseline] {
            let fields = value.as_object_mut().ok_or("scenario must be an object")?;
            for field in ["id", "control"] {
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
        changed.get_mut(&control_id).unwrap()["wifi"]["workload"]["link"]["phy"] = "he20".into();
        assert!(validate(&changed).is_err());
        let mut changed = documents.clone();
        changed.get_mut(&control_id).unwrap()["wifi"]["workload"]["payload_bytes"] = 512.into();
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
