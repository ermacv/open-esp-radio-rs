//! ESP32-S31 application materialization of a validated radio topology.

use open_esp_radio_esp32s31_hal::Radio;
use open_esp_radio_esp32s31_phy::{
    PhyAsyncDelay, PhyTargetObserver, phy_cold::PhyCalibrationRecord,
};
use open_esp_radio_esp32s31_wifi_sta::cold_start::start_esp32s31_station_radio;
pub use open_esp_radio_esp32s31_wifi_sta::cold_start::{
    Esp32s31ColdStart as Esp32s31WifiStart, Esp32s31ColdStartConfig as Esp32s31WifiStartConfig,
    Esp32s31ColdStartFailure as Esp32s31WifiStartFailure,
};

use crate::{RadioConfig, RadioConfigError, RadioPlan};

/// Application inputs for topology validation and the common Wi-Fi cold start.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31RadioStartConfig {
    topology: RadioConfig,
    wifi: Esp32s31WifiStartConfig,
}

impl Esp32s31RadioStartConfig {
    pub const fn new(topology: RadioConfig, wifi: Esp32s31WifiStartConfig) -> Self {
        Self { topology, wifi }
    }

    pub const fn topology(self) -> RadioConfig {
        self.topology
    }

    pub const fn wifi(self) -> Esp32s31WifiStartConfig {
        self.wifi
    }
}

/// Powered/calibrated ESP32-S31 radio together with its exact owner topology.
pub struct Esp32s31StartedRadio<P> {
    plan: RadioPlan,
    wifi: Esp32s31WifiStart<P>,
}

impl<P> Esp32s31StartedRadio<P> {
    pub const fn plan(&self) -> RadioPlan {
        self.plan
    }

    pub const fn wifi(&self) -> &Esp32s31WifiStart<P> {
        &self.wifi
    }

    /// Checked exclusive monitor topology selected for this powered radio.
    pub const fn standalone_monitor_plan(&self) -> Option<crate::WifiStandaloneMonitorPlan> {
        self.plan.standalone_wifi_monitor()
    }

    pub fn into_parts(self) -> (RadioPlan, Esp32s31WifiStart<P>) {
        (self.plan, self.wifi)
    }
}

/// Failed start retaining the unique radio owner at its exact phase.
pub enum Esp32s31RadioStartFailure<P> {
    Configuration {
        radio: Radio<P>,
        error: RadioConfigError,
    },
    Wifi(Esp32s31WifiStartFailure<P>),
}

impl<P> Esp32s31RadioStartFailure<P> {
    pub const fn configuration_error(&self) -> Option<RadioConfigError> {
        match self {
            Self::Configuration { error, .. } => Some(*error),
            Self::Wifi(_) => None,
        }
    }
}

/// Validate the requested owner graph against the concrete S31 backend, then
/// perform the shared RF/PHY/Wi-Fi cold transition.
///
/// Role-specific DMA, executor and protocol storage are intentionally not
/// consumed here. They materialize later from [`Esp32s31StartedRadio::plan`].
pub async fn start_esp32s31_radio<P, D, O>(
    radio: Radio<P>,
    config: Esp32s31RadioStartConfig,
    calibration_record: Option<PhyCalibrationRecord>,
    observer: O,
) -> Result<Esp32s31StartedRadio<P>, Esp32s31RadioStartFailure<P>>
where
    P: open_esp_radio_esp32s31_hal::PowerClockControl
        + open_esp_radio_esp32s31_hal::phy_prelude::PhyPreludePlatformControl
        + open_esp_radio_esp32s31_hal::analog_i2c::PhyPmuControl
        + open_esp_radio_esp32s31_hal::wifi_bb::PhyWifiBbControl
        + open_esp_radio_esp32s31_hal::power_detector_platform::PhyPowerDetectorPlatformControl
        + open_esp_radio_esp32s31_hal::phy_temperature::PhyTemperatureSystemControl
        + open_esp_radio_esp32s31_hal::phy_i2c::PhyI2cMasterControl,
    D: PhyAsyncDelay,
    O: PhyTargetObserver + Clone,
{
    let plan = match config.topology.validate(super::RADIO_CAPABILITIES) {
        Ok(plan) => plan,
        Err(error) => {
            return Err(Esp32s31RadioStartFailure::Configuration { radio, error });
        }
    };
    let wifi =
        start_esp32s31_station_radio::<P, D, O>(radio, config.wifi, calibration_record, observer)
            .await
            .map_err(Esp32s31RadioStartFailure::Wifi)?;
    Ok(Esp32s31StartedRadio { plan, wifi })
}
