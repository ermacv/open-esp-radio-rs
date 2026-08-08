//! ESP32-S31 application materialization of a validated radio topology.

use open_esp_radio_esp32s31_hal::Radio;
use open_esp_radio_esp32s31_phy::{
    PhyAsyncDelay, PhyTargetObserver, phy_cold::PhyCalibrationRecord,
};
use open_esp_radio_esp32s31_wifi::cold_start::start_esp32s31_wifi;
pub use open_esp_radio_esp32s31_wifi::cold_start::{
    Esp32s31WifiColdStart as Esp32s31WifiStart,
    Esp32s31WifiColdStartConfig as Esp32s31WifiStartConfig,
    Esp32s31WifiColdStartFailure as Esp32s31WifiStartFailure,
};
use open_esp_radio_esp32s31_wifi::mac_start::start_esp32s31_wifi_mac;
pub use open_esp_radio_esp32s31_wifi::mac_start::{
    Esp32s31WifiMacPlatform, Esp32s31WifiMacReady, Esp32s31WifiMacStartConfig,
    Esp32s31WifiMacStartFailure, Esp32s31WifiMacStartReport,
};
pub use open_esp_radio_esp32s31_wifi::runtime::{
    Esp32s31WifiRuntimeTransitionReport, Esp32s31WifiStopped, enter_esp32s31_wifi_runtime,
};

use crate::{RadioConfig, RadioConfigError, RadioPlan, WifiMacAddress};
use open_esp_radio_wifi_softmac::{
    WifiPlan, WifiStandaloneMonitorPlan, interface::BoundVirtualInterface,
};

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
    /// Materialize the checked exclusive station role before any STA resource
    /// owner is constructed.
    pub fn try_into_station(
        self,
    ) -> Result<Esp32s31PreparedStation<P>, Esp32s31RoleMaterializationFailure<P>> {
        let Some(wifi_plan) = self.plan.wifi() else {
            return Err(Esp32s31RoleMaterializationFailure {
                reason: Esp32s31RoleMaterializationReason::MissingWifi,
                started: self,
            });
        };
        let Some(interface) = wifi_plan.station() else {
            return Err(Esp32s31RoleMaterializationFailure {
                reason: Esp32s31RoleMaterializationReason::MissingStation,
                started: self,
            });
        };
        Ok(Esp32s31PreparedStation {
            interface,
            plan: wifi_plan,
            wifi: self.wifi,
        })
    }

    /// Materialize exclusive monitor ownership. A station/AP topology cannot
    /// be narrowed to this type.
    pub fn try_into_standalone_monitor(
        self,
    ) -> Result<Esp32s31PreparedMonitor<P>, Esp32s31RoleMaterializationFailure<P>> {
        let Some(plan) = self.plan.standalone_wifi_monitor() else {
            return Err(Esp32s31RoleMaterializationFailure {
                reason: Esp32s31RoleMaterializationReason::NotStandaloneMonitor,
                started: self,
            });
        };
        Ok(Esp32s31PreparedMonitor {
            plan,
            wifi: self.wifi,
        })
    }
}

/// Role mismatch detected after cold start but before role-specific resources
/// move. The complete started radio is returned for another materialization or
/// explicit recovery policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31RoleMaterializationReason {
    MissingWifi,
    MissingStation,
    NotStandaloneMonitor,
}

pub struct Esp32s31RoleMaterializationFailure<P> {
    pub reason: Esp32s31RoleMaterializationReason,
    started: Esp32s31StartedRadio<P>,
}

impl<P> Esp32s31RoleMaterializationFailure<P> {
    pub fn into_started(self) -> Esp32s31StartedRadio<P> {
        self.started
    }
}

/// Powered/calibrated prerequisites narrowed to one STA interface.
///
/// This is not yet a running station: a runtime adapter must consume it with
/// the station's DMA, interrupt and protocol resources to create the service.
pub struct Esp32s31PreparedStation<P> {
    interface: BoundVirtualInterface,
    plan: WifiPlan,
    wifi: Esp32s31WifiStart<P>,
}

impl<P> Esp32s31PreparedStation<P> {
    pub const fn interface(&self) -> BoundVirtualInterface {
        self.interface
    }

    pub const fn plan(&self) -> WifiPlan {
        self.plan
    }

    pub fn into_parts(self) -> (WifiPlan, Esp32s31WifiStart<P>) {
        (self.plan, self.wifi)
    }
}

