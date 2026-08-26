//! Persistent PHY authority used by ESP32-S31 station scan and reconnect.

use core::marker::PhantomData;

use open_esp_radio_esp32s31_hal::{
    RadioRuntimeOwner, phy_i2c::PhyI2cMasterControl, radio_arena::Esp32s31RadioAccess,
};
use open_esp_radio_esp32s31_phy::{
    PhyAsyncDelay, PhyState, PhyTargetObserver, PhyTargetPortError, select_phy_channel_with_hal,
    switch_phy_channel_with_hal_and_mac_restart,
};

/// Persistent PHY authority used by either an initial scan or a reconnect scan.
///
/// `PhyState` is the typed PHY state created by registration;
/// despite its historical name, the same unique value carries mutable channel
/// state for the complete powered-radio lifetime.
pub struct Esp32s31ScanPhy<'state, P, O, D> {
    state: &'state mut PhyState,
    platform: &'state mut P,
    observer: O,
    _delay: PhantomData<fn() -> D>,
}

impl<'state, P, O, D> Esp32s31ScanPhy<'state, P, O, D>
where
    P: PhyI2cMasterControl,
    O: PhyTargetObserver,
    D: PhyAsyncDelay,
{
    pub const fn new(state: &'state mut PhyState, platform: &'state mut P, observer: O) -> Self {
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
        radio: &mut RadioRuntimeOwner,
    ) -> Result<(), PhyTargetPortError> {
        let mut hardware = radio.channel_hal(self.platform);
        select_phy_channel_with_hal::<D, _, _>(
            self.state,
            channel_or_frequency,
            cbw,
            &mut hardware,
            &mut self.observer,
        )
        .await
    }

    /// Stop the MAC, retune the PHY and restore the qualified REGDMA link.
    pub async fn switch_channel(
        &mut self,
        channel_or_frequency: u16,
        cbw: u8,
        radio: &mut RadioRuntimeOwner,
    ) -> Result<(), PhyTargetPortError> {
        let mut hardware = radio.channel_hal(self.platform);
        switch_phy_channel_with_hal_and_mac_restart::<D, _, _>(
            self.state,
            channel_or_frequency,
            cbw,
            &mut hardware,
            &mut self.observer,
        )
        .await
    }

    /// Stop, retune and restart through the arena's serialized channel-only
    /// capability. No PAC owner or generic register borrow crosses this API.
    pub async fn switch_published_channel(
        &mut self,
        channel_or_frequency: u16,
        cbw: u8,
        access: Esp32s31RadioAccess<'_>,
    ) -> Result<(), PhyTargetPortError> {
        let mut channel = access
            .try_channel_hal(self.platform)
            .map_err(|_| PhyTargetPortError::HardwareCapabilityUnavailable)?;
        switch_phy_channel_with_hal_and_mac_restart::<D, _, _>(
            self.state,
            channel_or_frequency,
            cbw,
            &mut channel,
            &mut self.observer,
        )
        .await
    }

    /// Return the exact persistent state, platform and observer owners after
    /// the scan transaction has stopped RX and selected its candidate.
    pub fn into_parts(self) -> (&'state mut PhyState, &'state mut P, O) {
        (self.state, self.platform, self.observer)
    }
}
