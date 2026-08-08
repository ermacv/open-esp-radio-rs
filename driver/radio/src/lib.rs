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
    MonitorDropReason, MonitorFrame, MonitorPublishOutcome, MonitorSink,
};

#[cfg(feature = "wifi")]
pub mod wifi {
    pub use open_esp_radio_ieee80211 as ieee80211;
    pub use open_esp_radio_wifi_softmac as softmac;
    pub use open_esp_radio_wifi_sta as sta;
    pub use open_esp_radio_wpa2 as wpa2;

    /// Compatibility alias for the former ambiguous layer name.
    #[doc(hidden)]
    pub use open_esp_radio_wifi_softmac as lmac;
}

#[cfg(any(
    feature = "adapter-embassy-net",
    feature = "adapter-embassy-wifi",
    feature = "esp32s31-wifi-embassy"
))]
pub mod adapters {
    #[cfg(feature = "adapter-embassy-net")]
    pub mod network {
        pub use open_esp_radio_embassy_net as embassy_net;
    }

    #[cfg(feature = "adapter-embassy-wifi")]
    pub mod wifi {
        pub use open_esp_radio_wifi_embassy as embassy;
    }

    #[cfg(feature = "esp32s31-wifi-embassy")]
    pub mod esp32s31 {
        pub use open_esp_radio_esp32s31_wifi_embassy as wifi_embassy;
    }
}

/// Compatibility exports for the former adapter namespace.
#[cfg(any(feature = "adapter-embassy-net", feature = "esp32s31-wifi-embassy"))]
#[doc(hidden)]
pub mod integration {
    pub use crate::adapters::*;
}

#[cfg(feature = "esp32s31")]
pub mod esp32s31 {
    pub use open_esp_radio_esp32s31_hal as hal;
    pub use open_esp_radio_esp32s31_pac as pac;
    pub use open_esp_radio_esp32s31_phy as phy;
    pub use open_esp_radio_esp32s31_registers as registers;

    #[cfg(feature = "esp32s31-wifi")]
    pub mod wifi {
        pub use open_esp_radio_esp32s31_wifi_dma as dma;
        pub use open_esp_radio_esp32s31_wifi_mac as mac;
        pub use open_esp_radio_esp32s31_wifi_sta as sta;

        /// Compatibility alias for the former chip-backend name.
        #[doc(hidden)]
        pub use open_esp_radio_esp32s31_wifi_mac as lmac;

        #[cfg(feature = "esp32s31-wifi-embassy")]
        pub mod embassy {
            pub use open_esp_radio_esp32s31_wifi_embassy::station;

            pub mod monitor {
                pub use open_esp_radio_esp32s31_wifi_embassy::{
                    monitor_rx::{
                        Esp32s31MonitorConfigError, Esp32s31MonitorPrepareError,
                        Esp32s31MonitorPrepareFailure, Esp32s31MonitorRx,
                        Esp32s31MonitorRxProgress,
                    },
                    monitor_service::{
                        ESP32S31_STANDALONE_MONITOR_INTERRUPT_MASK, Esp32s31MonitorCleanupError,
                        Esp32s31MonitorRunError, Esp32s31MonitorRunFailure,
                        Esp32s31MonitorRunReport, Esp32s31MonitorService,
                    },
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
        Esp32s31RadioStartConfig, Esp32s31RadioStartFailure, Esp32s31StartedRadio,
        Esp32s31WifiStart, Esp32s31WifiStartConfig, Esp32s31WifiStartFailure, start_esp32s31_radio,
    };
}