impl<P: Esp32s31WifiMacPlatform> Esp32s31PreparedStation<P> {
    /// Perform common MAC initialization while retaining the checked station
    /// identity as part of the resulting role owner.
    pub fn start_mac(
        self,
        handshake_sample_limit: u32,
        access_point_hardware_address: WifiMacAddress,
    ) -> Result<Esp32s31StationMacReady<P>, Esp32s31StationMacStartFailure<P>> {
        let station_hardware_address = WifiMacAddress::new(self.interface.interface.address)
            .expect("a validated station plan contains a unicast address");
        match start_esp32s31_wifi_mac(
            self.wifi,
            Esp32s31WifiMacStartConfig::new(
                handshake_sample_limit,
                station_hardware_address,
                access_point_hardware_address,
            ),
        ) {
            Ok(mac) => Ok(Esp32s31StationMacReady {
                interface: self.interface,
                plan: self.plan,
                mac,
            }),
            Err(failure) => Err(Esp32s31StationMacStartFailure {
                interface: self.interface,
                plan: self.plan,
                failure,
            }),
        }
    }
}

/// Station topology and powered MAC owner ready for scan/DMA resources.
pub struct Esp32s31StationMacReady<P> {
    interface: BoundVirtualInterface,
    plan: WifiPlan,
    mac: Esp32s31WifiMacReady<P>,
}

impl<P> Esp32s31StationMacReady<P> {
    pub const fn interface(&self) -> BoundVirtualInterface {
        self.interface
    }

    pub const fn plan(&self) -> WifiPlan {
        self.plan
    }

    pub fn into_parts(self) -> (WifiPlan, Esp32s31WifiMacReady<P>) {
        (self.plan, self.mac)
    }
}

/// Failed station MAC transition retaining its role identity and hardware.
pub struct Esp32s31StationMacStartFailure<P> {
    interface: BoundVirtualInterface,
    plan: WifiPlan,
    failure: Esp32s31WifiMacStartFailure<P>,
}

impl<P> Esp32s31StationMacStartFailure<P> {
    pub const fn interface(&self) -> BoundVirtualInterface {
        self.interface
    }

    pub const fn plan(&self) -> WifiPlan {
        self.plan
    }

    pub fn into_parts(self) -> (WifiPlan, Esp32s31WifiMacStartFailure<P>) {
        (self.plan, self.failure)
    }
}

/// Powered/calibrated prerequisites narrowed to exclusive monitor use.
///
/// RX DMA and interrupt ownership have not started at this boundary.
pub struct Esp32s31PreparedMonitor<P> {
    plan: WifiStandaloneMonitorPlan,
    wifi: Esp32s31WifiStart<P>,
}

impl<P> Esp32s31PreparedMonitor<P> {
    pub const fn plan(&self) -> WifiStandaloneMonitorPlan {
        self.plan
    }

    pub fn into_parts(self) -> (WifiStandaloneMonitorPlan, Esp32s31WifiStart<P>) {
        (self.plan, self.wifi)
    }
}

impl<P: Esp32s31WifiMacPlatform> Esp32s31PreparedMonitor<P> {
    /// Perform common MAC initialization before the standalone monitor policy
    /// is activated and DMA/IRQ resources move into the monitor service.
    pub fn start_mac(
        self,
        handshake_sample_limit: u32,
        station_hardware_address: WifiMacAddress,
        access_point_hardware_address: WifiMacAddress,
    ) -> Result<Esp32s31MonitorReady<P>, Esp32s31MonitorMacStartFailure<P>> {
        match start_esp32s31_wifi_mac(
            self.wifi,
            Esp32s31WifiMacStartConfig::new(
                handshake_sample_limit,
                station_hardware_address,
                access_point_hardware_address,
            ),
        ) {
            Ok(mac) => Ok(Esp32s31MonitorReady {
                plan: self.plan,
                wifi: enter_esp32s31_wifi_runtime(mac),
            }),
            Err(failure) => Err(Esp32s31MonitorMacStartFailure {
                plan: self.plan,
                failure,
            }),
        }
    }
}

/// Exclusive monitor plan joined to the common stopped Wi-Fi runtime owner.
pub struct Esp32s31MonitorReady<P> {
    plan: WifiStandaloneMonitorPlan,
    wifi: Esp32s31WifiStopped<P>,
}

impl<P> Esp32s31MonitorReady<P> {
    pub const fn plan(&self) -> WifiStandaloneMonitorPlan {
        self.plan
    }

    pub fn into_parts(self) -> (WifiStandaloneMonitorPlan, Esp32s31WifiStopped<P>) {
        (self.plan, self.wifi)
    }
}

/// Failed monitor MAC transition retaining its exclusive role plan.
pub struct Esp32s31MonitorMacStartFailure<P> {
    plan: WifiStandaloneMonitorPlan,
    failure: Esp32s31WifiMacStartFailure<P>,
}

impl<P> Esp32s31MonitorMacStartFailure<P> {
    pub const fn plan(&self) -> WifiStandaloneMonitorPlan {
        self.plan
    }

    pub fn into_parts(self) -> (WifiStandaloneMonitorPlan, Esp32s31WifiMacStartFailure<P>) {
        (self.plan, self.failure)
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
    let wifi = start_esp32s31_wifi::<P, D, O>(radio, config.wifi, calibration_record, observer)
        .await
        .map_err(Esp32s31RadioStartFailure::Wifi)?;
    Ok(Esp32s31StartedRadio { plan, wifi })
}
