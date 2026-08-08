#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

#[cfg(feature = "wifi")]
pub mod config;

#[cfg(feature = "wifi")]
pub use config::{
    RadioCapabilities, RadioCoexistenceCapabilities, RadioConfig, RadioConfigError, RadioPlan,
    RadioSubsystem, RadioSubsystems, WifiAccessPointConfig, WifiConfig, WifiConfigError,
    WifiMacAddress, WifiMacAddressError, WifiMonitorConfig, WifiPlan, WifiStandaloneMonitorPlan,
    WifiStationConfig,
};
#[cfg(feature = "wifi")]
pub use open_esp_radio_wifi_softmac::{
    MonitorDropReason, MonitorFilter, MonitorFrame, MonitorFrameType, MonitorFrameTypeMask,
    MonitorPublishOutcome, MonitorSink,
};

#[cfg(feature = "wifi")]
pub mod wifi {
    pub use open_esp_radio_ieee80211 as ieee80211;
    pub use open_esp_radio_wifi_softmac as softmac;
    pub use open_esp_radio_wifi_sta as sta;
    pub use open_esp_radio_wpa2 as wpa2;
}

#[cfg(any(feature = "adapter-embassy-net", feature = "adapter-embassy-wifi"))]
pub mod adapters {
    #[cfg(feature = "adapter-embassy-net")]
    pub mod network {
        pub use open_esp_radio_embassy_net as embassy_net;
    }

    #[cfg(feature = "adapter-embassy-wifi")]
    pub mod wifi {
        pub use open_esp_radio_wifi_embassy as embassy;
    }
}

#[cfg(feature = "esp32s31")]
pub mod esp32s31 {
    pub use open_esp_radio_esp32s31_hal as hal;
    pub use open_esp_radio_esp32s31_pac as pac;
    pub use open_esp_radio_esp32s31_phy as phy;
    pub use open_esp_radio_esp32s31_registers as registers;

    #[cfg(feature = "esp32s31-wifi")]
    pub mod wifi {
        pub use open_esp_radio_esp32s31_wifi as device;
        pub use open_esp_radio_esp32s31_wifi_dma as dma;
        pub use open_esp_radio_esp32s31_wifi_mac as mac;
        pub use open_esp_radio_esp32s31_wifi_sta as sta;

        #[cfg(feature = "esp32s31-wifi-embassy")]
        pub mod embassy {
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
                        Esp32s31MonitorTaskBuildFailure, Esp32s31MonitorTaskResources,
                        prepare_esp32s31_monitor_task,
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

    #[cfg(feature = "esp32s31-wifi")]
    pub const RADIO_CAPABILITIES: crate::RadioCapabilities = crate::RadioCapabilities::wifi_only(
        open_esp_radio_esp32s31_wifi_mac::capabilities::ESP32S31_MAC_SERVICE_CAPABILITIES,
    );

    #[cfg(all(feature = "esp32s31-wifi", target_arch = "riscv32"))]
    mod start;
    #[cfg(all(feature = "esp32s31-wifi", target_arch = "riscv32"))]
    pub use start::{
        Esp32s31MonitorMacStartFailure, Esp32s31MonitorReady, Esp32s31PreparedMonitor,
        Esp32s31PreparedStation, Esp32s31RadioStartConfig, Esp32s31RadioStartFailure,
        Esp32s31RoleMaterializationFailure, Esp32s31RoleMaterializationReason,
        Esp32s31StartedRadio, Esp32s31StationMacReady, Esp32s31StationMacStartFailure,
        Esp32s31WifiMacPlatform, Esp32s31WifiMacReady, Esp32s31WifiMacStartConfig,
        Esp32s31WifiMacStartFailure, Esp32s31WifiMacStartReport,
        Esp32s31WifiRuntimeTransitionReport, Esp32s31WifiStart, Esp32s31WifiStartConfig,
        Esp32s31WifiStartFailure, Esp32s31WifiStopped, enter_esp32s31_wifi_runtime,
        start_esp32s31_radio,
    };
}
