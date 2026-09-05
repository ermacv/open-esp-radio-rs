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
pub(crate) struct Recorder(Arc<Mutex<BTreeMap<String, Measurement>>>);

#[derive(Clone)]
pub(crate) struct CaptureRecorder {
    recorder: Recorder,
    prefix: String,
}

impl Recorder {
    /// Publish the exact rate used by a workload's existing validator. Callers
    /// supply its resolved floor, including any legacy integer rounding.
    pub(crate) fn rate(&self, name: &str, value: u64, floor: Option<u64>) {
        use crate::evidence::run::{Comparison, MeasurementUnit};
        let measured = Measurement::observed(name, value, MeasurementUnit::BitsPerSecond);
        self.record([match floor {
            Some(floor) => measured.evaluated(Comparison::AtLeast, floor),
            None => measured,
        }]);
    }

    pub(crate) fn record(&self, measurements: impl IntoIterator<Item = Measurement>) {
        let mut recorded = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for measurement in measurements {
            recorded.insert(measurement.name.clone(), measurement);
        }
    }

    pub(crate) fn snapshot(&self) -> Vec<Measurement> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .cloned()
            .collect()
    }

    pub(crate) fn capture(&self, relative: &Path) -> Result<CaptureRecorder> {
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
    pub(crate) fn record(
        &self,
        events: &[Envelope<Event>],
        received_bytes: u64,
    ) -> Vec<Measurement> {
        let observations = protocol::observations(&self.prefix, events, received_bytes);
        self.recorder.record(observations.iter().cloned());
        observations
    }
}

#[cfg(test)]
mod tests;
