#![no_std]

#[cfg(test)]
extern crate std;

pub use open_esp_radio_embassy_net as embassy_net;
pub use open_esp_radio_ieee80211 as ieee80211;
pub use open_esp_radio_wpa2 as wpa2;

#[cfg(feature = "esp32s31")]
pub mod esp32s31_embassy_tx;

#[cfg(feature = "esp32s31")]
pub mod esp32s31_cooperative_tx;

#[cfg(feature = "esp32s31")]
pub mod esp32s31_embassy_irq;

#[cfg(feature = "esp32s31")]
pub mod esp32s31 {
    pub use crate::esp32s31_cooperative_tx as cooperative_tx;
    pub use crate::esp32s31_embassy_irq as embassy_irq;
    pub use crate::esp32s31_embassy_tx as embassy_tx;
    pub use open_esp_radio_hal_esp32s31 as hal;
    pub use open_esp_radio_mac_esp32s31 as mac;
    pub use open_esp_radio_pac_esp32s31 as pac;
    pub use open_esp_radio_phy_esp32s31 as phy;
}
