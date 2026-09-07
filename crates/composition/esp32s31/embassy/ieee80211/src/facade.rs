//! Public application boundary for the ESP32-S31 radio composition.

use crate::{WifiDevice, WifiDevices, monitor::MonitorFrames};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

use oer_radio::{
    runtime::embassy::{EmbassyWifiStartKind, EmbassyWifiSupervisorPort},
    wifi::{WifiIdle, WifiServicePlanningError},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RadioError {
    Planning(WifiServicePlanningError),
    RoleActive(EmbassyWifiStartKind),
    HardwareFault,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NewError {
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
pub type WifiControl =
    WifiIdle<EmbassyWifiSupervisorPort<'static, CriticalSectionRawMutex, RadioError>>;

/// Materialized Wi-Fi application resources. The network device and monitor
/// capture stream are independent consumers of the same supervised radio.
pub struct WifiSystem {
    control: WifiControl,
    devices: WifiDevices,
    monitor_frames: MonitorFrames,
    station_status: crate::StationStatus,
    access_point_status: crate::AccessPointStatus,
    #[cfg(feature = "diagnostics")]
    diagnostics: crate::DiagnosticSnapshot,
}

/// Named application capabilities materialized from the Wi-Fi subsystem.
/// PAC, DMA and interrupt state remain exclusively in [`SystemRunner`].
pub struct WifiParts {
    pub control: WifiControl,
    pub station_device: WifiDevice,
    pub access_point_device: WifiDevice,
    pub monitor_frames: MonitorFrames,
    pub station_status: crate::StationStatus,
    pub access_point_status: crate::AccessPointStatus,
    #[cfg(feature = "diagnostics")]
    pub diagnostics: crate::DiagnosticSnapshot,
}

impl WifiSystem {
    pub(super) fn new(
        control: WifiControl,
        devices: WifiDevices,
        monitor_frames: MonitorFrames,
        #[cfg(feature = "diagnostics")] diagnostics: crate::DiagnosticSnapshot,
    ) -> Self {
        Self {
            control,
            devices,
            monitor_frames,
            station_status: crate::StationStatus::new(),
            access_point_status: crate::AccessPointStatus::new(),
            #[cfg(feature = "diagnostics")]
            diagnostics,
        }
    }

    pub fn into_parts(self) -> WifiParts {
        WifiParts {
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
pub struct RadioInstance {
    wifi: WifiSystem,
    initialization: RadioInitialization,
}

/// Named subsystem capabilities returned by the radio root.
pub struct RadioParts {
    pub wifi: WifiSystem,
    pub initialization: RadioInitialization,
}

impl RadioInstance {
    pub(super) const fn new(wifi: WifiSystem, initialization: RadioInitialization) -> Self {
        Self {
            wifi,
            initialization,
        }
    }

    pub fn into_parts(self) -> RadioParts {
        RadioParts {
            wifi: self.wifi,
            initialization: self.initialization,
        }
    }
}

/// Value-only cold-start evidence available without exposing PHY, register or
/// calibration owners.
pub struct RadioInitialization {
    pub start: oer_esp32s31_wifi::mac_start::WifiMacStartReport,
    pub transition: oer_esp32s31_wifi::runtime::WifiRuntimeTransitionReport,
    pub calibration_cache: Option<oer_esp32s31_phy::PhyCalibrationCache>,
}
