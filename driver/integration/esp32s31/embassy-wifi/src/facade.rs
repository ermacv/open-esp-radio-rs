//! Public application boundary for the ESP32-S31 radio composition.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use open_esp_radio::{
    WifiIdle, WifiServicePlanningError,
    embassy_supervisor::{EmbassyWifiStartKind, EmbassyWifiSupervisorPort},
};

use crate::{Esp32s31WifiDevice, Esp32s31WifiDevices, monitor::Esp32s31MonitorFrames};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31RadioError {
    Planning(WifiServicePlanningError),
    RoleActive(EmbassyWifiStartKind),
    HardwareFault,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31NewError {
    RadioAlreadyClaimed,
    RadioStart,
    StationRole,
    MacStart,
    StationMemoryInUse,
    RxDmaLayout,
    TxDmaLayout,
    ConnectedResources,
    MonitorResources,
    SupervisorInUse,
}

/// Hardware-free Wi-Fi typestate root for the sole ESP32-S31 radio runner.
pub type Esp32s31WifiControl =
    WifiIdle<EmbassyWifiSupervisorPort<'static, CriticalSectionRawMutex, Esp32s31RadioError>>;

/// Materialized Wi-Fi application resources. The network device and monitor
/// capture stream are independent consumers of the same supervised radio.
pub struct Esp32s31Wifi {
    control: Esp32s31WifiControl,
    devices: Esp32s31WifiDevices,
    monitor_frames: Esp32s31MonitorFrames,
    station_status: crate::Esp32s31StationStatus,
    access_point_status: crate::Esp32s31AccessPointStatus,
    #[cfg(feature = "diagnostics")]
    diagnostics: crate::Esp32s31DiagnosticSnapshot,
}

/// Named application capabilities materialized from the Wi-Fi subsystem.
/// PAC, DMA and interrupt state remain exclusively in [`Esp32s31RadioRunner`].
pub struct Esp32s31WifiParts {
    pub control: Esp32s31WifiControl,
    pub station_device: Esp32s31WifiDevice,
    pub access_point_device: Esp32s31WifiDevice,
    pub monitor_frames: Esp32s31MonitorFrames,
    pub station_status: crate::Esp32s31StationStatus,
    pub access_point_status: crate::Esp32s31AccessPointStatus,
    #[cfg(feature = "diagnostics")]
    pub diagnostics: crate::Esp32s31DiagnosticSnapshot,
}

impl Esp32s31Wifi {
    pub(super) fn new(
        control: Esp32s31WifiControl,
        devices: Esp32s31WifiDevices,
        monitor_frames: Esp32s31MonitorFrames,
        #[cfg(feature = "diagnostics")] diagnostics: crate::Esp32s31DiagnosticSnapshot,
    ) -> Self {
        Self {
            control,
            devices,
            monitor_frames,
            station_status: crate::Esp32s31StationStatus::new(),
            access_point_status: crate::Esp32s31AccessPointStatus::new(),
            #[cfg(feature = "diagnostics")]
            diagnostics,
        }
    }

    pub fn into_parts(self) -> Esp32s31WifiParts {
        Esp32s31WifiParts {
            control: self.control,
            station_device: self.devices.station,
            access_point_device: self.devices.access_point,
            monitor_frames: self.monitor_frames,
            station_status: self.station_status,
            access_point_status: self.access_point_status,
            #[cfg(feature = "diagnostics")]
            diagnostics: self.diagnostics,
        }
    }
}

/// Sole application radio root. Consuming it materializes Wi-Fi exactly once;
/// future BLE/802.15.4 roots remain owned by this boundary until implemented.
pub struct Esp32s31Radio {
    wifi: Esp32s31Wifi,
    initialization: Esp32s31RadioInitialization,
}

/// Named subsystem capabilities returned by the radio root.
pub struct Esp32s31RadioParts {
    pub wifi: Esp32s31Wifi,
    pub initialization: Esp32s31RadioInitialization,
}

impl Esp32s31Radio {
    pub(super) const fn new(
        wifi: Esp32s31Wifi,
        initialization: Esp32s31RadioInitialization,
    ) -> Self {
        Self {
            wifi,
            initialization,
        }
    }

    pub fn into_parts(self) -> Esp32s31RadioParts {
        Esp32s31RadioParts {
            wifi: self.wifi,
            initialization: self.initialization,
        }
    }
}

/// Value-only cold-start evidence available without exposing PHY, register or
/// calibration owners.
pub struct Esp32s31RadioInitialization {
    pub start: open_esp_radio_esp32s31_wifi::mac_start::Esp32s31WifiMacStartReport,
    pub transition: open_esp_radio_esp32s31_wifi::runtime::Esp32s31WifiRuntimeTransitionReport,
    pub calibration_cache: Option<open_esp_radio_esp32s31_phy::PhyCalibrationCache>,
}
