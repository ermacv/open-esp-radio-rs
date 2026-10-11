//! Bounds a HIL requirement places on recorded measurement values.
//!
//! A scenario defines its experiment and its own success; qualification
//! bounds what the experiment observed (#363). A bound names each measurement
//! with its full identity, so a renamed or retyped measurement is missing,
//! never a silent pass. It is judged on every current applicable run of its
//! scenario together, never on a selected one (owner decision on #363): it
//! holds in each of their repetitions, or on the sums over all of them. A
//! ratio needs a minimum denominator, below which the evidence is
//! insufficient, never a pass. A bound changes no run's closure: collected
//! evidence stays current when it changes.

use std::fmt;

use oer_hil_schema::run::{Better, MeasurementUnit};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Result;

/// The ratio limit's scale: basis points of the denominator.
const BASIS_POINTS: u64 = 10_000;

/// One measurement as a bound names it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) struct Metric {
    pub(crate) name: String,
    pub(crate) unit: MeasurementUnit,
    pub(crate) semantics: u16,
    /// The direction its producer declares, if any.
    pub(crate) better: Option<Better>,
}

/// A ratio's two measurements.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) struct RatioDocument {
    pub(crate) numerator: Metric,
    pub(crate) denominator: Metric,
}

/// Where a bound holds.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Over {
    /// Every repetition of every current run on its own.
    Repetition,
    /// The sum over every repetition of every current run.
    CurrentRuns,
}

/// A bound as a catalog writes it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) struct BoundDocument {
    pub(crate) measurement: Option<Metric>,
    pub(crate) ratio: Option<RatioDocument>,
    pub(crate) over: Over,
    pub(crate) at_most: Option<u64>,
    pub(crate) at_least: Option<u64>,
    pub(crate) at_most_basis_points: Option<u64>,
    pub(crate) at_least_basis_points: Option<u64>,
    pub(crate) minimum_denominator: Option<u64>,
}

/// What a bound limits.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Quantity {
    Value(Metric),
    Ratio {
        numerator: Metric,
        denominator: Metric,
        /// The least denominator that decides the ratio.
        minimum_denominator: u64,
    },
}

/// The limit, in the measurement's unit or, for a ratio, in basis points.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Limit {
    AtMost(u64),
    AtLeast(u64),
}

/// A validated bound.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Bound {
    pub(crate) quantity: Quantity,
    pub(crate) over: Over,
    pub(crate) limit: Limit,
}

