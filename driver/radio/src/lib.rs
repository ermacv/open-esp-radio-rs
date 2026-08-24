#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

#[cfg(feature = "wifi")]
mod config;
#[cfg(feature = "wifi-embassy")]
#[doc(hidden)]
pub mod embassy_supervisor;
#[cfg(feature = "wifi")]
pub mod requests;
#[cfg(feature = "wifi")]
pub mod supervisor;

#[cfg(feature = "wifi")]
pub use config::{
    WifiAccessPointConfig, WifiConfig, WifiConfigError, WifiMacAddress, WifiMacAddressError,
    WifiMonitorConfig, WifiPlan, WifiStandaloneEspNowPlan, WifiStandaloneMonitorPlan,
    WifiStationConfig,
};
#[cfg(feature = "wifi")]
pub use open_esp_radio_ieee80211::{
    channel::{WifiChannel, WifiChannelError, WifiChannelWidth},
    security::WifiSecurityMode,
    ssid::{WifiSsid, WifiSsidError},
    station::StaAssociationPreference,
};
#[cfg(feature = "wifi")]
pub use open_esp_radio_wifi_softmac::{
    MONITOR_CHANNEL_SEQUENCE_CAPACITY, MacRxEvidence, MonitorChannelPolicy, MonitorChannelSequence,
    MonitorChannelSequenceError, MonitorDropReason, MonitorFilter, MonitorFrame, MonitorFrameType,
    MonitorFrameTypeMask, MonitorPublishOutcome, MonitorSink,
};
#[cfg(feature = "wifi")]
pub use open_esp_radio_wifi_sta::station::{StaLifecycleStage, StaReconnectPolicy};
#[cfg(feature = "wifi")]
pub use open_esp_radio_wpa2::Pmk;
#[cfg(feature = "wifi")]
pub use requests::{
    AccessPointBeaconInterval, AccessPointBeaconIntervalError, AccessPointClientLimit,
    AccessPointClientLimitError, AccessPointDtimPeriod, AccessPointDtimPeriodError,
    AccessPointInactiveTimeout, AccessPointInactiveTimeoutError, AccessPointRequest,
    AccessPointRequestError, AccessPointSecurity, MonitorCapturePolicy, MonitorRequest,
    StandaloneEspNowPeerError, StandaloneEspNowRequest, StationAccessPointRequest,
    StationDiscovery, StationListenInterval, StationPowerMode, StationPowerSavePolicy,
    StationRequest, StationScanChannelIter, StationScanChannelOrderIter, StationScanChannels,
    StationScanChannelsError, StationScanPolicy, StationSecurity, WifiScanRequest,
    WifiServicePlanningError, WifiServicePlanningFailure, WifiServiceRequest,
    WifiServiceRequestError, WifiServiceRequestFailure, WifiSupervisorConfiguration,
};
#[cfg(feature = "wifi")]
pub use supervisor::{
    RadioController, RadioSubsystemGeneration, WIFI_SCAN_RESULT_CAPACITY, WifiAccessPoint,
    WifiIdle, WifiMonitor, WifiRoleStartFailure, WifiRoleStopFailure, WifiScanCompleted,
    WifiScanFailure, WifiScanOperationFailure, WifiScanReport, WifiScanResult, WifiStartFailure,
    WifiStartReport, WifiStartResult, WifiStation, WifiStationAccessPoint, WifiStopReport,
    WifiSupervisorPort,
};
