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
    cooperative_tx::CooperativeTxHardware,
    ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile, WifiTxTimer},
    running_scan::{
        Esp32s31RunningScanPhy, Esp32s31RunningScanReceive, Esp32s31RunningScanTransmit,
    },
    sta_scan::{
        Esp32s31ScanFrameObserver, Esp32s31ScanObservationContext, Esp32s31ScanProbeReport,
        Esp32s31ScanProbeRequest, Esp32s31ScanRx, Esp32s31ScanRxError, Esp32s31ScanRxPhase,
        Esp32s31ScanRxProgress, Esp32s31ScanTxState, Esp32s31ScanTxSummary,
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

    /// Retune the PHY while the caller still owns a cold, stopped MAC.
    pub async fn select_channel(
        &mut self,
        channel_or_frequency: u16,
        cbw: u8,
        registers: &mut RadioRegisters,
    ) -> Result<(), PhyTargetPortError> {
        open_esp_radio_esp32s31_phy::select_phy_channel::<D, _, _>(
            self.state,
            channel_or_frequency,
            cbw,
            self.platform,
            registers,
            &mut self.observer,
        )
        .await
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

impl<'state, 'cell, 'registers, P, O, D>
    Esp32s31RunningScanPhy<CooperativeTxHardware<'cell, 'registers>>
    for Esp32s31ScanPhy<'state, P, O, D>
where
    P: PhyWifiBbControl + PhyTemperatureSystemControl + PhyI2cMasterControl,
    O: PhyTargetObserver,
    D: PhyAsyncDelay,
{
    type Error = PhyTargetPortError;

    fn switch_channel<'a>(
        &'a mut self,
        hardware: &'a mut CooperativeTxHardware<'cell, 'registers>,
        channel: u8,
    ) -> impl core::future::Future<Output = Result<(), Self::Error>> + 'a {
        async move {
            let mut registers = hardware.register_cell().borrow_mut();
            self.switch_channel(u16::from(channel), 0, &mut registers)
                .await
        }
    }
}

impl<P, O, D> Esp32s31RunningScanPhy<ColdRadioRegisters> for Esp32s31ScanPhy<'_, P, O, D>
where
    P: PhyWifiBbControl + PhyTemperatureSystemControl + PhyI2cMasterControl,
    O: PhyTargetObserver,
    D: PhyAsyncDelay,
{
    type Error = PhyTargetPortError;

    fn switch_channel<'a>(
        &'a mut self,
        hardware: &'a mut ColdRadioRegisters,
        channel: u8,
    ) -> impl core::future::Future<Output = Result<(), Self::Error>> + 'a {
        async move { self.switch_channel(u16::from(channel), 0, hardware).await }
    }
}

impl<const COUNT: usize, const DMA_BUFFER_SIZE: usize, const DMA_STORAGE_SIZE: usize>
    Esp32s31RunningScanReceive<ColdRadioRegisters>
    for Esp32s31ScanRx<'_, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>
{
    type Error = Esp32s31ScanRxError;

    fn prepare_initial(&mut self, _hardware: &mut ColdRadioRegisters) -> Result<(), Self::Error> {
        let actual = self.phase();
        if actual == Esp32s31ScanRxPhase::Prepared {
            Ok(())
        } else {
            Err(Esp32s31ScanRxError::InvalidPhase {
                expected: Esp32s31ScanRxPhase::Prepared,
                actual,
            })
        }
    }

    fn start<'a>(
        &'a mut self,
        hardware: &'a mut ColdRadioRegisters,
    ) -> impl core::future::Future<Output = Result<(), Self::Error>> + 'a {
        async move { Esp32s31ScanRx::start(self, hardware) }
    }

    fn observe_management<O, const RECORDS: usize>(
        &mut self,
        hardware: &mut ColdRadioRegisters,
        context: &mut Esp32s31ScanObservationContext<'_, O, RECORDS>,
    ) -> Result<Esp32s31ScanRxProgress, Self::Error>
    where
        O: Esp32s31ScanFrameObserver,
    {
        Esp32s31ScanRx::observe_management(self, hardware, context)
    }

    fn stop(&mut self, hardware: &mut ColdRadioRegisters) -> Result<(), Self::Error> {
        Esp32s31ScanRx::stop(self, hardware)
    }

    fn prepare_next(&mut self, hardware: &mut ColdRadioRegisters) -> Result<(), Self::Error> {
        Esp32s31ScanRx::prepare_next(self, hardware)
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

impl<'slot, P, E, T, const BUFFER_SIZE: usize> Esp32s31RunningScanTransmit<ColdRadioRegisters>
    for Esp32s31ColdScanTx<'slot, P, E, T, BUFFER_SIZE>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
{
    type Error = ControlTxError;

    fn begin_scan(&mut self) {
        Esp32s31ColdScanTx::begin_scan(self);
    }

    fn transmit_probe_request<'a>(
        &'a mut self,
        hardware: &'a mut ColdRadioRegisters,
        request: Esp32s31ScanProbeRequest<'a>,
    ) -> impl core::future::Future<Output = Result<Esp32s31ScanProbeReport, Self::Error>> + 'a {
        Esp32s31ColdScanTx::transmit_probe_request(self, hardware, request)
    }
}
