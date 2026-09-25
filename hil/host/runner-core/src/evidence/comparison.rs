//! Descriptive deltas for explicit control/experiment pairs in one run.
//!
//! Absolute workload checks remain authoritative. These observations never
//! assign relative PASS, statistical significance or product qualification.

use serde::Serialize;

use super::run::{MeasurementUnit, MeasurementVerdict, Outcome, ScenarioResult};
use crate::scenario::Scenario;

#[derive(Debug, Serialize)]
pub struct Report {
    schema: u16,
    non_regression: &'static str,
    comparisons: Vec<Pair>,
}

#[derive(Debug, Serialize)]
struct Pair {
    control: String,
    experiment: String,
    observation: Observation,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
enum Observation {
    Measured { metrics: Vec<Delta> },
    Unavailable { reason: String },
}

#[derive(Debug, Serialize)]
struct Delta {
    name: &'static str,
    unit: MeasurementUnit,
    control_values: Vec<u64>,
    experiment_values: Vec<u64>,
    /// Experiment minus control arithmetic means. Raw samples remain exact.
    mean_delta: f64,
    /// Undefined when the control mean is zero; never substituted with zero.
    mean_delta_percent: Option<f64>,
}

pub fn collect(selected: &[&Scenario], results: &[ScenarioResult]) -> Option<Report> {
    let comparisons = selected
        .iter()
        .filter_map(|experiment| {
            let relation = experiment.comparison.as_ref()?;
            let control = relation.control();
            let observation = observe(
                results.iter().find(|result| result.scenario == control),
                results
                    .iter()
                    .find(|result| result.scenario == experiment.id),
            )
            .unwrap_or_else(|reason| Observation::Unavailable { reason });
            Some(Pair {
                control: control.to_owned(),
                experiment: experiment.id.clone(),
                observation,
            })
        })
        .collect::<Vec<_>>();
    (!comparisons.is_empty()).then_some(Report {
        schema: 1,
        non_regression: "not-evaluated",
        comparisons,
    })
}

fn observe(
    control: Option<&ScenarioResult>,
    experiment: Option<&ScenarioResult>,
) -> Result<Observation, String> {
    let (Some(control), Some(experiment)) = (control, experiment) else {
        return Err("control or experiment was not executed in this run".into());
    };
    if control.image != experiment.image
        || control.required_repetitions != experiment.required_repetitions
    {
        return Err("control and experiment have different images or repetition counts".into());
    }
    for result in [control, experiment] {
        if result.outcome != Outcome::Passed
            || result.failure.is_some()
            || result.repetitions.len() != usize::from(result.required_repetitions)
            || result.repetitions.is_empty()
            || result
                .repetitions
                .iter()
                .enumerate()
                .any(|(index, repetition)| {
                    repetition.repetition as usize != index + 1
                        || repetition.outcome != Outcome::Passed
                        || repetition.failure.is_some()
                })
        {
            return Err(format!(
                "{} did not complete all absolute checks and cleanup",
                result.scenario
            ));
        }
    }
    let metrics = [
        ("udp.rx.target-rate", MeasurementUnit::BitsPerSecond),
        ("udp.rx.host-offer-rate", MeasurementUnit::BitsPerSecond),
        ("udp.rx.maximum-silence", MeasurementUnit::Microseconds),
    ]
    .into_iter()
    .map(|(name, unit)| {
        let values = |result: &ScenarioResult| -> Result<Vec<u64>, String> {
            result
                .repetitions
                .iter()
                .map(|repetition| {
                    let mut matches = repetition
                        .measurements
                        .iter()
                        .filter(|measurement| measurement.name == name);
                    let measurement = matches.next().ok_or_else(|| {
                        format!(
                            "{} repetition {} lacks {name}",
                            result.scenario, repetition.repetition
                        )
                    })?;
                    if matches.next().is_some()
                        || measurement.unit != unit
                        || measurement.verdict == Some(MeasurementVerdict::Failed)
                    {
                        return Err(format!(
                            "{} has ambiguous, failed or incorrectly dimensioned {name}",
                            result.scenario
                        ));
                    }
                    Ok(measurement.value)
                })
                .collect()
        };
        let control_values = values(control)?;
        let experiment_values = values(experiment)?;
        let baseline: u128 = control_values.iter().map(|value| u128::from(*value)).sum();
        let measured: u128 = experiment_values
            .iter()
            .map(|value| u128::from(*value))
            .sum();
        // At most 20 u64 samples: both sums and the signed difference fit i128.
        let difference = measured as i128 - baseline as i128;
        Ok(Delta {
            name,
            unit,
            mean_delta: difference as f64 / control_values.len() as f64,
            mean_delta_percent: (baseline != 0)
                .then(|| difference as f64 * 100.0 / baseline as f64),
            control_values,
            experiment_values,
        })
    })
    .collect::<Result<Vec<_>, String>>()?;
    Ok(Observation::Measured { metrics })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        evidence::run::{Measurement, RUN_SCHEMA, RepetitionResult},
        image::ImageClass,
    };

