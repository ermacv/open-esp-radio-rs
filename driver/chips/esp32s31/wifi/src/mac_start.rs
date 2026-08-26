//! Executor-neutral transition from calibrated PHY ownership to cold MAC ownership.

use open_esp_radio_esp32s31_hal::{Radio, state::Powered};
use open_esp_radio_esp32s31_phy::{PhyCalibrationCache, PhyState, PhyTxTargetPowerProfile};
use open_esp_radio_esp32s31_wifi_mac::init::{
    MacCoexPtiSource, MacColdStartError, MacColdStartOutcome, MacDelayEntropy,
    MacSlowClockCalibrationSource, MacTxPowerSource, initialize_wifi_mac,
};
use open_esp_radio_wifi_softmac::WifiMacAddress;

use crate::cold_start::{Esp32s31WifiColdStart, Esp32s31WifiColdStartReport};

/// Platform operations needed to join the calibrated PHY power table to the
/// finite MAC initializer.
///
/// The installation method is deliberately semantic: MAC code never borrows
/// the PHY parameter arena and never depends on an ESP-HAL singleton type.
pub trait Esp32s31WifiMacPlatform:
    MacCoexPtiSource + MacDelayEntropy + MacSlowClockCalibrationSource + MacTxPowerSource
{
    fn install_phy_tx_power_profile(&mut self, profile: PhyTxTargetPowerProfile);
}

/// Role-neutral inputs for the common Wi-Fi MAC transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31WifiMacStartConfig {
    handshake_sample_limit: u32,
    station_address: WifiMacAddress,
    access_point_address: WifiMacAddress,
}

impl Esp32s31WifiMacStartConfig {
    pub const fn new(
        handshake_sample_limit: u32,
        station_address: WifiMacAddress,
        access_point_address: WifiMacAddress,
    ) -> Self {
        Self {
            handshake_sample_limit,
            station_address,
            access_point_address,
        }
    }

    pub const fn handshake_sample_limit(self) -> u32 {
        self.handshake_sample_limit
    }

    pub const fn station_address(self) -> WifiMacAddress {
        self.station_address
    }

    pub const fn access_point_address(self) -> WifiMacAddress {
        self.access_point_address
    }
}

/// Reports from the PHY and common MAC transitions kept with the owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31WifiMacStartReport {
    pub wifi: Esp32s31WifiColdStartReport,
    pub mac: MacColdStartOutcome,
}

/// Powered radio after common MAC initialization but before role-specific RX,
/// DMA and interrupt policy is activated.
pub struct Esp32s31WifiMacReady<P> {
    radio: Radio<P, Powered>,
    phy: PhyState,
    calibration_cache: Option<PhyCalibrationCache>,
    report: Esp32s31WifiMacStartReport,
}

impl<P> Esp32s31WifiMacReady<P> {
    pub const fn report(&self) -> Esp32s31WifiMacStartReport {
        self.report
    }

    pub const fn calibration_cache(&self) -> Option<&PhyCalibrationCache> {
        self.calibration_cache.as_ref()
    }

    pub fn radio_mut(&mut self) -> &mut Radio<P, Powered> {
        &mut self.radio
    }

    pub fn into_parts(
        self,
    ) -> (
        Radio<P, Powered>,
        PhyState,
        Option<PhyCalibrationCache>,
        Esp32s31WifiMacStartReport,
    ) {
        (self.radio, self.phy, self.calibration_cache, self.report)
    }
}

/// Failed MAC transition retaining the powered radio and calibrated PHY.
pub struct Esp32s31WifiMacStartFailure<P> {
    pub error: MacColdStartError,
    radio: Radio<P, Powered>,
    phy: PhyState,
    calibration_cache: Option<PhyCalibrationCache>,
    wifi_report: Esp32s31WifiColdStartReport,
}

impl<P> Esp32s31WifiMacStartFailure<P> {
    pub fn into_parts(
        self,
    ) -> (
        Radio<P, Powered>,
        PhyState,
        Option<PhyCalibrationCache>,
        Esp32s31WifiColdStartReport,
        MacColdStartError,
    ) {
        (
            self.radio,
            self.phy,
            self.calibration_cache,
            self.wifi_report,
            self.error,
        )
    }
}

/// Perform the common MAC transition exactly once after PHY calibration.
pub fn start_esp32s31_wifi_mac<P>(
    cold: Esp32s31WifiColdStart<P>,
    config: Esp32s31WifiMacStartConfig,
) -> Result<Esp32s31WifiMacReady<P>, Esp32s31WifiMacStartFailure<P>>
where
    P: Esp32s31WifiMacPlatform,
{
    let wifi_report = cold.report();
    let (mut radio, phy, tx_power, calibration_cache, _) = cold.into_parts();
    let mac = {
        let (platform, mut mac) = radio.cold_mac_parts();
        platform.install_phy_tx_power_profile(tx_power);
        initialize_wifi_mac(
            platform,
            &mut mac,
            open_esp_radio_esp32s31_wifi_mac::init::MacColdStartConfig {
                handshake_sample_limit: config.handshake_sample_limit,
                station_address: config.station_address.bytes(),
                access_point_address: config.access_point_address.bytes(),
            },
        )
    };
    let mac = match mac {
        Ok(mac) => mac,
        Err(error) => {
            return Err(Esp32s31WifiMacStartFailure {
                error,
                radio,
                phy,
                calibration_cache,
                wifi_report,
            });
        }
    };
    Ok(Esp32s31WifiMacReady {
        radio,
        phy,
        calibration_cache,
        report: Esp32s31WifiMacStartReport {
            wifi: wifi_report,
            mac,
        },
    })
}
