#![no_std]
#![deny(unsafe_code)]

//! Product-level ESP32-S31 Wi-Fi service composition.
//!
//! Unlike the lower reusable Embassy and esp-hal adapters, this crate owns the
//! complete station supervisor, network datapath and connected-epoch wiring.
//! Board firmware supplies physical peripherals, credentials and application
//! policy; it does not rebuild scan, join, DMA/IRQ or reconnect transactions.

mod connected;
mod station;

pub use connected::Esp32s31StationNetworkEvents;
pub use station::run;

/// Board identity and application station policy consumed by the product
/// supervisor. Reading eFuse and deriving credentials remain board/application
/// responsibilities rather than hidden radio-driver side effects.
pub struct Esp32s31StationServiceConfig {
    pub(crate) station_mac: open_esp_radio::WifiMacAddress,
    pub(crate) access_point_mac: open_esp_radio::WifiMacAddress,
    pub(crate) calibration: open_esp_radio::esp32s31::phy::PhyCalibrationIdentity,
    pub(crate) initial_channel: open_esp_radio::wifi::ieee80211::channel::WifiChannel,
    pub(crate) request: open_esp_radio::StationRequest,
}

impl Esp32s31StationServiceConfig {
    pub const fn new(
        station_mac: open_esp_radio::WifiMacAddress,
        access_point_mac: open_esp_radio::WifiMacAddress,
        calibration: open_esp_radio::esp32s31::phy::PhyCalibrationIdentity,
        initial_channel: open_esp_radio::wifi::ieee80211::channel::WifiChannel,
        request: open_esp_radio::StationRequest,
    ) -> Self {
        Self {
            station_mac,
            access_point_mac,
            calibration,
            initial_channel,
            request,
        }
    }
}
