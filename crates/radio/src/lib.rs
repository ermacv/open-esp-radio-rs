#![no_std]

#[cfg(test)]
extern crate std;

#[cfg(feature = "wifi")]
pub mod wifi {
    pub use open_esp_radio_embassy_net as embassy_net;
    pub use open_esp_radio_ieee80211 as ieee80211;
    pub use open_esp_radio_wpa2 as wpa2;
}

#[cfg(feature = "esp32s31-wifi")]
pub mod esp32s31_wifi_embassy_tx;

#[cfg(feature = "esp32s31-wifi")]
pub mod esp32s31_wifi_cooperative_tx;

#[cfg(feature = "esp32s31-wifi")]
pub mod esp32s31_wifi_embassy_irq;

#[cfg(feature = "esp32s31")]
pub mod esp32s31 {
    pub use open_esp_radio_esp32s31_hal as hal;
    pub use open_esp_radio_esp32s31_pac as pac;
    pub use open_esp_radio_esp32s31_phy as phy;

    #[cfg(feature = "esp32s31-wifi")]
    pub mod wifi {
        pub use crate::esp32s31_wifi_cooperative_tx as cooperative_tx;
        pub use crate::esp32s31_wifi_embassy_irq as embassy_irq;
        pub use crate::esp32s31_wifi_embassy_tx as embassy_tx;
        pub use open_esp_radio_esp32s31_wifi_mac as mac;
    }
}
