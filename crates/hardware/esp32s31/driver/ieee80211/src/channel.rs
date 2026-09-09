//! ESP32-S31 lowering of portable Wi-Fi channel definitions.

#[cfg(target_arch = "riscv32")]
use oer_esp32s31_hal::owner::RadioRuntimeOwner;
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_phy::{
    PhyAsyncDelay, PhyTargetObserver, PhyTargetPortError, RegisteredWifiPhy,
    switch_registered_wifi_channel,
};

use oer_ieee80211::channel::{WifiChannel, WifiChannelWidth};

/// Exact arguments accepted by the recovered ESP32-S31 PHY channel root.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyChannel {
    pub channel_or_frequency: u16,
    pub cbw: u8,
}

pub const fn lower_wifi_channel(channel: WifiChannel) -> PhyChannel {
    match channel.width() {
        WifiChannelWidth::Mhz20 => PhyChannel {
            channel_or_frequency: channel.primary() as u16,
            cbw: 0,
        },
        WifiChannelWidth::Mhz40Above => PhyChannel {
            channel_or_frequency: channel.center_frequency_mhz(),
            cbw: 2,
        },
        WifiChannelWidth::Mhz40Below => PhyChannel {
            channel_or_frequency: channel.center_frequency_mhz(),
            cbw: 3,
        },
    }
}

/// Retune an initialized Wi-Fi MAC while its role-specific DMA/IRQ service is
/// stopped and therefore owns no asynchronous access to these registers.
#[cfg(target_arch = "riscv32")]
pub async fn switch_esp32s31_wifi_channel<D: PhyAsyncDelay, P, O: PhyTargetObserver>(
    state: &mut RegisteredWifiPhy,
    channel: WifiChannel,
    platform: &mut P,
    radio: &mut RadioRuntimeOwner,
    observer: &mut O,
) -> Result<(), PhyTargetPortError> {
    let channel = lower_wifi_channel(channel);
    let mut hardware = radio.channel_hal(platform);
    switch_registered_wifi_channel::<D, _, _>(
        state,
        channel.channel_or_frequency,
        channel.cbw,
        &mut hardware,
        observer,
    )
    .await
}

#[cfg(test)]
mod tests;
