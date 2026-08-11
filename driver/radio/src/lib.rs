#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

#[cfg(feature = "wifi")]
mod config;
#[cfg(feature = "esp32s31-wifi-embassy")]
#[doc(hidden)]
pub mod embassy_supervisor;
#[cfg(feature = "wifi")]
pub mod requests;
#[cfg(feature = "wifi")]
pub mod supervisor;

#[cfg(feature = "wifi")]
pub use config::{
    WifiAccessPointConfig, WifiConfig, WifiConfigError, WifiMacAddress, WifiMacAddressError,
    WifiMonitorConfig, WifiPlan, WifiStandaloneMonitorPlan, WifiStationConfig,
};
#[cfg(feature = "wifi")]
pub use open_esp_radio_ieee80211::ssid::{WifiSsid, WifiSsidError};
#[cfg(feature = "wifi")]
pub use open_esp_radio_wifi_softmac::{
    MonitorDropReason, MonitorFilter, MonitorFrame, MonitorFrameType, MonitorFrameTypeMask,
    MonitorPublishOutcome, MonitorSink,
};
#[cfg(feature = "wifi")]
pub use requests::{
    AccessPointRequest, AccessPointRequestError, AccessPointSecurity, MonitorCapturePolicy,
    MonitorRequest, StationDiscovery, StationRequest, StationScanChannelIter,
    StationScanChannelOrderIter, StationScanChannels, StationScanChannelsError, StationScanPolicy,
    StationSecurity, WifiScanRequest, WifiServicePlanningError, WifiServicePlanningFailure,
    WifiServiceRequest, WifiServiceRequestError, WifiServiceRequestFailure,
    WifiSupervisorConfiguration,
};
#[cfg(feature = "wifi")]
pub use supervisor::{
    RadioController, RadioSubsystemGeneration, WIFI_SCAN_RESULT_CAPACITY, WifiAccessPoint,
    WifiIdle, WifiMonitor, WifiRoleStartFailure, WifiRoleStopFailure, WifiScanCompleted,
    WifiScanFailure, WifiScanOperationFailure, WifiScanReport, WifiScanResult, WifiStartFailure,
    WifiStartReport, WifiStartResult, WifiStation, WifiStopReport, WifiSupervisorPort,
};

#[cfg(feature = "wifi")]
pub mod wifi {
    pub use open_esp_radio_ieee80211 as ieee80211;
    pub use open_esp_radio_wifi_ap as ap;
    pub use open_esp_radio_wifi_softmac as softmac;
    pub use open_esp_radio_wifi_sta as sta;
    pub use open_esp_radio_wpa2 as wpa2;
}

#[cfg(feature = "esp32s31")]
pub mod esp32s31 {
    pub use open_esp_radio_esp32s31_hal as hal;
    pub use open_esp_radio_esp32s31_phy as phy;
    pub use open_esp_radio_esp32s31_registers as registers;

    #[cfg(feature = "esp32s31-wifi")]
    pub mod wifi {
        pub use open_esp_radio_esp32s31_wifi as device;
        pub use open_esp_radio_esp32s31_wifi_ap as ap;
        pub use open_esp_radio_esp32s31_wifi_dma as dma;
        pub use open_esp_radio_esp32s31_wifi_mac as mac;
        pub use open_esp_radio_esp32s31_wifi_sta as sta;

        #[cfg(feature = "esp32s31-wifi-embassy")]
        pub mod embassy {
            pub mod resources {
                pub use open_esp_radio_esp32s31_wifi_embassy::resource_profile::{
                    ESP32S31_DEFAULT_CONTROL_QUEUE_DEPTH, ESP32S31_DEFAULT_NETWORK_FRAME_CAPACITY,
                    ESP32S31_DEFAULT_NETWORK_RX_QUEUE_DEPTH,
                    ESP32S31_DEFAULT_NETWORK_TX_QUEUE_DEPTH, ESP32S31_DEFAULT_NETWORK_TX_TRAILER,
                    ESP32S31_DEFAULT_RX_BUFFER_SIZE, ESP32S31_DEFAULT_RX_BUFFER_STORAGE_SIZE,
                    ESP32S31_DEFAULT_RX_DESCRIPTOR_COUNT, ESP32S31_DEFAULT_RX_REORDER_WINDOW,
                    ESP32S31_DEFAULT_RX_STAGE_CAPACITY, ESP32S31_DEFAULT_RX_STAGE_SLOT_COUNT,
                    ESP32S31_DEFAULT_SCAN_FRAME_CAPACITY, ESP32S31_DEFAULT_SCAN_RECORD_CAPACITY,
                    ESP32S31_DEFAULT_TX_AMPDU_FRAME_COUNT, ESP32S31_DEFAULT_TX_BUFFER_SIZE,
                    Esp32s31DefaultRxDmaStorage, Esp32s31DefaultScanTable,
                    Esp32s31DefaultStationMemory, Esp32s31DefaultStationMemoryError,
                    Esp32s31DefaultStationMemoryLease, Esp32s31DefaultWifiResourceProfile,
                };
            }

