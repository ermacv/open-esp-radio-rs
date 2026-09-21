//! Canonical executable scenario values. Defaults are a shared data contract;
//! parsing, validation and evidence decisions remain with their respective owner.
use serde_json::Value;

pub(crate) fn normalize(document: &Value) -> Value {
    let defaults: Value = serde_json::from_str(include_str!(
        "../../../../hil/schema/scenario-v4-defaults.json"
    ))
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

/// Compare the executed experiment independently of firmware/source identity.
/// Only named numeric criteria independently evaluated by the consumer may vary.
pub(super) fn matches(
    observation: &super::ScenarioEvidence,
    requirement: &super::HilRequirement,
    catalog: &super::ScenarioCatalog,
) -> super::Result<bool> {
    #[cfg(test)]
    if observation.completion_seal.is_none() && observation.run_directory.is_none() {
        return Ok(true);
    }
    let Some(current) = catalog.definitions.get(&requirement.scenario) else {
        // An empty catalog is used by callers assessing only recorded outcomes.
        // Qualification validates every requirement against a loaded catalog.
        return Ok(true);
    };
    let (Some(run), Some(identity)) = (
        &observation.run_directory,
        observation
            .subject
            .as_ref()
            .and_then(|s| s.procedure.as_ref()),
    ) else {
        return Ok(false);
    };
    let document: Value = super::read_json(&run.join(&identity.path))?;
    Ok(for_requirement(current, requirement) == for_requirement(&document, requirement))
}

fn for_requirement(document: &Value, requirement: &super::HilRequirement) -> Value {
    let mut value = normalize(document);
    // These thresholds only assess retained measurements. Other criteria can
    // request extra traffic/assertions and therefore remain part of execution.
    if !requirement.checks.is_empty()
        && value.pointer("/workload/kind").and_then(Value::as_str) == Some("udp")
        && value.pointer("/workload/direction").and_then(Value::as_str) == Some("rx")
    {
        for key in ["minimum_rx_bps", "maximum_rx_silence_ms"] {
            if let Some(criteria) = value["criteria"].as_object_mut() {
                criteria.remove(key);
            }
        }
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn numeric_reassessment_does_not_relax_stimulus_or_extra_observations() {
        let original = json!({"id":"rx","image":"correctness", "workload":{"kind":"udp","direction":"rx","duration_seconds":30},
            "criteria":{"minimum_rx_bps":1000,"maximum_rx_silence_ms":10}});
        let mut requirement = super::super::HilRequirement {
            scenario: "rx".into(),
            checks: vec!["udp.rx.target-rate".into()],
            minimum_repetitions: 1,
        };
        let mut changed = original.clone();
        changed["criteria"]["minimum_rx_bps"] = json!(2000);
        assert_eq!(
            for_requirement(&original, &requirement),
            for_requirement(&changed, &requirement)
        );
        changed["criteria"]["require_post_maintenance_echo"] = json!(true);
        assert_ne!(
            for_requirement(&original, &requirement),
            for_requirement(&changed, &requirement)
        );
        changed = original.clone();
        changed["workload"]["duration_seconds"] = json!(60);
        assert_ne!(
            for_requirement(&original, &requirement),
            for_requirement(&changed, &requirement)
        );
        changed = original.clone();
        changed["criteria"]["minimum_rx_bps"] = json!(2000);
        requirement.checks.clear();
        assert_ne!(
            for_requirement(&original, &requirement),
            for_requirement(&changed, &requirement)
        );
    }
}