impl BoundDocument {
    /// The bound, or why the catalog's declaration is not one.
    pub(crate) fn validated(&self, context: &str) -> Result<Bound> {
        let invalid = |reason: &str| format!("HIL bound of {context}: {reason}");
        let limit = match (
            self.at_most,
            self.at_least,
            self.at_most_basis_points,
            self.at_least_basis_points,
        ) {
            (Some(limit), None, None, None) | (None, None, Some(limit), None) => {
                Limit::AtMost(limit)
            }
            (None, Some(limit), None, None) | (None, None, None, Some(limit)) => {
                Limit::AtLeast(limit)
            }
            _ => return Err(invalid("needs exactly one limit").into()),
        };
        let in_basis_points =
            self.at_most_basis_points.is_some() || self.at_least_basis_points.is_some();
        let quantity = match (&self.measurement, &self.ratio) {
            (Some(metric), None) => {
                // A sum of a value grows with every current run, so its
                // verdict would follow how often the scenario ran.
                if self.over == Over::CurrentRuns {
                    return Err(invalid(
                        "a single measurement holds per repetition; sum it as a ratio",
                    )
                    .into());
                }
                if in_basis_points || self.minimum_denominator.is_some() {
                    return Err(invalid(
                        "a measurement's limit is in its own unit and has no denominator",
                    )
                    .into());
                }
                metric.validated(context)?;
                Quantity::Value(metric.clone())
            }
            (None, Some(ratio)) => {
                if !in_basis_points {
                    return Err(invalid("a ratio's limit is in basis points").into());
                }
                let Some(minimum_denominator) = self.minimum_denominator.filter(|&m| m > 0) else {
                    return Err(invalid("a ratio needs a positive minimum-denominator").into());
                };
                ratio.numerator.validated(context)?;
                ratio.denominator.validated(context)?;
                if ratio.numerator.name == ratio.denominator.name {
                    return Err(invalid("a ratio names two measurements").into());
                }
                if ratio.numerator.unit != ratio.denominator.unit {
                    return Err(invalid("a ratio's measurements share their unit").into());
                }
                Quantity::Ratio {
                    numerator: ratio.numerator.clone(),
                    denominator: ratio.denominator.clone(),
                    minimum_denominator,
                }
            }
            _ => return Err(invalid("needs exactly one of measurement and ratio").into()),
        };
        if let (Quantity::Ratio { .. }, Limit::AtMost(l) | Limit::AtLeast(l)) = (&quantity, limit)
            && l > BASIS_POINTS
        {
            return Err(invalid("a ratio's limit is at most 10000 basis points").into());
        }
        let bounded = match &quantity {
            Quantity::Value(metric)
            | Quantity::Ratio {
                numerator: metric, ..
            } => metric,
        };
        let direction = match limit {
            Limit::AtMost(_) => Better::Lower,
            Limit::AtLeast(_) => Better::Higher,
        };
        if bounded.better != Some(direction) {
            return Err(invalid(&format!(
                "`{}` must declare better = \"{}\" for this limit",
                bounded.name,
                direction.id()
            ))
            .into());
        }
        Ok(Bound {
            quantity,
            over: self.over,
            limit,
        })
    }
}

impl Metric {
    fn validated(&self, context: &str) -> Result<()> {
        if self.name.is_empty() || self.semantics == 0 {
            return Err(format!(
                "HIL bound of {context}: a measurement needs its name and a semantics version"
            )
            .into());
        }
        Ok(())
    }

    /// This measurement's value among one repetition's, if the repetition
    /// recorded it exactly once with this identity.
    fn value(&self, measurements: &[Value]) -> Option<u64> {
        let mut named = measurements
            .iter()
            .filter(|m| m.get("name").and_then(Value::as_str) == Some(&self.name));
        let measurement = named.next()?;
        if named.next().is_some() {
            return None;
        }
        let unit = serde_json::from_value::<MeasurementUnit>(measurement.get("unit")?.clone());
        let better = match measurement.get("better") {
            None | Some(Value::Null) => None,
            Some(better) => Some(serde_json::from_value::<Better>(better.clone()).ok()?),
        };
        (unit.ok()? == self.unit
            && measurement.get("semantics").and_then(Value::as_u64) == Some(self.semantics.into())
            && better == self.better)
            .then(|| measurement.get("value").and_then(Value::as_u64))
            .flatten()
    }
}

/// What a bound found in the current runs.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Assessment {
    Passed,
    Failed,
    /// A repetition lacks a named measurement, or records it with another
    /// identity.
    Unavailable,
    /// A ratio's denominator stays below its minimum.
    InsufficientSample,
}

/// One bound's finding, with the values it judged.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct Finding {
    pub(crate) bound: String,
    pub(crate) assessment: Assessment,
    /// The judged value of each repetition, or of their sum: the value, or
    /// the numerator and the denominator.
    pub(crate) values: Vec<(u64, Option<u64>)>,
}

impl Bound {
    /// Whether every one of a run's `repetitions` recorded this bound's
    /// measurements with their identity, so the run can join the judged
    /// set.
    pub(crate) fn available(&self, repetitions: &[Vec<Value>]) -> bool {
        !repetitions.is_empty()
            && repetitions
                .iter()
                .all(|measurements| self.sample(measurements).is_some())
    }

