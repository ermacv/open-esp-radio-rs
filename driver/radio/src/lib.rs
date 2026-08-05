#![no_std]

#[cfg(test)]
extern crate std;

#[cfg(feature = "wifi")]
pub mod wifi {
    pub use open_esp_radio_ieee80211 as ieee80211;
    pub use open_esp_radio_wifi_lmac as lmac;
    pub use open_esp_radio_wifi_sta as sta;
    pub use open_esp_radio_wpa2 as wpa2;
}

#[cfg(any(feature = "integration-embassy-net", feature = "esp32s31-wifi-embassy"))]
pub mod integration {
    #[cfg(feature = "integration-embassy-net")]
    pub mod network {
        pub use open_esp_radio_embassy_net as embassy_net;
    }

    #[cfg(feature = "esp32s31-wifi-embassy")]
    pub mod esp32s31 {
        pub use open_esp_radio_esp32s31_wifi_embassy as wifi_embassy;
    }
}

#[cfg(feature = "esp32s31")]
pub mod esp32s31 {
    pub use open_esp_radio_esp32s31_hal as hal;
    pub use open_esp_radio_esp32s31_pac as pac;
    pub use open_esp_radio_esp32s31_phy as phy;
    pub use open_esp_radio_esp32s31_registers as registers;

    #[cfg(feature = "esp32s31-wifi")]
    pub mod wifi {
        pub use open_esp_radio_esp32s31_wifi_lmac as lmac;
        pub use open_esp_radio_esp32s31_wifi_sta as sta;

        #[cfg(feature = "esp32s31-wifi-embassy")]
        pub mod embassy {
            pub use open_esp_radio_esp32s31_wifi_embassy::station;
        }
    }
}
