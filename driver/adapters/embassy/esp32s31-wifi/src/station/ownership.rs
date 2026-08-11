//! Common ownership boundary between stopped Wi-Fi and one station role.

use open_esp_radio_esp32s31_hal::RadioRegisters;
use open_esp_radio_esp32s31_pac::MacInterruptSetup;
use open_esp_radio_esp32s31_phy::PhyState;
use open_esp_radio_esp32s31_wifi::runtime::{
    Esp32s31WifiRuntimeContext, Esp32s31WifiRuntimeParts, Esp32s31WifiStopped,
};
use open_esp_radio_ieee80211::channel::WifiChannel;

use super::Esp32s31StationRadioOwner;

/// Common runtime state retained beside a materialized station composition.
///
/// Role-local DMA, network and executor resources are deliberately not stored
/// here: the station task must consume them while it runs and return them at
/// its terminal edge. Register and interrupt-setup ownership are likewise
/// transferred into the finite station phases. This owner can reconstruct a
/// stopped state only when all three independently returned inputs are
/// supplied again.
pub struct Esp32s31StationRoleOwner<P> {
    platform: P,
    context: Esp32s31WifiRuntimeContext,
}

impl<P> Esp32s31StationRoleOwner<P> {
    /// Borrow the persistent PHY and platform authority for one finite role
    /// transaction without exposing the stopped-owner constructor.
    pub fn radio_mut(&mut self) -> (&mut PhyState, &mut P) {
        (self.context.phy_mut(), &mut self.platform)
    }

    pub fn set_current_channel(&mut self, channel: WifiChannel) {
        self.context.set_current_channel(channel);
    }

    /// Reassemble the role-neutral owner only from independently returned
    /// PAC and interrupt-route capabilities.
    pub fn into_stopped<L>(
        self,
        registers: RadioRegisters,
        interrupt_setup: MacInterruptSetup,
        resources: L,
    ) -> Esp32s31StationStopped<P, L> {
        Esp32s31StationStopped {
            wifi: self
                .context
                .into_stopped(self.platform, registers, interrupt_setup),
            resources,
        }
    }
}

impl<P> Esp32s31StationRadioOwner for Esp32s31StationRoleOwner<P> {
    type Platform = P;

    fn radio_mut(&mut self) -> (&mut PhyState, &mut Self::Platform) {
        Esp32s31StationRoleOwner::radio_mut(self)
    }
}

/// Capabilities transferred from stopped Wi-Fi into the first station phase.
pub struct Esp32s31StationMaterialized<P, L> {
    pub owner: Esp32s31StationRoleOwner<P>,
    pub registers: RadioRegisters,
    pub interrupt_setup: MacInterruptSetup,
    /// Exact role-local owner graph which must move into the station task.
    pub resources: L,
}

/// Cleanly dematerialized station role.
///
/// The common Wi-Fi owner may be consumed by another supported role. The
/// role-local resources remain separate and can be rebound to a later station
/// control epoch.
pub struct Esp32s31StationStopped<P, L> {
    pub wifi: Esp32s31WifiStopped<P>,
    pub resources: L,
}

/// Consume the role-neutral Wi-Fi owner before any station scan, DMA or IRQ
/// epoch begins.
pub fn materialize_esp32s31_station<P, L>(
    wifi: Esp32s31WifiStopped<P>,
    resources: L,
) -> Esp32s31StationMaterialized<P, L> {
    let Esp32s31WifiRuntimeParts {
        platform,
        registers,
        interrupt_setup,
        context,
    } = wifi.into_runtime_parts();
    Esp32s31StationMaterialized {
        owner: Esp32s31StationRoleOwner { platform, context },
        registers,
        interrupt_setup,
        resources,
    }
}
