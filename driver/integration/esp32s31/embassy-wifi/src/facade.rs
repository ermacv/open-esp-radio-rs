//! Public application boundary for the ESP32-S31 radio composition.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use open_esp_radio::{
    WifiIdle, WifiServicePlanningError,
    embassy_supervisor::{EmbassyWifiStartKind, EmbassyWifiSupervisorPort},
};

use crate::{connected::Esp32s31WifiDevice, monitor::Esp32s31MonitorFrames};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31RadioError {
    Planning(WifiServicePlanningError),
    RoleActive(EmbassyWifiStartKind),
    UnsupportedPowerPolicy,
    HardwareFault,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31NewError {
    WorkerUnavailable,
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
    device: Esp32s31WifiDevice,
    monitor_frames: Esp32s31MonitorFrames,
    #[cfg(feature = "qualification")]
    qualification: crate::Esp32s31QualificationSnapshot,
}

/// Named application capabilities materialized from the Wi-Fi subsystem.
/// PAC, DMA and interrupt state remain exclusively in [`Esp32s31RadioRunner`].
pub struct Esp32s31WifiParts {
    pub control: Esp32s31WifiControl,
    pub device: Esp32s31WifiDevice,
    pub monitor_frames: Esp32s31MonitorFrames,
    #[cfg(feature = "qualification")]
    pub qualification: crate::Esp32s31QualificationSnapshot,
}

impl Esp32s31Wifi {
    pub(super) fn new(
        control: Esp32s31WifiControl,
        device: Esp32s31WifiDevice,
        monitor_frames: Esp32s31MonitorFrames,
        #[cfg(feature = "qualification")] qualification: crate::Esp32s31QualificationSnapshot,
    ) -> Self {
        Self {
            control,
            device,
            monitor_frames,
            #[cfg(feature = "qualification")]
            qualification,
        }
    }

    pub fn into_parts(self) -> Esp32s31WifiParts {
        Esp32s31WifiParts {
            control: self.control,
            device: self.device,
            monitor_frames: self.monitor_frames,
            #[cfg(feature = "qualification")]
            qualification: self.qualification,
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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31RadioInitialization {
    pub start: open_esp_radio::esp32s31::Esp32s31WifiMacStartReport,
    pub transition: open_esp_radio::esp32s31::Esp32s31WifiRuntimeTransitionReport,
    pub calibration_record:
        Option<[u8; open_esp_radio::esp32s31::phy::phy_cold::PHY_COLD_CALIBRATION_RECORD_LEN]>,
}
