//! The comparison of an A/B experiment's arms.
//!
//! Each run contributes one value per measurement, the mean of its
//! repetitions' [`samples`], so drift between rounds falls on the arms alike;
//! each measurement is judged with the one noise-aware [`samples::compare`].

use std::collections::BTreeMap;

use oer_hil_run_bundle::experiment::Arm;
use serde::Serialize;

use crate::{
    run::Run,
    samples::{self, Better, Compared},
};

/// One compared measurement.
#[derive(Clone, Debug, Serialize)]
pub struct MeasurementComparison {
    pub scenario: String,
    pub measurement: String,
    pub unit: String,
    /// Which way the measurement's gate prefers; `None` for an ungated
    /// figure, whose significant differences are reported as changes.
    pub better: Option<Better>,
    pub comparison: Compared,
}

/// Per (scenario, measurement): its unit, its gate's direction and each
/// arm's run means.
type ArmMeans = BTreeMap<(String, String), (String, Option<Better>, Vec<f64>, Vec<f64>)>;

/// The comparison of every measurement both arms measured.
pub fn compare(runs: &[(Arm, &Run)]) -> Vec<MeasurementComparison> {
    let mut means = ArmMeans::new();
    for (arm, run) in runs {
        let Some(suite) = &run.suite else {
            continue;
        };
        for sample in samples::samples(suite) {
            let Some(spread) = sample.spread() else {
                continue;
            };
            let entry = means
                .entry((sample.scenario.clone(), sample.measurement.clone()))
                .or_insert_with(|| {
                    (
                        sample.unit.to_string(),
                        sample.better(),
                        Vec::new(),
                        Vec::new(),
                    )
                });
            match arm {
                Arm::A => entry.2.push(spread.mean),
                Arm::B => entry.3.push(spread.mean),
            }
        }
    }
    means
        .into_iter()
        .filter_map(|((scenario, measurement), (unit, better, a, b))| {
            Some(MeasurementComparison {
                comparison: samples::compare(better, &a, &b)?,
                scenario,
                measurement,
                unit,
                better,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests;
