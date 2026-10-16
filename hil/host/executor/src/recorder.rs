//! What one scenario run counts besides its trace.

use core::{fmt, num::NonZeroU32};
use std::collections::BTreeMap;

use oer_hil_model::{CountName, Operation};
use oer_hil_run_bundle_format::run::{Measurement, MeasurementUnit};
use serde::Serialize;

/// The measurement that counts infrastructure retries; no count or
/// measurement may take its name.
pub const INFRASTRUCTURE_RETRIES: &str = "infrastructure.retries";

/// One retry of infrastructure: an operation of the stand or the link,
/// not of the protocol under test, repeated because its first attempt
/// failed. Every one is recorded and counted; a rising count is a finding
/// about the stand, never a reason to raise a retry count.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct InfrastructureRetry {
    pub operation: Operation,
    /// The attempt that failed, the first being one.
    pub attempt: NonZeroU32,
    pub cause: String,
}

/// Why a count or measurement was not recorded.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RecordError {
    /// The name is already a count, a measurement or reserved.
    Collision(String),
    /// A count no longer fits in `u64`.
    Overflow(CountName),
}

impl fmt::Display for RecordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Collision(name) => write!(
                f,
                "`{name}` is already a count or a measurement, or is reserved"
            ),
            Self::Overflow(name) => write!(f, "count `{name}` overflowed"),
        }
    }
}

impl std::error::Error for RecordError {}

/// The counts and measurements of one run. A name is either a count or a
/// measurement, never both, so no record can hide another.
#[derive(Debug, Default)]
pub struct Recorder {
    counts: BTreeMap<CountName, u64>,
    measurements: Vec<Measurement>,
    infrastructure_retries: Vec<InfrastructureRetry>,
}

impl Recorder {
    fn measured(&self, name: &str) -> bool {
        self.measurements
            .iter()
            .any(|measurement| measurement.name == name)
    }

    /// Adds to a named count; counts become `count` measurements.
    pub fn count(&mut self, name: &CountName, delta: u64) -> Result<(), RecordError> {
        if name.as_str() == INFRASTRUCTURE_RETRIES || self.measured(name.as_str()) {
            return Err(RecordError::Collision(name.to_string()));
        }
        let count = self.counts.entry(name.clone()).or_default();
        *count = count
            .checked_add(delta)
            .ok_or_else(|| RecordError::Overflow(name.clone()))?;
        Ok(())
    }

    pub fn measure(&mut self, measurement: Measurement) -> Result<(), RecordError> {
        let name = measurement.name.as_str();
        let counted = self.counts.keys().any(|count| count.as_str() == name);
        if name == INFRASTRUCTURE_RETRIES || counted || self.measured(name) {
            return Err(RecordError::Collision(measurement.name));
        }
        self.measurements.push(measurement);
        Ok(())
    }

    pub fn infrastructure_retry(
        &mut self,
        operation: Operation,
        attempt: NonZeroU32,
        cause: impl Into<String>,
    ) {
        self.infrastructure_retries.push(InfrastructureRetry {
            operation,
            attempt,
            cause: cause.into(),
        });
    }

    pub fn infrastructure_retries(&self) -> &[InfrastructureRetry] {
        &self.infrastructure_retries
    }

    /// Every measurement: the explicit ones, each count by name, and the
    /// infrastructure retries.
    pub fn measurements(&self) -> Vec<Measurement> {
        let mut all = self.measurements.clone();
        all.extend(self.counts.iter().map(|(name, &value)| {
            Measurement::observed(name.to_string(), value, MeasurementUnit::Count)
        }));
        all.push(Measurement::observed(
            INFRASTRUCTURE_RETRIES,
            self.infrastructure_retries.len() as u64,
            MeasurementUnit::Count,
        ));
        all
    }
}

#[cfg(test)]
mod tests {
    use oer_hil_model::name;

    use super::*;

    #[test]
    fn counts_and_infrastructure_retries_become_measurements() {
        let mut recorder = Recorder::default();
        let attempts = name!(CountName, "connect.attempts");
        recorder.count(&attempts, 2).unwrap();
        recorder.count(&attempts, 1).unwrap();
        recorder.infrastructure_retry(
            name!(Operation, "link.sync"),
            NonZeroU32::MIN,
            "no READY after the port opened",
        );
        let measurements = recorder.measurements();
        let value = |name: &str| {
            measurements
                .iter()
                .find(|m| m.name == name)
                .map(|m| m.value)
        };
        assert_eq!(value("connect.attempts"), Some(3));
        assert_eq!(value(INFRASTRUCTURE_RETRIES), Some(1));
    }

    #[test]
    fn a_name_is_a_count_or_a_measurement_never_both() {
        let mut recorder = Recorder::default();
        let lost = name!(CountName, "connect.lost");
        recorder.count(&lost, 1).unwrap();
        let same = Measurement::observed("connect.lost", 9, MeasurementUnit::Count);
        assert_eq!(
            recorder.measure(same),
            Err(RecordError::Collision("connect.lost".into()))
        );
        recorder
            .measure(Measurement::observed(
                "drain",
                4,
                MeasurementUnit::Microseconds,
            ))
            .unwrap();
        assert!(recorder.count(&name!(CountName, "drain"), 1).is_err());
        assert!(
            recorder
                .count(&name!(CountName, "infrastructure.retries"), 1)
                .is_err()
        );
    }

    #[test]
    fn an_overflowing_count_is_refused() {
        let mut recorder = Recorder::default();
        let name = name!(CountName, "frames");
        recorder.count(&name, u64::MAX).unwrap();
        assert_eq!(recorder.count(&name, 1), Err(RecordError::Overflow(name)));
    }
}
