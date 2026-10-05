//! Validation of recorded measurements against the run bundle's contract.

use super::*;

/// Validate recorded measurement values (a run's or a shard's) against the
/// run bundle's one measurement contract: each must be a measurement of
/// this build's vocabulary, and together they must pass
/// [`oer_hil_run_bundle_format::run::validation::validate_measurements`].
pub(super) fn validate(values: &[serde_json::Value], outcome: Outcome) -> Result<()> {
    let measurements = values
        .iter()
        .map(|value| serde_json::from_value(value.clone()))
        .collect::<std::result::Result<Vec<oer_hil_run_bundle_format::run::Measurement>, _>>()?;
    oer_hil_run_bundle_format::run::validation::validate_measurements(&measurements, outcome)
        .map_err(|error| error.to_string().into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_measurement_contract_conformance() {
        let cases: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../hil/tests/fixtures/evidence/measurements.json"
        ))
        .unwrap();
        for case in cases.as_array().unwrap() {
            let outcome = serde_json::from_value(case["outcome"].clone()).unwrap();
            let actual = validate(case["measurements"].as_array().unwrap(), outcome).is_ok();
            assert_eq!(actual, case["valid"].as_bool().unwrap(), "{}", case["name"]);
        }
    }
}
