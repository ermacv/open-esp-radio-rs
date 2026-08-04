//! ESP32-S31 target binding for the shared station-scan transaction.
//!
//! The executor-neutral scan module owns transaction order and RX-ring
//! authority. This target-only owner keeps the persistent PHY state, platform
//! controls, delay and observer together so cold and running scan ports do not
//! reconstruct the five-argument channel-switch boundary in application or
//! HIL code.

use core::marker::PhantomData;

use crate::{
    control_tx::{ControlTxError, Esp32s31ControlTx},
    ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile, WifiTxTimer},
    sta_scan::{
        Esp32s31ScanProbeReport, Esp32s31ScanProbeRequest, Esp32s31ScanTxState,
        Esp32s31ScanTxSummary,
    },
};
use open_esp_radio_esp32s31_hal::{
    ColdRadioRegisters, RadioRegisters, phy_i2c::PhyI2cMasterControl,
    phy_temperature::PhyTemperatureSystemControl, wifi_bb::PhyWifiBbControl,
};
use open_esp_radio_esp32s31_phy::{
    PhyAsyncDelay, PhyTargetObserver, PhyTargetPortError, phy_cold::PhyColdState,
    switch_phy_channel_with_mac_restart,
};
use open_esp_radio_ieee80211::management::ProbeRequest;

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

/// Unique polling-only TX owner for a cold active scan.
///
/// Running rescan cannot use this owner because the one-way PAC transition
/// removes its task-side interrupt clear capability. The distinct running
/// owner may poll only while borrowing the `MacInterruptSetup` returned by a
/// fully quiesced connected epoch. Keeping the types separate makes that
/// executor and interrupt-ownership boundary explicit.
pub struct Esp32s31ColdScanTx<'slot, P, E, T, const BUFFER_SIZE: usize> {
    control: Esp32s31ControlTx<'slot, P, E, T, BUFFER_SIZE>,
    state: Esp32s31ScanTxState,
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
            state: Esp32s31ScanTxState::new(),
        }
    }

    /// Start one cold scan transaction with a fresh bounded telemetry epoch.
    pub fn begin_scan(&mut self) {
        self.state.begin_scan();
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
        if !self.state.active_probe_available() {
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
        let result = self
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
