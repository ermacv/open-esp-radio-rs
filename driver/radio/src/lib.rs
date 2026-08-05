#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

#[cfg(feature = "wifi")]
pub mod wifi {
    pub use open_esp_radio_ieee80211 as ieee80211;
    pub use open_esp_radio_wifi_softmac as softmac;
    pub use open_esp_radio_wifi_sta as sta;
    pub use open_esp_radio_wpa2 as wpa2;

    /// Compatibility alias for the former ambiguous layer name.
    #[doc(hidden)]
    pub use open_esp_radio_wifi_softmac as lmac;
}

#[cfg(any(feature = "adapter-embassy-net", feature = "esp32s31-wifi-embassy"))]
pub mod adapters {
    #[cfg(feature = "adapter-embassy-net")]
    pub mod network {
        pub use open_esp_radio_embassy_net as embassy_net;
    }

    #[cfg(feature = "esp32s31-wifi-embassy")]
    pub mod esp32s31 {
        pub use open_esp_radio_esp32s31_wifi_embassy as wifi_embassy;
    }
}

/// Compatibility exports for the former adapter namespace.
#[cfg(any(feature = "adapter-embassy-net", feature = "esp32s31-wifi-embassy"))]
#[doc(hidden)]
pub mod integration {
    pub use crate::adapters::*;
}

#[cfg(feature = "esp32s31")]
pub mod esp32s31 {
    pub use open_esp_radio_esp32s31_hal as hal;
    pub use open_esp_radio_esp32s31_pac as pac;
    pub use open_esp_radio_esp32s31_phy as phy;
    pub use open_esp_radio_esp32s31_registers as registers;

    #[cfg(feature = "esp32s31-wifi")]
    pub mod wifi {
        pub use open_esp_radio_esp32s31_wifi_dma as dma;
        pub use open_esp_radio_esp32s31_wifi_mac as mac;
        pub use open_esp_radio_esp32s31_wifi_sta as sta;

        /// Compatibility alias for the former chip-backend name.
        #[doc(hidden)]
        pub use open_esp_radio_esp32s31_wifi_mac as lmac;

        #[cfg(feature = "esp32s31-wifi-embassy")]
        pub mod embassy {
            pub use open_esp_radio_esp32s31_wifi_embassy::station;
        }
    }
}
