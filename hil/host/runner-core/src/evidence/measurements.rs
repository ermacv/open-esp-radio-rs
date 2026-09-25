//! Shared numeric observations. Workloads retain ownership of acceptance policy.
//!
//! A recorder belongs to one repetition. Captures publish decoded observations
//! even when teardown or a gate fails; replayed evidence is counted only once.

use crate::{Result, evidence::run::Measurement};
use open_esp_radio_hil_protocol::{Envelope, Event};
use std::{
    collections::BTreeMap,
    path::{Component, Path},
    sync::{Arc, Mutex},
};

mod protocol;

#[derive(Clone, Default)]
pub struct Recorder(Arc<Mutex<BTreeMap<String, Measurement>>>);

#[derive(Clone)]
pub struct CaptureRecorder {
    recorder: Recorder,
    prefix: String,
}

impl Recorder {
    /// Record the outcome of an actually attempted semantic check. A missing
    /// check stays absent; scenario success must never synthesize it later.
    pub fn check(&self, name: &str, passed: bool) {
        use crate::evidence::run::{Comparison, MeasurementUnit};
        let mut recorded = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // A later successful attempt cannot erase an earlier failure in this
        // repetition. Numeric telemetry still uses the ordinary record path.
        let passed = passed
            && recorded
                .get(name)
                .is_none_or(|previous| previous.value == 1);
        recorded.insert(
            name.to_owned(),
            Measurement::observed(name, u64::from(passed), MeasurementUnit::Count)
                .evaluated(Comparison::Exactly, 1),
        );
    }

    /// Publish the exact rate used by a workload's existing validator. Callers
    /// supply its resolved floor, including any legacy integer rounding.
    pub fn rate(&self, name: &str, value: u64, floor: Option<u64>) {
        use crate::evidence::run::{Comparison, MeasurementUnit};
        let measured = Measurement::observed(name, value, MeasurementUnit::BitsPerSecond);
        self.record([match floor {
            Some(floor) => measured.evaluated(Comparison::AtLeast, floor),
            None => measured,
        }]);
    }

    pub fn record(&self, measurements: impl IntoIterator<Item = Measurement>) {
        let mut recorded = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for measurement in measurements {
            recorded.insert(measurement.name.clone(), measurement);
        }
    }

    pub fn snapshot(&self) -> Vec<Measurement> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .cloned()
            .collect()
    }

    pub fn capture(&self, relative: &Path) -> Result<CaptureRecorder> {
        let mut prefix = String::from("target");
        for component in relative.components() {
            let Component::Normal(name) = component else {
                return Err("measurement capture scope must stay within its repetition".into());
            };
            let name = name.to_str().ok_or("capture scope is not UTF-8")?;
            if name.is_empty()
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            {
                return Err(
                    "capture scope must contain lowercase letters, digits or hyphens".into(),
                );
            }
            prefix.push('.');
            prefix.push_str(name);
        }
        if prefix.len() > 48 {
            return Err("measurement capture scope is too long".into());
        }
        Ok(CaptureRecorder {
            recorder: self.clone(),
            prefix,
        })
    }
}

impl CaptureRecorder {
    pub fn record(&self, events: &[Envelope<Event>], received_bytes: u64) -> Vec<Measurement> {
        let observations = protocol::observations(&self.prefix, events, received_bytes);
        self.recorder.record(observations.iter().cloned());
        observations
    }
}

#[cfg(test)]
mod tests;
