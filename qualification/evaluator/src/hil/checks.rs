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
    image_sensitive: bool,
}

impl Contract {
    pub(super) fn image_sensitive(&self) -> bool {
        self.image_sensitive
    }
}

/// Whole-scenario obligations include mandatory memory/timing assertions, even
/// when those assertions have not yet been published as individually named checks.
pub(super) fn whole_scenario_image_sensitive(document: &Value) -> bool {
    document.get("transfer").and_then(Value::as_str) == Some("identical-image")
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

pub(super) fn contracts(document: &Value) -> Result<BTreeMap<String, Contract>> {
    let mut checks = BTreeMap::new();
    let workload = document.pointer("/wifi/workload");
    let field = |name: &str| workload.and_then(|workload| workload.get(name));
    let exactly_once = |checks: &mut BTreeMap<String, Contract>, name: &str| {
        checks.insert(
            name.into(),
            Contract {
                unit: "count",
                image_sensitive: false,
                comparison: "exactly",
                threshold: 1,
            },
        );
    };
    let kind = field("kind").and_then(Value::as_str);
    // A station workload induces the protection; an access-point workload
    // observes its own protection toward the OpenWrt client.
    let protection = field("induced_protection")
        .or_else(|| field("protection"))
        .filter(|v| !v.is_null());
    if let Some(induced) = protection {
        let percent = induced
            .get("minimum_protected_ppdu_percent")
            .and_then(Value::as_u64)
            .filter(|percent| (1..=100).contains(percent))
            .ok_or("induced protection names no protected-PPDU floor")?;
        checks.insert(
            "wifi.protection.rts-cts-before-data".into(),
            Contract {
                unit: "basis-points",
                image_sensitive: false,
                comparison: "at-least",
                threshold: percent * 100,
            },
        );
        for name in [
            "wifi.protection.control-rate",
            "wifi.protection.nav-covers-exchange",
        ] {
            checks.insert(
                name.into(),
                Contract {
                    unit: "count",
                    image_sensitive: false,
                    comparison: "exactly",
                    threshold: 0,
                },
            );
        }
    }
    if matches!(kind, Some("station-ap-loss" | "station-ap-absence")) {
        exactly_once(&mut checks, "wifi.station.control-responsive");
        if kind == Some("station-ap-loss") {
            exactly_once(&mut checks, "wifi.station.ap-loss-reconnected");
            if field("require_recovery_echo").and_then(Value::as_bool) == Some(true) {
                exactly_once(&mut checks, "wifi.station.recovered-ip-exchange");
            }
        } else if field("initially_absent").and_then(Value::as_bool) == Some(true) {
            exactly_once(&mut checks, "wifi.station.initial-retry-exhausted");
        } else {
            exactly_once(&mut checks, "wifi.station.recovery-retry-exhausted");
        }
        return Ok(checks);
    }
    if !receive_only_station_udp(document) {
        return Ok(checks);
    }
    let criterion = |name: &str| {
        field("criteria")
            .and_then(|criteria| criteria.get(name))
            .and_then(Value::as_u64)
    };
    if let Some(floor) = criterion("minimum_rx_bps") {
        for (name, threshold) in [
            ("udp.rx.target-rate", floor / 1_000 * 1_000),
            ("udp.rx.host-offer-rate", floor),
        ] {
            checks.insert(
                name.into(),
                Contract {
                    unit: "bits-per-second",
                    image_sensitive: true,
                    comparison: "at-least",
                    threshold,
                },
            );
        }
    }
    if let Some(limit) = criterion("maximum_rx_silence_ms") {
        checks.insert(
            "udp.rx.maximum-silence".into(),
            Contract {
                unit: "microseconds",
                image_sensitive: true,
                comparison: "at-most",
                threshold: limit
                    .checked_mul(1_000)
                    .ok_or("RX silence criterion overflows microseconds")?,
            },
        );
    }
    if let Some(maintenance) = field("maintenance").filter(|v| !v.is_null()) {
        exactly_once(&mut checks, "wifi.maintenance.transaction-valid");
        exactly_once(&mut checks, "wifi.maintenance.same-link");
        if maintenance
            .get("require_post_maintenance_echo")
            .and_then(Value::as_bool)
            == Some(true)
        {
            exactly_once(&mut checks, "wifi.maintenance.ip-exchange-resumed");
        }
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
    fn induced_protection_publishes_its_air_contracts() {
        let published = contracts(&json!({"wifi":{"workload":{"kind":"station-udp",
            "offer":{"tx_bps":1000},"induced_protection":{"peer":"non-ht-member",
            "minimum_protected_ppdu_percent":95}}}}))
        .unwrap();
        let share = &published["wifi.protection.rts-cts-before-data"];
        let sample = |value: u64| {
            json!({"name":"wifi.protection.rts-cts-before-data","value":value,"unit":"basis-points",
                "threshold":{"comparison":"at-least","value":9_500},"verdict":"passed"})
        };
        assert!(passes(
            "wifi.protection.rts-cts-before-data",
            share,
            &[sample(9_600)]
        ));
        assert!(!passes(
            "wifi.protection.rts-cts-before-data",
            share,
            &[sample(9_400)]
        ));
        assert!(published.contains_key("wifi.protection.control-rate"));
        assert!(published.contains_key("wifi.protection.nav-covers-exchange"));
        assert!(
            contracts(&json!({"wifi":{"workload":{"kind":"station-udp",
                "induced_protection":{"peer":"non-ht-member"}}}}))
            .is_err()
        );
        let access_point = contracts(&json!({"wifi":{"workload":{"kind":"access-point",
            "protection":{"minimum_protected_ppdu_percent":90}}}}))
        .unwrap();
        assert_eq!(
            access_point["wifi.protection.rts-cts-before-data"].threshold,
            9_000
        );
    }

    #[test]
    fn ap_start_failure_is_distinct_from_recovery_and_control_liveness() {
        let initial = contracts(
            &serde_json::json!({"wifi":{"workload":{"kind":"station-ap-absence","initially_absent":true}}}),
        )
        .unwrap();
        let recovered =
            contracts(&serde_json::json!({"wifi":{"workload":{"kind":"station-ap-absence"}}}))
                .unwrap();
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
        let mut document = json!({"wifi":{"workload":{"kind":"station-udp","offer":{"rx_bps":1000},
            "maintenance":{"operation":"calibration","require_post_maintenance_echo":true}}}});
        let contract = contracts(&document).unwrap();
        assert!(contract.contains_key(name));
        assert!(!passes(name, &contract[name], &[observed(1)]));
        let mut echo = observed(1);
        echo["name"] = json!(name);
        assert!(passes(name, &contract[name], &[echo]));
        document["wifi"]["workload"]["maintenance"]["require_post_maintenance_echo"] = json!(false);
        assert!(!contracts(&document).unwrap().contains_key(name));
    }

    #[test]
    fn independently_checks_values_units_thresholds_duplicates_and_missing_data() {
        let contracts = contracts(&json!({"wifi": {"workload": {"kind": "station-udp", "offer": {"rx_bps": 1000}, "maintenance": {"operation": "calibration"}}}})).unwrap();
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
            let contracts = contracts(&json!({"wifi":{"workload":{"kind":"station-udp",
                "offer":{"rx_bps":100_000_000},"criteria":{"minimum_rx_bps":floor}}}}))
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
