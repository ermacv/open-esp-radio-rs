//! Wi-Fi configuration, service requests and affine role lifecycle.
//!
//! These contracts are independent of the executor used to drive the radio.

mod config;
pub mod requests;
pub mod supervisor;

pub use config::{
    WifiAccessPointConfig, WifiConfig, WifiConfigError, WifiMacAddress, WifiMacAddressError,
    WifiMonitorConfig, WifiPlan, WifiStandaloneEspNowPlan, WifiStandaloneMonitorPlan,
    WifiStationConfig,
};
pub use oer_wifi_softmac::{
    MONITOR_CHANNEL_SEQUENCE_CAPACITY, MacRxEvidence, MonitorChannelPolicy, MonitorChannelSequence,
    MonitorChannelSequenceError, MonitorDropReason, MonitorFilter, MonitorFrame, MonitorFrameType,
    MonitorFrameTypeMask, MonitorPublishOutcome, MonitorSink,
};
pub use oer_wifi_sta::station::{StaLifecycleStage, StaReconnectPolicy};
pub use oer_wpa2::Pmk;
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
pub use supervisor::{
    RadioController, RadioSubsystemGeneration, WIFI_SCAN_RESULT_CAPACITY, WifiAccessPoint,
    WifiIdle, WifiMonitor, WifiRoleStartFailure, WifiRoleStopFailure, WifiScanCompleted,
    WifiScanFailure, WifiScanOperationFailure, WifiScanReport, WifiScanResult, WifiStartFailure,
    WifiStartReport, WifiStartResult, WifiStation, WifiStationAccessPoint, WifiStopReport,
    WifiSupervisorPort,
};
pub use {
    oer_ieee80211::channel::WifiChannel, oer_ieee80211::channel::WifiChannelError,
    oer_ieee80211::channel::WifiChannelWidth, oer_ieee80211::security::WifiSecurityMode,
    oer_ieee80211::ssid::WifiSsid, oer_ieee80211::ssid::WifiSsidError,
    oer_ieee80211::station::association::Preference,
};

#[cfg(test)]
pub(crate) mod test_support;
