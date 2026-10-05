//! The one aggregation of a run's measurements and the one comparison of
//! two sets of values.
//!
//! [`samples`] groups a suite's typed measurements by scenario and name,
//! one value per repetition that measured it; every analysis reads figures
//! through it: `runs compare` and `history`, performance reports and
//! baselines, and A/B comparisons. [`compare`] judges two sets of values
//! with Welch's unequal-variance interval and a practical tolerance, and
//! [`change`] a set against a reviewed baseline.

use std::collections::BTreeMap;

use oer_hil_run_bundle::{
    experiment::Arm,
    run::{Comparison, MeasurementUnit, SuiteResult, Threshold},
};
use serde::{Deserialize, Serialize};

/// Which way a gated measurement improves.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Better {
    Higher,
    Lower,
}

/// The gate of a performance figure: the direction an `at-least` or
/// `at-most` threshold prefers and its value. An `exactly` threshold is a
/// correctness check, not a performance figure, and gives no gate.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct Gate {
    pub better: Better,
    pub threshold: f64,
}

impl Gate {
    pub fn of(threshold: &Threshold) -> Option<Self> {
        let value = threshold.value as f64;
        match threshold.comparison {
            Comparison::AtLeast => Some(Self {
                better: Better::Higher,
                threshold: value,
            }),
            Comparison::AtMost => Some(Self {
                better: Better::Lower,
                threshold: value,
            }),
            Comparison::Exactly => None,
        }
    }
}

/// The values of one measurement of one scenario in one run, one per
/// repetition that measured it, in repetition order.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Sample {
    pub scenario: String,
    pub measurement: String,
    pub unit: MeasurementUnit,
    /// The gate of the first repetition that measured it; `None` for an
    /// ungated figure.
    pub gate: Option<Gate>,
    pub values: Vec<f64>,
}

impl Sample {
    pub fn better(&self) -> Option<Better> {
        self.gate.map(|gate| gate.better)
    }

    pub fn spread(&self) -> Option<Spread> {
        Spread::of(&self.values)
    }
}

/// Every measurement of every scenario of `suite`, ordered by scenario and
/// name.
pub fn samples(suite: &SuiteResult) -> Vec<Sample> {
    let mut samples: BTreeMap<(&str, &str), Sample> = BTreeMap::new();
    for scenario in &suite.scenarios {
        for measurement in scenario.repetitions.iter().flat_map(|r| &r.measurements) {
            samples
                .entry((&scenario.scenario, &measurement.name))
                .or_insert_with(|| Sample {
                    scenario: scenario.scenario.clone(),
                    measurement: measurement.name.clone(),
                    unit: measurement.unit,
                    gate: measurement.threshold.as_ref().and_then(Gate::of),
                    values: Vec::new(),
                })
                .values
                .push(measurement.value as f64);
        }
    }
    samples.into_values().collect()
}

/// Mean and sample standard deviation.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct Spread {
    pub mean: f64,
    pub deviation: f64,
    pub count: usize,
}

impl Spread {
    pub fn of(values: &[f64]) -> Option<Self> {
        if values.is_empty() {
            return None;
        }
        let count = values.len();
        let mean = values.iter().sum::<f64>() / count as f64;
        let deviation = if count > 1 {
            (values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (count - 1) as f64).sqrt()
        } else {
            0.0
        };
        Some(Self {
            mean,
            deviation,
            count,
        })
    }
}

/// What a comparison of two sets of values concludes.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum Verdict {
    /// The 95 % confidence interval of the difference excludes zero and the
    /// difference is at least 2 % of A's mean.
    Significant { better: Arm },
    /// As [`Self::Significant`], for a measurement without a gate: nothing
    /// says which direction is better, so only the higher side is named.
    Changed { higher: Arm },
    /// The interval includes zero, or the difference is too small to matter.
    WithinNoise,
    /// Fewer than [`MINIMUM_REPETITIONS`] values on a side.
    InsufficientRepetitions,
}

impl std::fmt::Display for Verdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Significant { better } => write!(f, "significant, {better} better"),
            Self::Changed { higher } => write!(f, "significant change, {higher} higher (ungated)"),
            Self::WithinNoise => f.write_str("within noise"),
            Self::InsufficientRepetitions => f.write_str("insufficient repetitions"),
        }
    }
}

