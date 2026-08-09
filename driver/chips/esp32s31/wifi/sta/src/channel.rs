//! Persistent PHY authority used by ESP32-S31 station scan and reconnect.

use core::marker::PhantomData;

use open_esp_radio_esp32s31_hal::{
    RadioRegisters, phy_i2c::PhyI2cMasterControl, phy_temperature::PhyTemperatureSystemControl,
    wifi_bb::PhyWifiBbControl,
};
use open_esp_radio_esp32s31_phy::{
    PhyAsyncDelay, PhyTargetObserver, PhyTargetPortError, phy_cold::PhyColdState,
    switch_phy_channel_with_mac_restart,
};

/// Persistent PHY authority used by either an initial scan or a reconnect scan.
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
