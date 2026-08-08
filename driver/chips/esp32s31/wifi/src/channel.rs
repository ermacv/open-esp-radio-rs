//! ESP32-S31 lowering of portable Wi-Fi channel definitions.

#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_hal::{
    RadioRegisters, phy_i2c::PhyI2cMasterControl, phy_temperature::PhyTemperatureSystemControl,
    wifi_bb::PhyWifiBbControl,
};
#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_phy::{
    PhyAsyncDelay, PhyTargetObserver, PhyTargetPortError, phy_cold::PhyColdState,
    switch_phy_channel_with_mac_restart,
};
use open_esp_radio_ieee80211::channel::{WifiChannel, WifiChannelWidth};

/// Exact arguments accepted by the recovered ESP32-S31 PHY channel root.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Esp32s31PhyChannel {
    pub channel_or_frequency: u16,
    pub cbw: u8,
}

pub(crate) const fn lower_wifi_channel(channel: WifiChannel) -> Esp32s31PhyChannel {
    match channel.width() {
        WifiChannelWidth::Mhz20 => Esp32s31PhyChannel {
            channel_or_frequency: channel.primary() as u16,
            cbw: 0,
        },
        WifiChannelWidth::Mhz40Above => Esp32s31PhyChannel {
            channel_or_frequency: channel.center_frequency_mhz(),
            cbw: 2,
        },
        WifiChannelWidth::Mhz40Below => Esp32s31PhyChannel {
            channel_or_frequency: channel.center_frequency_mhz(),
            cbw: 3,
        },
    }
}

/// Retune an initialized Wi-Fi MAC while its role-specific DMA/IRQ service is
/// stopped and therefore owns no asynchronous access to these registers.
#[cfg(target_arch = "riscv32")]
pub async fn switch_esp32s31_wifi_channel<
    D: PhyAsyncDelay,
    P: PhyWifiBbControl + PhyTemperatureSystemControl + PhyI2cMasterControl,
    O: PhyTargetObserver,
>(
    state: &mut PhyColdState,
    channel: WifiChannel,
    platform: &mut P,
    registers: &mut RadioRegisters,
    observer: &mut O,
) -> Result<(), PhyTargetPortError> {
    let channel = lower_wifi_channel(channel);
    switch_phy_channel_with_mac_restart::<D, _, _>(
        state,
        channel.channel_or_frequency,
        channel.cbw,
        platform,
        registers,
        observer,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowering_contains_the_chip_encoding_in_the_chip_crate() {
        assert_eq!(
            lower_wifi_channel(WifiChannel::mhz20(6).unwrap()),
            Esp32s31PhyChannel {
                channel_or_frequency: 6,
                cbw: 0,
            }
        );
        assert_eq!(
            lower_wifi_channel(WifiChannel::new_2_4_ghz(6, WifiChannelWidth::Mhz40Above).unwrap()),
            Esp32s31PhyChannel {
                channel_or_frequency: 2_447,
                cbw: 2,
            }
        );
    }
}
