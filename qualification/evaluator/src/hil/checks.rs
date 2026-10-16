//! Independent checking of named observations against scenario-owned criteria.
//!
//! The recorded assessment must be internally valid, but is not the current
//! criterion. Numeric observations are compared with the requested contract.
//! Applicability and experiment completion are decided separately by the index.

use super::*;
use serde_json::Value;

#[derive(Clone, Debug)]
pub(super) struct Contract {
    unit: &'static str,
    comparison: &'static str,
    threshold: u64,
}

/// Station UDP whose offer flows only to the target.
pub(super) fn receive_only_station_udp(document: &Value) -> bool {
    document
        .pointer("/wifi/workload/kind")
        .and_then(Value::as_str)
        == Some("station-udp")
        && document
            .pointer("/wifi/workload/offer/rx_bps")
            .is_some_and(|v| !v.is_null())
        && document
            .pointer("/wifi/workload/offer/tx_bps")
            .is_none_or(Value::is_null)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Assessment {
    Passed,
    Failed,
    Unavailable,
}

pub(super) fn assess(name: &str, contract: &Contract, measurements: &[Value]) -> Assessment {
    // Validate the original assessment, allowing either truthful outcome here.
    // The run loader separately validates it against the actual repetition.
    if measurement::validate(measurements, Outcome::Failed).is_err() {
        return Assessment::Unavailable;
    }
    let mut matches = measurements
        .iter()
        .filter(|measurement| measurement.get("name").and_then(Value::as_str) == Some(name));
    let Some(measurement) = matches.next() else {
        return Assessment::Unavailable;
    };
    let Some(value) = measurement.get("value").and_then(Value::as_u64) else {
        return Assessment::Unavailable;
    };
    if matches.next().is_some()
        || measurement.get("unit").and_then(Value::as_str) != Some(contract.unit)
    {
        return Assessment::Unavailable;
    }
    let passed = match contract.comparison {
        "at-least" => value >= contract.threshold,
        "at-most" => value <= contract.threshold,
        "exactly" => value == contract.threshold,
        _ => return Assessment::Unavailable,
    };
    if passed {
        Assessment::Passed
    } else {
        Assessment::Failed
    }
}

/// A contract no scenario family publishes, for the mechanism's tests.
#[cfg(test)]
pub(super) fn contract(unit: &'static str, comparison: &'static str, threshold: u64) -> Contract {
    Contract {
        unit,
        comparison,
        threshold,
    }
}

#[cfg(test)]
fn passes(name: &str, contract: &Contract, measurements: &[Value]) -> bool {
    assess(name, contract, measurements) == Assessment::Passed
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NAME: &str = "proof.count";

    /// The repository catalog with `checks` published for `scenario`.
    fn catalog(scenario: &str, checks: &[&str]) -> ScenarioCatalog {
        let mut catalog = ScenarioCatalog::load(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .as_path(),
            Path::new("hil/scenarios"),
        )
        .unwrap();
        catalog.checks.insert(
            scenario.into(),
            checks
                .iter()
                .map(|name| ((*name).to_owned(), contract("count", "exactly", 1)))
                .collect(),
        );
        catalog
    }

    fn observed(name: &str, value: u64) -> Value {
        json!({"name": name, "value": value, "unit": "count", "semantics": 1, "better": null, "threshold": {"comparison": "exactly", "value": 1}, "verdict": "passed"})
    }

    #[test]
    fn independently_checks_values_units_thresholds_duplicates_and_missing_data() {
        let contract = contract("count", "exactly", 1);
        assert!(passes(NAME, &contract, &[observed(NAME, 1)]));
        assert!(!passes(NAME, &contract, &[observed(NAME, 0)]));
        assert!(!passes(NAME, &contract, &[]));
        assert!(!passes(
            NAME,
            &contract,
            &[observed(NAME, 1), observed(NAME, 1)]
        ));
        assert_eq!(
            assess(NAME, &contract, &[observed("other.count", 1)]),
            Assessment::Unavailable
        );
        for changed in [
            json!({"unit": "bytes"}),
            json!({"verdict": null}),
            json!({"threshold": {"comparison": "at-least", "value": 2}}),
        ] {
            let mut sample = observed(NAME, 1);
            sample
                .as_object_mut()
                .unwrap()
                .extend(changed.as_object().unwrap().clone());
            assert!(!passes(NAME, &contract, &[sample]));
        }
    }

    #[test]
    fn criteria_are_recomputed_without_rewriting_the_original_assessment() {
        let name = "proof.rate";
        let original = json!({"name": name, "value":90_000_000,
            "unit":"bits-per-second", "semantics": 1, "better": "higher", "threshold":{"comparison":"at-least","value":95_000_000},
            "verdict":"failed"});
        let measurements = vec![original.clone()];
        for (floor, expected) in [(85_000_000, true), (90_000_000, true), (95_000_000, false)] {
            assert_eq!(
                passes(
                    name,
                    &contract("bits-per-second", "at-least", floor),
                    &measurements
                ),
                expected
            );
        }
        assert!(passes(
            name,
            &contract("bits-per-second", "at-most", 90_000_000),
            &measurements
        ));
        assert_eq!(measurements[0], original);
        // Reassessing one number is not authorization to turn a failed
        // scenario/lifecycle into a successful qualification observation.
    }

    #[test]
    fn every_repetition_must_contain_the_requested_proof() {
        let catalog = catalog("bluetooth-trouble-secure-gatt-hci-read-failure", &[NAME]);
        let requirement = HilRequirement {
            scenario: "bluetooth-trouble-secure-gatt-hci-read-failure".into(),
            checks: vec![NAME.into()],
            minimum_repetitions: 3,
            bounds: Vec::new(),
        };
        catalog.validate_requirement(&requirement).unwrap();
        let mut index = HilEvidenceIndex::synthetic(&[(&requirement.scenario, 3)]);
        assert!(index.evidence_for(&requirement, &catalog).is_none());
        index.scenarios.get_mut(&requirement.scenario).unwrap()[0].measurements =
            vec![vec![observed(NAME, 1)]; 3];
        assert!(
            index
                .evidence_for(&requirement, &catalog)
                .unwrap()
                .contains(":checks=proof.count")
        );
        index.scenarios.get_mut(&requirement.scenario).unwrap()[0].measurements[1].clear();
        assert!(index.evidence_for(&requirement, &catalog).is_none());
    }

    #[test]
    fn scope_and_duplicate_requirements_fail_static_validation() {
        let catalog = catalog("bluetooth-trouble-secure-gatt-hci-read-failure", &[NAME]);
        let mut requirement = HilRequirement {
            scenario: "boot-smoke".into(),
            checks: vec![NAME.into()],
            minimum_repetitions: 1,
            bounds: Vec::new(),
        };
        assert!(catalog.validate_requirement(&requirement).is_err());
        requirement.scenario = "bluetooth-trouble-secure-gatt-hci-read-failure".into();
        catalog.validate_requirement(&requirement).unwrap();
        requirement.checks.push(NAME.into());
        assert!(catalog.validate_requirement(&requirement).is_err());
    }

    #[test]
    fn individual_proofs_from_different_runs_do_not_satisfy_one_obligation() {
        let other_name = "proof.other";
        let catalog = catalog(
            "bluetooth-trouble-secure-gatt-hci-read-failure",
            &[NAME, other_name],
        );
        let requirement = HilRequirement {
            scenario: "bluetooth-trouble-secure-gatt-hci-read-failure".into(),
            checks: vec![NAME.into(), other_name.into()],
            minimum_repetitions: 1,
            bounds: Vec::new(),
        };
        let mut index = HilEvidenceIndex::synthetic(&[(&requirement.scenario, 1)]);
        let entries = index.scenarios.get_mut(&requirement.scenario).unwrap();
        entries[0].measurements = vec![vec![observed(NAME, 1)]];
        let mut other = entries[0].clone();
        other.run_id = "another-run".into();
        other.measurements[0][0]["name"] = other_name.into();
        entries.push(other);
        for check in &requirement.checks {
            let single = HilRequirement {
                checks: vec![check.clone()],
                ..requirement.clone()
            };
            assert!(index.evidence_for(&single, &catalog).is_some());
        }
        assert!(index.evidence_for(&requirement, &catalog).is_none());
    }
}
