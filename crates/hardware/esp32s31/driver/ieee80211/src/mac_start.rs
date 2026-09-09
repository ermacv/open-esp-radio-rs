//! Executor-neutral transition from calibrated PHY ownership to cold MAC ownership.

use crate::cold_start::{WifiColdStart, WifiColdStartReport};

use oer_esp32s31_phy::{PhyCalibrationCache, PhyTxTargetPowerProfile, RegisteredPhyRadio};

use oer_esp32s31_wifi_mac::init::{
    MacCoexPtiSource, MacColdStartError, MacColdStartOutcome, MacDelayEntropy,
    MacSlowClockCalibrationSource, MacTxPowerSource, initialize_wifi_mac,
};

use oer_wifi_softmac::WifiMacAddress;

/// Platform operations needed to join the calibrated PHY power table to the
/// finite MAC initializer.
///
/// The installation method is deliberately semantic: MAC code never borrows
/// the PHY parameter arena and never depends on an ESP-HAL singleton type.
pub trait WifiMacPlatform:
    MacCoexPtiSource + MacDelayEntropy + MacSlowClockCalibrationSource + MacTxPowerSource
{
    fn install_phy_tx_power_profile(&mut self, profile: PhyTxTargetPowerProfile);
}

/// Role-neutral inputs for the common Wi-Fi MAC transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiMacStartConfig {
    handshake_sample_limit: u32,
    station_address: WifiMacAddress,
    access_point_address: WifiMacAddress,
}

impl WifiMacStartConfig {
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
pub struct WifiMacStartReport {
    pub wifi: WifiColdStartReport,
    pub mac: MacColdStartOutcome,
}

/// Powered radio after common MAC initialization but before role-specific RX,
/// DMA and interrupt policy is activated.
pub struct WifiMacReady<P> {
    radio: RegisteredPhyRadio<P>,
    calibration_cache: Option<PhyCalibrationCache>,
    report: WifiMacStartReport,
}

impl<P> WifiMacReady<P> {
    pub const fn report(&self) -> WifiMacStartReport {
        self.report
    }

    pub const fn calibration_cache(&self) -> Option<&PhyCalibrationCache> {
        self.calibration_cache.as_ref()
    }

    pub fn radio_mut(&mut self) -> &mut RegisteredPhyRadio<P> {
        &mut self.radio
    }

    pub fn into_parts(
        self,
    ) -> (
        RegisteredPhyRadio<P>,
        Option<PhyCalibrationCache>,
        WifiMacStartReport,
    ) {
        (self.radio, self.calibration_cache, self.report)
    }
}

/// Failed MAC transition retaining the powered radio and calibrated PHY.
pub struct WifiMacStartFailure<P> {
    pub error: MacColdStartError,
    radio: RegisteredPhyRadio<P>,
    calibration_cache: Option<PhyCalibrationCache>,
    wifi_report: WifiColdStartReport,
}

impl<P> WifiMacStartFailure<P> {
    pub fn into_parts(
        self,
    ) -> (
        RegisteredPhyRadio<P>,
        Option<PhyCalibrationCache>,
        WifiColdStartReport,
        MacColdStartError,
    ) {
        (
            self.radio,
            self.calibration_cache,
            self.wifi_report,
            self.error,
        )
    }
}

/// Perform the common MAC transition exactly once after PHY calibration.
///
/// Failure returns the powered radio, calibrated PHY and optional calibration
/// cache together. These affine owners remain inline in both outcomes so the
/// caller can recover the exact initialization frontier without an allocator.
#[expect(
    clippy::result_large_err,
    reason = "MAC failure returns the unique powered radio, calibrated PHY and calibration cache inline; boxing requires an allocator and dropping them loses the recovery frontier"
)]
pub fn start_esp32s31_wifi_mac<P>(
    cold: WifiColdStart<P>,
    config: WifiMacStartConfig,
) -> Result<WifiMacReady<P>, WifiMacStartFailure<P>>
where
    P: WifiMacPlatform,
{
    let wifi_report = cold.report();
    let (mut radio, tx_power, calibration_cache, _) = cold.into_parts();
    let mac = {
        let (platform, mut mac) = radio.cold_mac_parts();
        platform.install_phy_tx_power_profile(tx_power);
        initialize_wifi_mac(
            platform,
            &mut mac,
            oer_esp32s31_wifi_mac::init::MacColdStartConfig {
                handshake_sample_limit: config.handshake_sample_limit,
                station_address: config.station_address.bytes(),
                access_point_address: config.access_point_address.bytes(),
            },
        )
    };
    let mac = match mac {
        Ok(mac) => mac,
        Err(error) => {
            return Err(WifiMacStartFailure {
                error,
                radio,
                calibration_cache,
                wifi_report,
            });
        }
    };
    Ok(WifiMacReady {
        radio,
        calibration_cache,
        report: WifiMacStartReport {
            wifi: wifi_report,
            mac,
        },
    })
}
