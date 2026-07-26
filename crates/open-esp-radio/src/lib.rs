#![no_std]

#[cfg(feature = "esp32s31")]
pub mod esp32s31 {
    pub use open_esp_radio_hal_esp32s31 as hal;
    pub use open_esp_radio_pac_esp32s31 as pac;
    pub use open_esp_radio_phy_esp32s31 as phy;
}