    fn result(name: &str, rate: u64, silence: u64) -> ScenarioResult {
        ScenarioResult::from_repetitions(
            name.into(),
            ImageClass::DiagnosticRxDelivery,
            1,
            vec![RepetitionResult {
                schema: RUN_SCHEMA,
                repetition: 1,
                outcome: Outcome::Passed,
                started_unix_millis: 0,
                duration_millis: 1,
                artifact_directory: "sample".into(),
                attachments: vec![],
                failure: None,
                measurements: vec![
                    Measurement::observed(
                        "udp.rx.target-rate",
                        rate,
                        MeasurementUnit::BitsPerSecond,
                    ),
                    Measurement::observed(
                        "udp.rx.host-offer-rate",
                        100,
                        MeasurementUnit::BitsPerSecond,
                    ),
                    Measurement::observed(
                        "udp.rx.maximum-silence",
                        silence,
                        MeasurementUnit::Microseconds,
                    ),
                ],
            }],
        )
    }

    #[test]
    fn reports_signed_deltas_without_a_relative_verdict() {
        let control = result("control", 100, 0);
        let experiment = result("experiment", 96, 12);
        let Observation::Measured { metrics } = observe(Some(&control), Some(&experiment)).unwrap()
        else {
            panic!("missing observations")
        };
        assert_eq!(metrics[0].mean_delta, -4.0);
        assert_eq!(metrics[0].mean_delta_percent, Some(-4.0));
        assert_eq!(metrics[2].mean_delta, 12.0);
        assert_eq!(metrics[2].mean_delta_percent, None);
        let report = serde_json::to_value(Observation::Measured { metrics }).unwrap();
        assert!(report.get("verdict").is_none());
    }

    #[test]
    fn missing_failed_incomplete_or_ambiguous_observations_are_not_comparable() {
        let control = result("control", 100, 1);
        let experiment = result("experiment", 96, 12);
        assert!(observe(None, Some(&experiment)).is_err());
        let mut failed = control.clone();
        failed.outcome = Outcome::Failed;
        assert!(observe(Some(&failed), Some(&experiment)).is_err());
        let mut partial = control.clone();
        partial.repetitions.clear();
        assert!(observe(Some(&partial), Some(&experiment)).is_err());
        let mut missing = control.clone();
        missing.repetitions[0].measurements.pop();
        assert!(observe(Some(&missing), Some(&experiment)).is_err());
        let mut duplicate = control;
        let repeated = duplicate.repetitions[0].measurements[0].clone();
        duplicate.repetitions[0].measurements.push(repeated);
        assert!(observe(Some(&duplicate), Some(&experiment)).is_err());
        let mut inconsistent = result("control", 100, 1);
        inconsistent.repetitions[0].measurements[0].verdict = Some(MeasurementVerdict::Failed);
        assert!(observe(Some(&inconsistent), Some(&experiment)).is_err());
    }
}
