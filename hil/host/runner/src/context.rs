//! Immutable laboratory inputs and initialization policy for one workload.

use crate::lab::config::LabConfig;
use crate::{
    Result,
    evidence::measurements::Recorder,
    session::{SerialCapture, Settings, Target},
};
use std::path::Path;

pub(crate) struct Context<'a> {
    pub(crate) lab: &'a LabConfig,
    pub(crate) settings: Settings,
    pub(crate) measurements: Recorder,
    output: &'a Path,
    fixture: Option<&'a crate::fixture::prepared::Prepared>,
}

impl<'a> Context<'a> {
    pub(crate) fn new(lab: &'a LabConfig, settings: Settings, output: &'a Path) -> Self {
        Self {
            lab,
            settings,
            output,
            fixture: None,
            measurements: Recorder::default(),
        }
    }

    /// The laboratory and initialization settings a target session uses.
    pub(crate) fn target(&self) -> Target<'a> {
        Target {
            lab: self.lab,
            settings: self.settings,
        }
    }

    pub(crate) fn with_fixture(mut self, fixture: &'a crate::fixture::prepared::Prepared) -> Self {
        self.fixture = Some(fixture);
        self
    }

    pub(crate) fn ap(
        &self,
    ) -> Result<std::cell::RefMut<'_, crate::fixture::controlled_ap::ControlledAp>> {
        self.fixture.ok_or("workload has no prepared fixture")?.ap()
    }

    /// Every workload family uses the same reset/capture/measurement lifetime.
    /// Explicit finish and error unwinding both return observations to this
    /// repetition; fixture restoration remains with the concrete fixture owner.
    pub(crate) fn capture(&self, output: &Path) -> Result<SerialCapture> {
        oer_process::check_cancelled()?;
        let relative = output
            .strip_prefix(self.output)
            .map_err(|_| "capture output is outside its repetition")?;
        let recorder = self.measurements.capture(relative)?;
        Ok(SerialCapture::start_with_reset(&self.lab.device.serial, output)?.record_into(recorder))
    }

    pub(crate) fn with_capture<T>(
        &self,
        output: &Path,
        operation: impl FnOnce(&SerialCapture) -> Result<T>,
    ) -> Result<T> {
        let capture = self.capture(output)?;
        let result = operation(&capture);
        capture.finish_with(result)
    }
}