/// The fewest values per side from which a comparison is judged.
pub const MINIMUM_REPETITIONS: usize = 3;

/// A comparison of one measurement: both sides, the difference B − A with
/// its Welch 95 % confidence interval, and the verdict.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct Compared {
    pub a: Spread,
    pub b: Spread,
    pub difference: f64,
    /// Half-width of the 95 % confidence interval of `difference`.
    pub interval: f64,
    pub verdict: Verdict,
}

/// Two-sided 95 % critical value of Student's t for `freedom` degrees of
/// freedom, from the standard table; beyond 30 it approaches the normal 1.96.
fn t_critical_95(freedom: f64) -> f64 {
    const TABLE: [f64; 30] = [
        12.706, 4.303, 3.182, 2.776, 2.571, 2.447, 2.365, 2.306, 2.262, 2.228, 2.201, 2.179, 2.160,
        2.145, 2.131, 2.120, 2.110, 2.101, 2.093, 2.086, 2.080, 2.074, 2.069, 2.064, 2.060, 2.056,
        2.052, 2.048, 2.045, 2.042,
    ];
    // Rounding the Welch degrees of freedom down keeps the interval
    // conservative.
    let index = (freedom.floor() as usize).max(1);
    TABLE
        .get(index - 1)
        .copied()
        .unwrap_or(if index <= 60 { 2.000 } else { 1.960 })
}

/// Relative tolerance below which a change is noise even for perfectly
/// repeatable values.
const MINIMUM_TOLERANCE: f64 = 0.02;

/// Compare the values of sides A and B of one measurement whose better
/// direction is `better` (`None` for an ungated measurement), with Welch's
/// unequal-variance t interval.
pub fn compare(better: Option<Better>, a: &[f64], b: &[f64]) -> Option<Compared> {
    let (spread_a, spread_b) = (Spread::of(a)?, Spread::of(b)?);
    let difference = spread_b.mean - spread_a.mean;
    let (variance_a, variance_b) = (
        spread_a.deviation.powi(2) / spread_a.count as f64,
        spread_b.deviation.powi(2) / spread_b.count as f64,
    );
    let standard_error = (variance_a + variance_b).sqrt();
    let interval = if standard_error == 0.0 {
        0.0
    } else {
        let freedom = (variance_a + variance_b).powi(2)
            / (variance_a.powi(2) / (spread_a.count as f64 - 1.0).max(1.0)
                + variance_b.powi(2) / (spread_b.count as f64 - 1.0).max(1.0));
        t_critical_95(freedom) * standard_error
    };
    let verdict = if spread_a.count < MINIMUM_REPETITIONS || spread_b.count < MINIMUM_REPETITIONS {
        Verdict::InsufficientRepetitions
    } else if difference.abs() <= interval
        || difference.abs() < MINIMUM_TOLERANCE * spread_a.mean.abs()
    {
        Verdict::WithinNoise
    } else {
        let arm = |b_wins: bool| if b_wins { Arm::B } else { Arm::A };
        match better {
            Some(Better::Higher) => Verdict::Significant {
                better: arm(difference > 0.0),
            },
            Some(Better::Lower) => Verdict::Significant {
                better: arm(difference < 0.0),
            },
            None => Verdict::Changed {
                higher: arm(difference > 0.0),
            },
        }
    };
    Some(Compared {
        a: spread_a,
        b: spread_b,
        difference,
        interval,
        verdict,
    })
}

/// How a figure compares with its reviewed baseline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Change {
    Improved,
    Within,
    Regressed,
}

/// Compare `current` with a baseline `reference` of a figure that `better`
/// prefers: a move in the worse direction by more than twice the baseline's
/// deviation and 2 % of its mean is a regression; the same margin in the
/// better direction is an improvement.
pub fn change(reference: &Spread, better: Better, current: &Spread) -> Change {
    let margin = (2.0 * reference.deviation).max(MINIMUM_TOLERANCE * reference.mean.abs());
    let delta = current.mean - reference.mean;
    let worse = match better {
        Better::Higher => -delta,
        Better::Lower => delta,
    };
    if worse > margin {
        Change::Regressed
    } else if -worse > margin {
        Change::Improved
    } else {
        Change::Within
    }
}

#[cfg(test)]
pub(crate) mod tests;
