#![expect(
    clippy::large_enum_variant,
    reason = "no-alloc cold-start failure retains the exact recoverable hardware frontier"
)]

//! Role-neutral ESP32-S31 radio initialization.
//!
//! Cold PHY and common MAC startup happen exactly once. Station, access-point,
//! scan and monitor owners are materialized later by the physical supervisor
//! from the returned stopped frontier; startup therefore cannot accidentally
//! lock the radio into the first role an application happens to use.

use crate::{
    cold_start::{
        WifiColdStartConfig as Esp32s31WifiStartConfig,
        WifiColdStartFailure as Esp32s31WifiStartFailure, start_esp32s31_wifi,
    },
    mac_start::{
        WifiMacPlatform, WifiMacStartConfig, WifiMacStartFailure, start_esp32s31_wifi_mac,
    },
    runtime::{WifiStopped, enter_esp32s31_wifi_runtime},
};

use oer_esp32s31_hal::owner::Radio;

use oer_esp32s31_phy::state::client::PhyPllTrackClock;
use oer_esp32s31_phy::{PhyAsyncDelay, PhyCalibrationCache, PhyTargetObserver};

/// Inputs for the one common PHY/MAC transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RadioStartConfig {
    wifi: Esp32s31WifiStartConfig,
    mac: WifiMacStartConfig,
}

impl RadioStartConfig {
    pub const fn new(wifi: Esp32s31WifiStartConfig, mac: WifiMacStartConfig) -> Self {
        Self { wifi, mac }
    }
}

/// Role-neutral stopped Wi-Fi returned after common initialization.
pub struct RadioReady<P> {
    wifi: WifiStopped<P>,
    calibration_cache: Option<PhyCalibrationCache>,
}

impl<P> RadioReady<P> {
    pub fn into_parts(self) -> (WifiStopped<P>, Option<PhyCalibrationCache>) {
        (self.wifi, self.calibration_cache)
    }
}

/// Failed common initialization retaining the exact hardware frontier.
pub enum RadioStartFailure<P> {
    Wifi(Esp32s31WifiStartFailure<P>),
    Mac(WifiMacStartFailure<P>),
}

/// Perform cold PHY and common MAC initialization without choosing a Wi-Fi
/// role. Role topology is validated for each supervisor epoch immediately
/// before that epoch consumes this stopped owner.
#[allow(
    large_assignments,
    reason = "the unique initialized radio owner graph crosses an explicit poll boundary; the linked-image stack-frame audit independently bounds this reviewed future"
)]
pub async fn start_esp32s31_radio<P, D, O>(
    radio: Radio<P>,
    config: RadioStartConfig,
    calibration_cache: Option<PhyCalibrationCache>,
    observer: O,
    clock: &mut impl PhyPllTrackClock,
) -> Result<RadioReady<P>, RadioStartFailure<P>>
where
    P: WifiMacPlatform,
    D: PhyAsyncDelay,
    O: PhyTargetObserver + Clone,
{
    let wifi =
        start_esp32s31_wifi::<P, D, O>(radio, config.wifi, calibration_cache, observer, clock)
            .await
            .map_err(RadioStartFailure::Wifi)?;
    let mac = start_esp32s31_wifi_mac(wifi, config.mac).map_err(RadioStartFailure::Mac)?;
    let runtime = enter_esp32s31_wifi_runtime(mac);
    Ok(RadioReady {
        wifi: runtime.wifi,
        calibration_cache: runtime.calibration_cache,
    })
}
