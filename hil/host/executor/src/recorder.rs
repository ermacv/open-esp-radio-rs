//! What one scenario run counts besides its trace.

use std::collections::BTreeMap;

use oer_hil_run_bundle_format::run::{Measurement, MeasurementUnit};
use serde::Serialize;

/// One retry of infrastructure: an operation of the stand or the link,
/// not of the protocol under test, repeated because its first attempt
/// failed. Every one is recorded and counted; a rising count is a finding
/// about the stand, never a reason to raise a retry count.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct InfrastructureRetry {
    /// What was retried, such as `link.sync` or `board.reset-cause`.
    pub operation: &'static str,
    /// The attempt that failed, the first being one.
    pub attempt: u32,
    pub cause: String,
}

/// The counters and measurements of one run.
#[derive(Debug, Default)]
pub struct Recorder {
    counts: BTreeMap<String, u64>,
    measurements: Vec<Measurement>,
    infrastructure_retries: Vec<InfrastructureRetry>,
}

impl Recorder {
    /// Adds to a named count; counts become `count` measurements.
    pub fn count(&mut self, name: &str, delta: u64) {
        *self.counts.entry(name.to_owned()).or_default() += delta;
    }

    pub fn measure(&mut self, measurement: Measurement) {
        self.measurements.push(measurement);
    }

    pub fn infrastructure_retry(
        &mut self,
        operation: &'static str,
        attempt: u32,
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

    /// Every measurement: the explicit ones, then each count, by name.
    pub fn measurements(&self) -> Vec<Measurement> {
        let mut all = self.measurements.clone();
        all.extend(self.counts.iter().map(|(name, &value)| {
            Measurement::observed(name.clone(), value, MeasurementUnit::Count)
        }));
        all.push(Measurement::observed(
            "infrastructure.retries",
            self.infrastructure_retries.len() as u64,
            MeasurementUnit::Count,
        ));
        all
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_and_infrastructure_retries_become_measurements() {
        let mut recorder = Recorder::default();
        recorder.count("connect.attempts", 2);
        recorder.count("connect.attempts", 1);
        recorder.infrastructure_retry("link.sync", 1, "no READY after the port opened");
        let measurements = recorder.measurements();
        let value = |name: &str| {
            measurements
                .iter()
                .find(|m| m.name == name)
                .map(|m| m.value)
        };
        assert_eq!(value("connect.attempts"), Some(3));
        assert_eq!(value("infrastructure.retries"), Some(1));
    }
}
