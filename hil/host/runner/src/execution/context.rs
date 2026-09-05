//! Immutable laboratory inputs and initialization policy for one workload.

use crate::{Result, evidence::measurements::Recorder, session::SerialCapture};
use crate::{lab::config::LabConfig, scenario::Scenario};
use open_esp_radio_hil_protocol::{
    WifiDataPlanePlacement, WifiRxAdmissionPolicy, WifiRxChecksumPolicy, WifiRxContinuationPolicy,
    WifiRxDispatchPolicy, WifiTxBufferPolicy, WifiTxUdpChecksumPolicy,
};
use std::path::Path;

pub(crate) struct Context<'a> {
    pub(crate) lab: &'a LabConfig,
    pub(crate) settings: Settings,
    pub(crate) measurements: Recorder,
    output: &'a Path,
}

impl<'a> Context<'a> {
    pub(crate) fn new(lab: &'a LabConfig, settings: Settings, output: &'a Path) -> Self {
        Self {
            lab,
            settings,
            output,
            measurements: Recorder::default(),
        }
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Settings {
    pub(crate) data_plane: WifiDataPlanePlacement,
    pub(crate) rx_checksum: WifiRxChecksumPolicy,
    pub(crate) tx_udp_checksum: WifiTxUdpChecksumPolicy,
    pub(crate) tx_buffer: WifiTxBufferPolicy,
    pub(crate) rx_admission: WifiRxAdmissionPolicy,
    pub(crate) rx_dispatch: WifiRxDispatchPolicy,
    pub(crate) rx_continuation: WifiRxContinuationPolicy,
    pub(crate) l1_cache_counters: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            data_plane: WifiDataPlanePlacement::SplitRadioNetwork,
            rx_checksum: WifiRxChecksumPolicy::Software,
            tx_udp_checksum: WifiTxUdpChecksumPolicy::Software,
            tx_buffer: WifiTxBufferPolicy::OwnedSramPromotion,
            rx_admission: WifiRxAdmissionPolicy::SynchronousShared,
            rx_dispatch: WifiRxDispatchPolicy::Asynchronous,
            rx_continuation: WifiRxContinuationPolicy::ImmediateSoftwareProbe,
            l1_cache_counters: false,
        }
    }
}

impl From<&Scenario> for Settings {
    fn from(scenario: &Scenario) -> Self {
        Self {
            data_plane: scenario.data_plane,
            rx_checksum: scenario.rx_checksum,
            tx_udp_checksum: scenario.tx_udp_checksum,
            tx_buffer: scenario.tx_buffer,
            rx_admission: scenario.rx_admission,
            rx_dispatch: scenario.rx_dispatch,
            rx_continuation: scenario.rx_continuation,
            l1_cache_counters: scenario.l1_cache_counters,
        }
    }
}
