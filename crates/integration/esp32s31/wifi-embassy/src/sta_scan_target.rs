//! ESP32-S31 target binding for the shared station-scan transaction.
//!
//! The executor-neutral scan module owns transaction order and RX-ring
//! authority. This target-only owner keeps the persistent PHY state, platform
//! controls, delay and observer together so cold and running scan ports do not
//! reconstruct the five-argument channel-switch boundary in application or
//! HIL code.

use core::marker::PhantomData;

use open_esp_radio_esp32s31_hal::{
    ColdRadioRegisters, RadioRegisters, phy_i2c::PhyI2cMasterControl,
    phy_temperature::PhyTemperatureSystemControl, wifi_bb::PhyWifiBbControl,
};
use open_esp_radio_esp32s31_phy::{
    PhyAsyncDelay, PhyTargetObserver, PhyTargetPortError, phy_cold::PhyColdState,
    switch_phy_channel_with_mac_restart,
};
use open_esp_radio_esp32s31_wifi_mac::tx::TxCompletion;
use open_esp_radio_ieee80211::management::ProbeRequest;

use crate::{
    control_tx::{ControlTxError, Esp32s31ControlTx},
    ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile, WifiTxTimer},
    sta_scan::Esp32s31ActiveProbeOutcome,
};

/// Persistent PHY authority used by either a cold scan or a running rescan.
///
/// `PhyColdState` is the recovered PHY state image created by registration;
/// despite its historical name, the same unique value carries mutable channel
/// state for the complete powered-radio lifetime.
pub struct Esp32s31ScanPhy<'state, P, O, D> {
    state: &'state mut PhyColdState,
    platform: &'state mut P,
    observer: O,
    _delay: PhantomData<fn() -> D>,
}

impl<'state, P, O, D> Esp32s31ScanPhy<'state, P, O, D>
where
    P: PhyWifiBbControl + PhyTemperatureSystemControl + PhyI2cMasterControl,
    O: PhyTargetObserver,
    D: PhyAsyncDelay,
{
    pub const fn new(
        state: &'state mut PhyColdState,
        platform: &'state mut P,
        observer: O,
    ) -> Self {
        Self {
            state,
            platform,
            observer,
            _delay: PhantomData,
        }
    }

    /// Stop the MAC, retune the PHY and restore the qualified REGDMA link.
    pub async fn switch_channel(
        &mut self,
        channel_or_frequency: u16,
        cbw: u8,
        registers: &mut RadioRegisters,
    ) -> Result<(), PhyTargetPortError> {
        switch_phy_channel_with_mac_restart::<D, _, _>(
            self.state,
            channel_or_frequency,
            cbw,
            self.platform,
            registers,
            &mut self.observer,
        )
        .await
    }

    /// Return the exact persistent state, platform and observer owners after
    /// the scan transaction has stopped RX and selected its candidate.
    pub fn into_parts(self) -> (&'state mut PhyColdState, &'state mut P, O) {
        (self.state, self.platform, self.observer)
    }
}

/// Complete inputs for one active-scan Probe Request publication.
pub struct Esp32s31ScanProbeRequest<'a> {
    pub source: [u8; 6],
    pub sequence_number: u16,
    pub ssid: &'a [u8],
    pub supported_rates: &'a [u8],
    pub current_channel: Option<u8>,
    pub descriptor_capacity: Option<u32>,
}

/// Detailed terminal observation retained for HIL evidence and telemetry.
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

/// Unique polling-only TX owner for a cold active scan.
///
/// Running rescan cannot use this owner: once the MAC IRQ runtime exists, TX
/// completion must be driven by the cooperative runner rather than polling.
/// Keeping this type cold-specific prevents those two executor contracts from
/// being silently conflated.
pub struct Esp32s31ColdScanTx<'slot, P, E, T, const BUFFER_SIZE: usize> {
    control: Esp32s31ControlTx<'slot, P, E, T, BUFFER_SIZE>,
    active_probe_available: bool,
    summary: Esp32s31ScanTxSummary,
}

impl<'slot, P, E, T, const BUFFER_SIZE: usize> Esp32s31ColdScanTx<'slot, P, E, T, BUFFER_SIZE>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
{
    pub const fn new(control: Esp32s31ControlTx<'slot, P, E, T, BUFFER_SIZE>) -> Self {
        Self {
            control,
            active_probe_available: true,
            summary: Esp32s31ScanTxSummary {
                completions: 0,
                failures: 0,
            },
        }
    }

    /// Start one cold scan transaction with a fresh bounded telemetry epoch.
    pub fn begin_scan(&mut self) {
        self.active_probe_available = true;
        self.summary = Esp32s31ScanTxSummary::default();
    }

    /// Publish one Probe Request or return a safe passive-scan disposition.
    ///
    /// A non-quiescent error is returned to the caller. It may not be hidden
    /// as passive fallback because the contained control owner can still be
    /// busy or permanently quarantined until radio reset.
    pub async fn transmit_probe_request(
        &mut self,
        registers: &mut ColdRadioRegisters,
        request: Esp32s31ScanProbeRequest<'_>,
    ) -> Result<Esp32s31ScanProbeReport, ControlTxError> {
        if !self.active_probe_available {
            return Ok(Esp32s31ScanProbeReport::PassiveWithoutAttempt);
        }
        // SOURCE: complete `_oracles/libpp.a[hal_tsf.o]` station-start
        // transaction. Keep this edge paired with every cold Probe Request as
        // in the previously board-qualified HIL path.
        registers.start_station_tsf(0);
        registers.clear_mac_interrupts(u32::MAX);
        let Esp32s31ScanProbeRequest {
            source,
            sequence_number,
            ssid,
            supported_rates,
            current_channel,
            descriptor_capacity,
        } = request;
        match self
            .control
            .transmit_probe_request(
                registers,
                ProbeRequest {
                    source,
                    sequence_number,
                    ssid,
                    supported_rates,
                },
                current_channel,
                descriptor_capacity,
            )
            .await
        {
            Ok(completion) => {
                self.summary.completions = self.summary.completions.saturating_add(1);
                if completion.status == 0 {
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

    pub fn into_parts(
        self,
    ) -> (
        Esp32s31ControlTx<'slot, P, E, T, BUFFER_SIZE>,
        Esp32s31ScanTxSummary,
    ) {
        (self.control, self.summary)
    }
}