    /// Judge the repetitions of every current applicable run together.
    pub(crate) fn assess(&self, repetitions: &[Vec<Value>]) -> Finding {
        let finding = |assessment, values| Finding {
            bound: self.to_string(),
            assessment,
            values,
        };
        let Some(per_repetition) = repetitions
            .iter()
            .map(|measurements| self.sample(measurements))
            .collect::<Option<Vec<_>>>()
            .filter(|values| !values.is_empty())
        else {
            return finding(Assessment::Unavailable, Vec::new());
        };
        let judged = match self.over {
            Over::Repetition => per_repetition,
            Over::CurrentRuns => {
                let sum = per_repetition
                    .iter()
                    .try_fold((0u64, Some(0u64)), |sum, value| {
                        Some((
                            sum.0.checked_add(value.0)?,
                            match (sum.1, value.1) {
                                (Some(a), Some(b)) => Some(a.checked_add(b)?),
                                _ => None,
                            },
                        ))
                    });
                let Some(sum) = sum else {
                    return finding(Assessment::Unavailable, Vec::new());
                };
                vec![sum]
            }
        };
        // A value outside the limit decides: more runs cannot undo it in its
        // repetition, so it outranks a sample still too small elsewhere.
        let assessments = judged
            .iter()
            .map(|&(value, denominator)| self.judge(value, denominator))
            .collect::<Vec<_>>();
        let assessment = [
            Assessment::Failed,
            Assessment::Unavailable,
            Assessment::InsufficientSample,
        ]
        .into_iter()
        .find(|worst| assessments.contains(worst))
        .unwrap_or(Assessment::Passed);
        finding(assessment, judged)
    }

    fn sample(&self, measurements: &[Value]) -> Option<(u64, Option<u64>)> {
        match &self.quantity {
            Quantity::Value(metric) => Some((metric.value(measurements)?, None)),
            Quantity::Ratio {
                numerator,
                denominator,
                ..
            } => Some((
                numerator.value(measurements)?,
                Some(denominator.value(measurements)?),
            )),
        }
    }

    fn judge(&self, value: u64, denominator: Option<u64>) -> Assessment {
        let passed = match (&self.quantity, denominator) {
            (Quantity::Value(_), None) => match self.limit {
                Limit::AtMost(limit) => value <= limit,
                Limit::AtLeast(limit) => value >= limit,
            },
            (
                Quantity::Ratio {
                    minimum_denominator,
                    ..
                },
                Some(denominator),
            ) => {
                if denominator < *minimum_denominator {
                    return Assessment::InsufficientSample;
                }
                // value / denominator against limit / 10000, without
                // division.
                let scaled = u128::from(value) * u128::from(BASIS_POINTS);
                match self.limit {
                    Limit::AtMost(limit) => scaled <= u128::from(limit) * u128::from(denominator),
                    Limit::AtLeast(limit) => scaled >= u128::from(limit) * u128::from(denominator),
                }
            }
            _ => return Assessment::Unavailable,
        };
        if passed {
            Assessment::Passed
        } else {
            Assessment::Failed
        }
    }
}

impl fmt::Display for Bound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (comparison, limit) = match self.limit {
            Limit::AtMost(limit) => ("<=", limit),
            Limit::AtLeast(limit) => (">=", limit),
        };
        let over = match self.over {
            Over::Repetition => "repetition",
            Over::CurrentRuns => "current-runs",
        };
        match &self.quantity {
            Quantity::Value(metric) => write!(
                f,
                "{}{comparison}{limit}{}/{over}",
                metric.name,
                metric.unit.id()
            ),
            Quantity::Ratio {
                numerator,
                denominator,
                minimum_denominator,
            } => write!(
                f,
                "{}/{}{comparison}{limit}bp/{over}/min{minimum_denominator}",
                numerator.name, denominator.name
            ),
        }
    }
}

#[cfg(test)]
mod tests;