            pub mod station {
                pub use open_esp_radio_esp32s31_wifi_embassy::station::{
                    Esp32s31StationCommand, Esp32s31StationCompletion, Esp32s31StationConfig,
                    Esp32s31StationControlError, Esp32s31StationControlResources,
                    Esp32s31StationController, Esp32s31StationExit, Esp32s31StationPrepareFailure,
                    Esp32s31StationReturnedResources, Esp32s31StationStartResources,
                    Esp32s31StationStopReason, Esp32s31StationTask, prepare_esp32s31_station_task,
                };
                #[cfg(target_arch = "riscv32")]
                pub use open_esp_radio_esp32s31_wifi_embassy::station::{
                    Esp32s31StationMaterialized, Esp32s31StationPhaseRebindFailure,
                    Esp32s31StationPhaseReclaimFailure, Esp32s31StationPhaseReclaimed,
                    Esp32s31StationPhaseRestoreFailure, Esp32s31StationRoleOwner,
                    Esp32s31StationRuntimeReclaimFailure, Esp32s31StationRuntimeReclaimed,
                    Esp32s31StationStopped, Esp32s31StationStoppedPhaseResources,
                    materialize_esp32s31_station, try_rebind_esp32s31_station_phase,
                    try_reclaim_esp32s31_station_phase, try_reclaim_esp32s31_station_runtime,
                    try_restore_esp32s31_station_phase,
                };
            }

            pub mod monitor {
                pub use open_esp_radio_esp32s31_wifi_embassy::monitor::{
                    Esp32s31MonitorCompletion, Esp32s31MonitorConfigError,
                    Esp32s31MonitorControlError, Esp32s31MonitorControlResources,
                    Esp32s31MonitorController, Esp32s31MonitorPrepareError,
                    Esp32s31MonitorRunError, Esp32s31MonitorRunFailure, Esp32s31MonitorRunReport,
                    Esp32s31MonitorRxProgress, Esp32s31MonitorStopError,
                };
                pub use open_esp_radio_esp32s31_wifi_embassy::{
                    embassy_irq::{EmbassyMacIrqRuntime, EmbassyPowerIrqRuntime},
                    rx_dma_service::Esp32s31RxDmaStorage,
                };
                #[cfg(target_arch = "riscv32")]
                pub use open_esp_radio_esp32s31_wifi_embassy::{
                    monitor::{
                        Esp32s31MonitorBuildError, Esp32s31MonitorBuildReport,
                        Esp32s31MonitorChannelSwitchError, Esp32s31MonitorInterrupts,
                        Esp32s31MonitorMemory, Esp32s31MonitorStopped,
                        Esp32s31MonitorStoppedResources, Esp32s31MonitorTask,
                        Esp32s31MonitorTaskBuildFailure, Esp32s31MonitorTaskExit,
                        Esp32s31MonitorTaskResources, prepare_esp32s31_monitor_task,
                    },
                    phy_delay::EmbassyEsp32s31PhyDelay,
                };
                pub use open_esp_radio_wifi_embassy::{
                    MonitorCaptureFrame, MonitorCaptureMetadata, MonitorCapturePool,
                    MonitorCaptureReceiver, MonitorCaptureResources, MonitorCaptureSink,
                };
            }
        }
    }

    #[cfg(all(feature = "esp32s31-wifi-embassy", target_arch = "riscv32"))]
    pub mod supervisor;

    #[cfg(all(feature = "esp32s31-wifi", target_arch = "riscv32"))]
    mod start;
    #[cfg(all(feature = "esp32s31-wifi", target_arch = "riscv32"))]
    pub use start::{
        Esp32s31RadioReady, Esp32s31RadioStartConfig, Esp32s31RadioStartFailure,
        Esp32s31WifiMacPlatform, Esp32s31WifiMacReady, Esp32s31WifiMacStartConfig,
        Esp32s31WifiMacStartFailure, Esp32s31WifiMacStartReport,
        Esp32s31WifiRuntimeTransitionReport, Esp32s31WifiStart, Esp32s31WifiStartConfig,
        Esp32s31WifiStartFailure, Esp32s31WifiStopped, enter_esp32s31_wifi_runtime,
        start_esp32s31_radio,
    };
}
