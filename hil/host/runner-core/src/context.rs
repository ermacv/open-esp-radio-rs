//! Immutable laboratory inputs and initialization policy for one workload.

use crate::lab::config::LabConfig;
use crate::{
    Result,
    evidence::measurements::Recorder,
    session::{SerialCapture, Settings, Target},
};
use std::path::Path;

pub struct Context<'a> {
    pub lab: &'a LabConfig,
    pub settings: Settings,
    pub measurements: Recorder,
    output: &'a Path,
}

impl<'a> Context<'a> {
    pub fn new(lab: &'a LabConfig, settings: Settings, output: &'a Path) -> Self {
        Self {
            lab,
            settings,
            output,
            measurements: Recorder::default(),
        }
    }

    /// The laboratory and initialization settings a target session uses.
    pub fn target(&self) -> Target<'a> {
        Target {
            lab: self.lab,
            settings: self.settings,
        }
    }

    /// Every workload family uses the same reset/capture/measurement lifetime.
    /// Explicit finish and error unwinding both return observations to this
    /// repetition; fixture restoration remains with the concrete fixture owner.
    pub fn capture(&self, output: &Path) -> Result<SerialCapture> {
        oer_process::check_cancelled()?;
        let relative = output
            .strip_prefix(self.output)
            .map_err(|_| "capture output is outside its repetition")?;
        let recorder = self.measurements.capture(relative)?;
        Ok(SerialCapture::start_with_reset(&self.lab.device.serial, output)?.record_into(recorder))
    }

    pub fn with_capture<T>(
        &self,
        output: &Path,
        operation: impl FnOnce(&SerialCapture) -> Result<T>,
    ) -> Result<T> {
        let capture = self.capture(output)?;
        let result = operation(&capture);
        capture.finish_with(result)
    }
}
