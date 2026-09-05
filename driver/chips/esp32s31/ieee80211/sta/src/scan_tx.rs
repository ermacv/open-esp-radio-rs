//! Concrete ESP32-S31 active-scan probe TX ownership.
//!
//! The executor-independent scan transaction decides when an active probe is
//! attempted. This module binds that edge to the polling control-TX owner and
//! retains the exact descriptor across successful and passive-fallback paths.

use crate::scan::Esp32s31ActiveProbeOutcome;
use open_esp_radio_esp32s31_wifi_mac::tx::{TxCompletion, TxHardware};
use open_esp_radio_ieee80211::management::ProbeRequest;

use crate::control_tx::{ControlTxError, Esp32s31ControlTx};
use open_esp_radio_esp32s31_wifi::ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile, WifiTxTimer};

/// Complete inputs for one active-scan Probe Request publication.
pub struct Esp32s31ScanProbeRequest<'a> {
    pub source: [u8; 6],
    pub sequence_number: u16,
    pub ssid: &'a [u8],
    pub supported_rates: &'a [u8],
    pub current_channel: Option<u8>,
    pub descriptor_capacity: Option<u32>,
}

/// Detailed terminal observation retained for applications and HIL telemetry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31ScanProbeReport {
    Transmitted(TxCompletion),
    PassiveWithoutAttempt,
    PassiveAfterCompletion(TxCompletion),
    PassiveAfterError(ControlTxError),
}

impl Esp32s31ScanProbeReport {
    pub const fn outcome(self) -> Esp32s31ActiveProbeOutcome {
        match self {
            Self::Transmitted(_) => Esp32s31ActiveProbeOutcome::Transmitted,
            Self::PassiveWithoutAttempt
            | Self::PassiveAfterCompletion(_)
            | Self::PassiveAfterError(_) => Esp32s31ActiveProbeOutcome::PassiveFallback,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Esp32s31ScanTxSummary {
    pub completions: u32,
    pub failures: u32,
}

/// Shared state machine for cold and running active-probe publication.
pub struct Esp32s31ScanTxState {
    active_probe_available: bool,
    summary: Esp32s31ScanTxSummary,
}

impl Esp32s31ScanTxState {
    pub const fn new() -> Self {
        Self {
            active_probe_available: true,
            summary: Esp32s31ScanTxSummary {
                completions: 0,
                failures: 0,
            },
        }
    }

    pub fn begin_scan(&mut self) {
        self.active_probe_available = true;
        self.summary = Esp32s31ScanTxSummary::default();
    }

    pub const fn active_probe_available(&self) -> bool {
        self.active_probe_available
    }

    pub fn classify(
        &mut self,
        result: Result<TxCompletion, ControlTxError>,
    ) -> Result<Esp32s31ScanProbeReport, ControlTxError> {
        match result {
            Ok(completion) => {
                self.summary.completions = self.summary.completions.saturating_add(1);
                if completion.status() == 0 {
                    Ok(Esp32s31ScanProbeReport::Transmitted(completion))
                } else {
                    self.summary.failures = self.summary.failures.saturating_add(1);
                    self.active_probe_available = false;
                    Ok(Esp32s31ScanProbeReport::PassiveAfterCompletion(completion))
                }
            }
            Err(error) if error.retains_quiescent_owner() => {
                self.summary.failures = self.summary.failures.saturating_add(1);
                self.active_probe_available = false;
                Ok(Esp32s31ScanProbeReport::PassiveAfterError(error))
            }
            Err(error) => {
                self.summary.failures = self.summary.failures.saturating_add(1);
                self.active_probe_available = false;
                Err(error)
            }
        }
    }

    pub const fn summary(&self) -> Esp32s31ScanTxSummary {
        self.summary
    }
}

impl Default for Esp32s31ScanTxState {
    fn default() -> Self {
        Self::new()
    }
}

/// Polling TX owner used by initial and running scans.
///
/// Probe completion is derived from the ordinary descriptor transaction, not
/// from ownership of the MAC interrupt setup token. The shared hard handler
/// may remain installed across a logical scan epoch and only acknowledges its
/// level source; no WDEV bottom half owns this control descriptor.
pub struct Esp32s31RunningScanTx<'slot, P, E, T, const BUFFER_SIZE: usize> {
    control: Esp32s31ControlTx<'slot, P, E, T, BUFFER_SIZE>,
    state: Esp32s31ScanTxState,
}

impl<'slot, P, E, T, const BUFFER_SIZE: usize> Esp32s31RunningScanTx<'slot, P, E, T, BUFFER_SIZE>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
{
    pub const fn new(control: Esp32s31ControlTx<'slot, P, E, T, BUFFER_SIZE>) -> Self {
        Self {
            control,
            state: Esp32s31ScanTxState::new(),
        }
    }

    #[cfg(test)]
    const fn new_for_test(control: Esp32s31ControlTx<'slot, P, E, T, BUFFER_SIZE>) -> Self {
        Self {
            control,
            state: Esp32s31ScanTxState::new(),
        }
    }

    pub fn begin_scan(&mut self) {
        self.state.begin_scan();
    }

    pub async fn transmit_probe_request<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        request: Esp32s31ScanProbeRequest<'_>,
    ) -> Result<Esp32s31ScanProbeReport, ControlTxError> {
        if !self.state.active_probe_available() {
            return Ok(Esp32s31ScanProbeReport::PassiveWithoutAttempt);
        }
        let Esp32s31ScanProbeRequest {
            source,
            sequence_number,
            ssid,
            supported_rates,
            current_channel,
            descriptor_capacity,
        } = request;
        let result = self
            .control
            .transmit_probe_request(
                hardware,
                ProbeRequest {
                    destination: open_esp_radio_ieee80211::management::BROADCAST_ADDRESS,
                    source,
                    bssid: open_esp_radio_ieee80211::management::BROADCAST_ADDRESS,
                    sequence_number,
                    ssid,
                    supported_rates,
                },
                current_channel,
                descriptor_capacity,
            )
            .await;
        self.state.classify(result)
    }

    pub fn into_parts(
        self,
    ) -> (
        Esp32s31ControlTx<'slot, P, E, T, BUFFER_SIZE>,
        Esp32s31ScanTxSummary,
    ) {
        (self.control, self.state.summary())
    }
}

#[cfg(test)]
mod tests;
