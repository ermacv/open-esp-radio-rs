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

impl Contract {
    pub(super) fn image_sensitive(&self) -> bool {
        self.unit != "count"
    }
}

/// Whole-scenario obligations include mandatory memory/timing assertions, even
/// when those assertions have not yet been published as individually named checks.
pub(super) fn whole_scenario_image_sensitive(document: &Value) -> bool {
    let workload = document.pointer("/workload/kind").and_then(Value::as_str);
    matches!(
        workload,
        Some(
            "bluetooth-gatt"
                | "bluetooth-secure-gatt"
                | "bluetooth-secure-gatt-timing"
                | "bluetooth-secure-gatt-hci-read-failure"
                | "memory-benchmark"
                | "timebase"
                | "boot-smoke"
        )
    ) || [
        "minimum_rx_bps",
        "minimum_tx_bps",
        "minimum_combined_bps",
        "minimum_bps_per_flow",
        "maximum_flow_skew_percent",
        "maximum_secondary_tx_interarrival_ms",
        "maximum_rx_silence_ms",
        "maximum_p95_ms",
    ]
    .iter()
    .any(|key| {
        document
            .get("criteria")
            .and_then(|c| c.get(key))
            .is_some_and(|v| !v.is_null())
    })
}

pub(super) fn contracts(document: &Value) -> Result<BTreeMap<String, Contract>> {
    let mut checks = BTreeMap::new();
    let kind = document.pointer("/workload/kind").and_then(Value::as_str);
    if matches!(kind, Some("station-ap-loss" | "station-ap-absence")) {
        let mut names = vec!["wifi.station.control-responsive"];
        if kind == Some("station-ap-loss") {
            names.push("wifi.station.ap-loss-reconnected");
            if document
                .pointer("/workload/require_recovery_echo")
                .and_then(Value::as_bool)
                == Some(true)
            {
                names.push("wifi.station.recovered-ip-exchange");
            }
        } else {
            names.push(
                if document
                    .pointer("/workload/initially_absent")
                    .and_then(Value::as_bool)
                    == Some(true)
                {
                    "wifi.station.initial-retry-exhausted"
                } else {
                    "wifi.station.recovery-retry-exhausted"
                },
            );
        }
        for name in names {
            checks.insert(
                name.into(),
                Contract {
                    unit: "count",
                    comparison: "exactly",
                    threshold: 1,
                },
            );
        }
        return Ok(checks);
    }
    if document.pointer("/workload/kind").and_then(Value::as_str) != Some("udp")
        || document
            .pointer("/workload/direction")
            .and_then(Value::as_str)
            != Some("rx")
    {
        return Ok(checks);
    }
    if let Some(floor) = document
        .pointer("/criteria/minimum_rx_bps")
        .and_then(Value::as_u64)
    {
        for (name, threshold) in [
            ("udp.rx.target-rate", floor / 1_000 * 1_000),
            ("udp.rx.host-offer-rate", floor),
        ] {
            checks.insert(
                name.into(),
                Contract {
                    unit: "bits-per-second",
                    comparison: "at-least",
                    threshold,
                },
            );
        }
    }
    if let Some(limit) = document
        .pointer("/criteria/maximum_rx_silence_ms")
        .and_then(Value::as_u64)
    {
        checks.insert(
            "udp.rx.maximum-silence".into(),
            Contract {
                unit: "microseconds",
                comparison: "at-most",
                threshold: limit
                    .checked_mul(1_000)
                    .ok_or("RX silence criterion overflows microseconds")?,
            },
        );
    }
    if document
        .pointer("/workload/station_pause")
        .is_some_and(|operation| !operation.is_null())
    {
        for name in [
            "wifi.maintenance.transaction-valid",
            "wifi.maintenance.same-link",
        ] {
            checks.insert(
                name.into(),
                Contract {
                    unit: "count",
                    comparison: "exactly",
                    threshold: 1,
                },
            );
        }
    }
    if document
        .pointer("/criteria/require_post_maintenance_echo")
        .and_then(Value::as_bool)
        == Some(true)
        && document
            .pointer("/workload/station_pause")
            .is_some_and(|v| !v.is_null())
    {
        checks.insert(
            "wifi.maintenance.ip-exchange-resumed".into(),
            Contract {
                unit: "count",
                comparison: "exactly",
                threshold: 1,
            },
        );
    }
    Ok(checks)
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

#[cfg(test)]
fn passes(name: &str, contract: &Contract, measurements: &[Value]) -> bool {
    assess(name, contract, measurements) == Assessment::Passed
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn catalog() -> ScenarioCatalog {
        ScenarioCatalog::load(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .as_path(),
            Path::new("hil/scenarios"),
        )
        .unwrap()
    }

    fn observed(value: u64) -> Value {
        json!({"name": "wifi.maintenance.same-link", "value": value, "unit": "count", "threshold": {"comparison": "exactly", "value": 1}, "verdict": "passed"})
    }

    #[test]
    fn ap_start_failure_is_distinct_from_recovery_and_control_liveness() {
        let initial = contracts(
            &serde_json::json!({"workload":{"kind":"station-ap-absence","initially_absent":true}}),
        )
        .unwrap();
        let recovered =
            contracts(&serde_json::json!({"workload":{"kind":"station-ap-absence"}})).unwrap();
        assert!(initial.contains_key("wifi.station.initial-retry-exhausted"));
        assert!(!initial.contains_key("wifi.station.recovery-retry-exhausted"));
        assert!(recovered.contains_key("wifi.station.recovery-retry-exhausted"));
        let name = "wifi.station.initial-retry-exhausted";
        let unrelated = serde_json::json!({"name":"wifi.station.control-responsive","value":1,"unit":"count","threshold":{"comparison":"exactly","value":1},"verdict":"passed"});
        assert_eq!(
            assess(name, &initial[name], &[unrelated]),
            Assessment::Unavailable
        );
    }

    #[test]
    fn unchanged_link_does_not_substitute_for_post_maintenance_exchange() {
        let name = "wifi.maintenance.ip-exchange-resumed";
        let mut document = json!({"workload":{"kind":"udp","direction":"rx","station_pause":"calibration"},
            "criteria":{"require_post_maintenance_echo":true}});
        let contract = contracts(&document).unwrap();
        assert!(contract.contains_key(name));
        assert!(!passes(name, &contract[name], &[observed(1)]));
        let mut echo = observed(1);
        echo["name"] = json!(name);
        assert!(passes(name, &contract[name], &[echo]));
        document["criteria"]["require_post_maintenance_echo"] = json!(false);
        assert!(!contracts(&document).unwrap().contains_key(name));
    }

    #[test]
    fn independently_checks_values_units_thresholds_duplicates_and_missing_data() {
        let contracts = contracts(&json!({"workload": {"kind": "udp", "direction": "rx", "station_pause": "calibration"}})).unwrap();
        let name = "wifi.maintenance.same-link";
        let contract = &contracts[name];
        assert!(passes(name, contract, &[observed(1)]));
        assert!(!passes(name, contract, &[observed(0)]));
        assert!(!passes(name, contract, &[]));
        assert!(!passes(name, contract, &[observed(1), observed(1)]));
        for changed in [
            json!({"unit": "bytes"}),
            json!({"verdict": null}),
            json!({"threshold": {"comparison": "at-least", "value": 2}}),
        ] {
            let mut sample = observed(1);
            sample
                .as_object_mut()
                .unwrap()
                .extend(changed.as_object().unwrap().clone());
            assert!(!passes(name, contract, &[sample]));
        }
    }

    #[test]
    fn criteria_are_recomputed_without_rewriting_the_original_assessment() {
        let original = json!({"name":"udp.rx.target-rate", "value":90_000_000,
            "unit":"bits-per-second", "threshold":{"comparison":"at-least","value":95_000_000},
            "verdict":"failed"});
        let measurements = vec![original.clone()];
        for (floor, expected) in [(85_000_000, true), (90_000_000, true), (95_000_000, false)] {
            let contracts = contracts(&json!({"workload":{"kind":"udp","direction":"rx"},
                "criteria":{"minimum_rx_bps":floor}}))
            .unwrap();
            assert_eq!(
                passes(
                    "udp.rx.target-rate",
                    &contracts["udp.rx.target-rate"],
                    &measurements
                ),
                expected
            );
        }
        assert_eq!(measurements[0], original);
        // Reassessing one number is not authorization to turn a failed
        // scenario/lifecycle into a successful qualification observation.
    }

    #[test]
    fn every_repetition_must_contain_the_requested_proof() {
        let catalog = catalog();
        let requirement = HilRequirement {
            scenario: "diagnostic-station-phy-rxcal-delivery-rx".into(),
            checks: vec!["wifi.maintenance.same-link".into()],
            minimum_repetitions: 3,
        };
        catalog.validate_requirement(&requirement).unwrap();
        let mut index = HilEvidenceIndex::synthetic(&[(&requirement.scenario, 3)]);
        assert!(index.evidence_for(&requirement, &catalog).is_none());
        index.scenarios.get_mut(&requirement.scenario).unwrap()[0].measurements =
            vec![vec![observed(1)]; 3];
        assert!(
            index
                .evidence_for(&requirement, &catalog)
                .unwrap()
                .contains(":checks=wifi.maintenance.same-link")
        );
        index.scenarios.get_mut(&requirement.scenario).unwrap()[0].measurements[1].clear();
        assert!(index.evidence_for(&requirement, &catalog).is_none());
    }

    #[test]
    fn scope_and_duplicate_requirements_fail_static_validation() {
        let catalog = catalog();
        let mut requirement = HilRequirement {
            scenario: "udp-rx-he20-ceiling".into(),
            checks: vec!["wifi.maintenance.same-link".into()],
            minimum_repetitions: 1,
        };
        assert!(catalog.validate_requirement(&requirement).is_err());
        requirement.checks = vec!["udp.rx.target-rate".into()];
        catalog.validate_requirement(&requirement).unwrap();
        requirement.checks.push("udp.rx.target-rate".into());
        assert!(catalog.validate_requirement(&requirement).is_err());
    }

    #[test]
    fn individual_proofs_from_different_runs_do_not_satisfy_one_obligation() {
        let catalog = catalog();
        let requirement = HilRequirement {
            scenario: "diagnostic-station-phy-rxcal-delivery-rx".into(),
            checks: vec![
                "wifi.maintenance.same-link".into(),
                "wifi.maintenance.transaction-valid".into(),
            ],
            minimum_repetitions: 1,
        };
        let mut index = HilEvidenceIndex::synthetic(&[(&requirement.scenario, 1)]);
        let entries = index.scenarios.get_mut(&requirement.scenario).unwrap();
        entries[0].measurements = vec![vec![observed(1)]];
        let mut other = entries[0].clone();
        other.run_id = "another-run".into();
        other.measurements[0][0]["name"] = "wifi.maintenance.transaction-valid".into();
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
