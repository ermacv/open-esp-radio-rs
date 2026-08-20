//! Role-neutral ESP32-S31 radio initialization.
//!
//! Cold PHY and common MAC startup happen exactly once. Station, access-point,
//! scan and monitor owners are materialized later by the physical supervisor
//! from the returned stopped frontier; startup therefore cannot accidentally
//! lock the radio into the first role an application happens to use.

use open_esp_radio_esp32s31_hal::Radio;
use open_esp_radio_esp32s31_phy::{PhyAsyncDelay, PhyCalibrationCache, PhyTargetObserver};
use open_esp_radio_esp32s31_wifi::{
    cold_start::start_esp32s31_wifi, mac_start::start_esp32s31_wifi_mac,
};

pub use open_esp_radio_esp32s31_wifi::cold_start::{
    Esp32s31WifiColdStart as Esp32s31WifiStart,
    Esp32s31WifiColdStartConfig as Esp32s31WifiStartConfig,
    Esp32s31WifiColdStartFailure as Esp32s31WifiStartFailure,
};
pub use open_esp_radio_esp32s31_wifi::mac_start::{
    Esp32s31WifiMacPlatform, Esp32s31WifiMacReady, Esp32s31WifiMacStartConfig,
    Esp32s31WifiMacStartFailure, Esp32s31WifiMacStartReport,
};
pub use open_esp_radio_esp32s31_wifi::runtime::{
    Esp32s31WifiRoleMaterialized, Esp32s31WifiRoleOwner, Esp32s31WifiRoleStopped,
    Esp32s31WifiRuntimeTransitionReport, Esp32s31WifiStopped, enter_esp32s31_wifi_runtime,
    materialize_esp32s31_wifi_role,
};

/// Inputs for the one common PHY/MAC transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31RadioStartConfig {
    wifi: Esp32s31WifiStartConfig,
    mac: Esp32s31WifiMacStartConfig,
}

impl Esp32s31RadioStartConfig {
    pub const fn new(wifi: Esp32s31WifiStartConfig, mac: Esp32s31WifiMacStartConfig) -> Self {
        Self { wifi, mac }
    }

    pub const fn wifi(self) -> Esp32s31WifiStartConfig {
        self.wifi
    }

    pub const fn mac(self) -> Esp32s31WifiMacStartConfig {
        self.mac
    }
}

/// Role-neutral stopped Wi-Fi returned after common initialization.
pub struct Esp32s31RadioReady<P> {
    wifi: Esp32s31WifiStopped<P>,
    calibration_cache: Option<PhyCalibrationCache>,
}

impl<P> Esp32s31RadioReady<P> {
    pub const fn wifi(&self) -> &Esp32s31WifiStopped<P> {
        &self.wifi
    }

    pub const fn calibration_cache(&self) -> Option<&PhyCalibrationCache> {
        self.calibration_cache.as_ref()
    }

    pub fn into_parts(self) -> (Esp32s31WifiStopped<P>, Option<PhyCalibrationCache>) {
        (self.wifi, self.calibration_cache)
    }
}

/// Failed common initialization retaining the exact hardware frontier.
pub enum Esp32s31RadioStartFailure<P> {
    Wifi(Esp32s31WifiStartFailure<P>),
    Mac(Esp32s31WifiMacStartFailure<P>),
}

/// Perform cold PHY and common MAC initialization without choosing a Wi-Fi
/// role. Role topology is validated for each supervisor epoch immediately
/// before that epoch consumes this stopped owner.
pub async fn start_esp32s31_radio<P, D, O>(
    radio: Radio<P>,
    config: Esp32s31RadioStartConfig,
    calibration_cache: Option<PhyCalibrationCache>,
    observer: O,
) -> Result<Esp32s31RadioReady<P>, Esp32s31RadioStartFailure<P>>
where
    P: open_esp_radio_esp32s31_hal::PowerClockControl
        + open_esp_radio_esp32s31_hal::phy_prelude::PhyPreludePlatformControl
        + open_esp_radio_esp32s31_hal::analog_i2c::PhyPmuControl
        + open_esp_radio_esp32s31_hal::wifi_bb::PhyWifiBbControl
        + open_esp_radio_esp32s31_hal::power_detector_platform::PhyPowerDetectorPlatformControl
        + open_esp_radio_esp32s31_hal::phy_temperature::PhyTemperatureSystemControl
        + open_esp_radio_esp32s31_hal::phy_i2c::PhyI2cMasterControl
        + Esp32s31WifiMacPlatform,
    D: PhyAsyncDelay,
    O: PhyTargetObserver + Clone,
{
    let wifi = start_esp32s31_wifi::<P, D, O>(radio, config.wifi, calibration_cache, observer)
        .await
        .map_err(Esp32s31RadioStartFailure::Wifi)?;
    let mac = start_esp32s31_wifi_mac(wifi, config.mac).map_err(Esp32s31RadioStartFailure::Mac)?;
    let runtime = enter_esp32s31_wifi_runtime(mac);
    Ok(Esp32s31RadioReady {
        wifi: runtime.wifi,
        calibration_cache: runtime.calibration_cache,
    })
}
