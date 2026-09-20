//! Independent structural validation of the current run measurement contract.

use super::*;

#[derive(Deserialize)]
struct Measurement {
    name: String,
    value: u64,
    unit: Unit,
    threshold: Option<Threshold>,
    verdict: Option<Verdict>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Unit {
    Count,
    Bytes,
    BitsPerSecond,
    Microseconds,
    BasisPoints,
}

#[derive(Deserialize)]
struct Threshold {
    comparison: Comparison,
    value: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Comparison {
    AtLeast,
    AtMost,
    Exactly,
}

#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
enum Verdict {
    Passed,
    Failed,
}

pub(super) fn validate(values: &[serde_json::Value], outcome: Outcome) -> Result<()> {
    let mut names = BTreeSet::new();
    for value in values {
        let measurement: Measurement = serde_json::from_value(value.clone())?;
        let Measurement {
            name,
            value,
            unit,
            threshold,
            verdict,
        } = measurement;
        // Deserialization checks the unit even for observations without a gate.
        let _ = unit;
        if name.is_empty()
            || name.len() > 128
            || !name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'-'))
            || !names.insert(name)
        {
            return Err("invalid or duplicate HIL measurement name".into());
        }
        match (threshold, verdict) {
            (None, None) => {}
            (Some(threshold), Some(verdict)) => {
                let passed = match threshold.comparison {
                    Comparison::AtLeast => value >= threshold.value,
                    Comparison::AtMost => value <= threshold.value,
                    Comparison::Exactly => value == threshold.value,
                };
                if passed != (verdict == Verdict::Passed) {
                    return Err(
                        "HIL measurement verdict contradicts its value and threshold".into(),
                    );
                }
                if !passed
                    && !matches!(
                        outcome,
                        Outcome::Failed | Outcome::Broken | Outcome::Interrupted
                    )
                {
                    return Err("failed HIL measurement contradicts repetition outcome".into());
                }
            }
            _ => return Err("HIL measurement needs both threshold and verdict, or neither".into()),
        }
    }
    Ok(())
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
