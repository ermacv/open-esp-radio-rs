//! Common ownership boundary between stopped Wi-Fi and one AP role.

use open_esp_radio_esp32s31_phy::PhyState;
use open_esp_radio_esp32s31_registers::{MacInterruptSetup, RadioRegisters};
use open_esp_radio_esp32s31_wifi::runtime::{
    Esp32s31WifiRuntimeContext, Esp32s31WifiRuntimeParts, Esp32s31WifiStopped,
};
use open_esp_radio_ieee80211::channel::WifiChannel;

/// Common radio state retained while the AP role owns DMA and IRQ resources.
pub struct Esp32s31AccessPointRoleOwner<P> {
    platform: P,
    context: Esp32s31WifiRuntimeContext,
}

impl<P> Esp32s31AccessPointRoleOwner<P> {
    pub fn radio_mut(&mut self) -> (&mut PhyState, &mut P) {
        (self.context.phy_mut(), &mut self.platform)
    }

    pub fn set_current_channel(&mut self, channel: WifiChannel) {
        self.context.set_current_channel(channel);
    }

    /// Reassemble role-neutral Wi-Fi only after runtime DMA and IRQ owners
    /// have independently returned their exact capabilities.
    pub fn into_stopped<L>(
        self,
        registers: RadioRegisters,
        interrupt_setup: MacInterruptSetup,
        resources: L,
    ) -> Esp32s31AccessPointStopped<P, L> {
        Esp32s31AccessPointStopped {
            wifi: self
                .context
                .into_stopped(self.platform, registers, interrupt_setup),
            resources,
        }
    }
}

pub struct Esp32s31AccessPointMaterialized<P, L> {
    pub owner: Esp32s31AccessPointRoleOwner<P>,
    pub registers: RadioRegisters,
    pub interrupt_setup: MacInterruptSetup,
    pub resources: L,
}

pub struct Esp32s31AccessPointStopped<P, L> {
    pub wifi: Esp32s31WifiStopped<P>,
    pub resources: L,
}

/// Consume role-neutral Wi-Fi before any AP DMA, interrupt or beacon epoch.
pub fn materialize_esp32s31_access_point<P, L>(
    wifi: Esp32s31WifiStopped<P>,
    resources: L,
) -> Esp32s31AccessPointMaterialized<P, L> {
    let Esp32s31WifiRuntimeParts {
        platform,
        registers,
        interrupt_setup,
        context,
    } = wifi.into_runtime_parts();
    Esp32s31AccessPointMaterialized {
        owner: Esp32s31AccessPointRoleOwner { platform, context },
        registers,
        interrupt_setup,
        resources,
    }
}
