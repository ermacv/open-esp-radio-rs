//! Application policy supplied when starting one Wi-Fi service.
//!
//! These values contain no peripheral, DMA or executor ownership.  The
//! topology and VIF identities remain in [`crate::WifiPlan`]; a request only
//! describes how an already validated role should operate for one runtime
//! epoch.

use core::{fmt, num::NonZeroU16};

use open_esp_radio_ieee80211::channel::WifiChannel;
use open_esp_radio_wifi_softmac::{
    MacServiceCapabilities, WifiConfig, WifiConfigError, WifiStationConfig,
};
pub use open_esp_radio_wifi_sta::request::{
    StationDiscovery, StationScanChannelIter, StationScanChannelOrderIter, StationScanChannels,
    StationScanChannelsError, StationScanPolicy, WifiSsid, WifiSsidError,
};
use open_esp_radio_wifi_sta::station::StaReconnectPolicy;
use open_esp_radio_wpa2::Pmk;

use crate::{WifiMonitorConfig, WifiPlan};

/// Security material owned by one station runtime request.
///
/// The current production station path implements WPA2-Personal. The request
/// owns the derived PMK rather than retaining a plaintext passphrase across
/// reconnects. [`Pmk`] clears its key bytes on drop.
pub enum StationSecurity {
    Wpa2Personal(Pmk),
}

impl StationSecurity {
    pub const fn wpa2_personal(pmk: Pmk) -> Self {
        Self::Wpa2Personal(pmk)
    }

    pub const fn pmk(&self) -> &Pmk {
        match self {
            Self::Wpa2Personal(pmk) => pmk,
        }
    }

    pub fn into_pmk(self) -> Pmk {
        match self {
            Self::Wpa2Personal(pmk) => pmk,
        }
    }
}

impl fmt::Debug for StationSecurity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Wpa2Personal(_) => formatter.write_str("Wpa2Personal(<redacted>)"),
        }
    }
}

/// Station RF power policy requested for connected epochs.
///
/// Power-save is a policy request, not a claim that the current backend has
/// already implemented the complete TSF/TIM/RF-owner transaction. A backend
/// must reject an unsupported value before moving the stopped owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationPowerPolicy {
    AlwaysAwake,
    LegacyPowerSave { wake_guard_micros: u32 },
}

/// Complete owned request for one station service epoch.
pub struct StationRequest {
    ssid: WifiSsid,
    security: StationSecurity,
    reconnect: StaReconnectPolicy,
    scan: StationScanPolicy,
    power: StationPowerPolicy,
}

impl StationRequest {
    pub const fn new(
        ssid: WifiSsid,
        security: StationSecurity,
        reconnect: StaReconnectPolicy,
        scan: StationScanPolicy,
    ) -> Self {
        Self {
            ssid,
            security,
            reconnect,
            scan,
            power: StationPowerPolicy::AlwaysAwake,
        }
    }

    pub const fn with_power_policy(mut self, power: StationPowerPolicy) -> Self {
        self.power = power;
        self
    }

    pub const fn ssid(&self) -> &WifiSsid {
        &self.ssid
    }

    pub const fn security(&self) -> &StationSecurity {
        &self.security
    }

    pub const fn reconnect_policy(&self) -> StaReconnectPolicy {
        self.reconnect
    }

    pub const fn scan_policy(&self) -> StationScanPolicy {
        self.scan
    }

    pub const fn power_policy(&self) -> StationPowerPolicy {
        self.power
    }

    pub fn into_parts(
        self,
    ) -> (
        StationDiscovery,
        StationSecurity,
        StaReconnectPolicy,
        StationPowerPolicy,
    ) {
        (
            StationDiscovery::new(self.ssid, self.scan),
            self.security,
            self.reconnect,
            self.power,
        )
    }
}

impl fmt::Debug for StationRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StationRequest")
            .field("ssid", &self.ssid)
            .field("security", &self.security)
            .field("reconnect", &self.reconnect)
            .field("scan", &self.scan)
            .field("power", &self.power)
            .finish()
    }
}

/// Host/export capture policy independent of the DMA ring size.
///
/// Queue capacity and DMA placement belong to `RadioResources<Profile>`.
/// Saturation is always best-effort drop with accounting; no request may turn
/// a slow capture consumer into RX-DMA backpressure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MonitorCapturePolicy {
    snapshot_length: Option<NonZeroU16>,
}

impl MonitorCapturePolicy {
    pub const fn complete_frames() -> Self {
        Self {
            snapshot_length: None,
        }
    }

    pub const fn truncate_at(snapshot_length: NonZeroU16) -> Self {
        Self {
            snapshot_length: Some(snapshot_length),
        }
    }

    pub const fn snapshot_length(self) -> Option<u16> {
        match self.snapshot_length {
            Some(length) => Some(length.get()),
            None => None,
        }
    }
}

impl Default for MonitorCapturePolicy {
    fn default() -> Self {
        Self::complete_frames()
    }
}

/// Complete value-only request for one standalone monitor epoch.
pub struct MonitorRequest {
    channel: WifiChannel,
    monitor: WifiMonitorConfig,
    capture: MonitorCapturePolicy,
}

impl MonitorRequest {
    pub const fn new(channel: WifiChannel, monitor: WifiMonitorConfig) -> Self {
        Self {
            channel,
            monitor,
            capture: MonitorCapturePolicy::complete_frames(),
        }
    }

    pub const fn with_capture_policy(mut self, capture: MonitorCapturePolicy) -> Self {
        self.capture = capture;
        self
    }

    pub const fn channel(&self) -> WifiChannel {
        self.channel
    }

    pub const fn monitor_config(&self) -> WifiMonitorConfig {
        self.monitor
    }

    pub const fn capture_policy(&self) -> MonitorCapturePolicy {
        self.capture
    }
}

impl fmt::Debug for MonitorRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MonitorRequest")
            .field("channel", &self.channel)
            .field("monitor", &self.monitor)
            .field("capture", &self.capture)
            .finish()
    }
}

/// Runtime policies joined to one already capability-checked Wi-Fi topology.
///
/// Optional fields describe independently composable role services; this is
/// deliberately not a mutually exclusive station/monitor mode enum. A later
/// STA+AP owner graph can carry both service requests, and a monitor tap can
/// accompany either when the checked [`WifiPlan`] permits it.
pub struct WifiServiceRequest {
    plan: WifiPlan,
    station: Option<StationRequest>,
    monitor: Option<MonitorRequest>,
}

impl WifiServiceRequest {
    /// Join a checked station-only topology to its runtime policy.
    pub fn station(
        plan: WifiPlan,
        request: StationRequest,
    ) -> Result<Self, WifiServiceRequestFailure<StationRequest>> {
        if plan.station().is_none() {
            return Err(WifiServiceRequestFailure::new(
                request,
                WifiServiceRequestError::MissingStationTopology,
            ));
        }
        if plan.access_point().is_some() || plan.monitor().is_some() {
            return Err(WifiServiceRequestFailure::new(
                request,
                WifiServiceRequestError::UnexpectedTopologyRole,
            ));
        }
        Ok(Self {
            plan,
            station: Some(request),
            monitor: None,
        })
    }

    /// Join a checked standalone-monitor topology to its runtime policy.
    pub fn standalone_monitor(
        plan: WifiPlan,
        request: MonitorRequest,
    ) -> Result<Self, WifiServiceRequestFailure<MonitorRequest>> {
        let Some(checked) = plan.standalone_monitor() else {
            return Err(WifiServiceRequestFailure::new(
                request,
                WifiServiceRequestError::NotStandaloneMonitorTopology,
            ));
        };
        if checked.monitor() != request.monitor_config() {
            return Err(WifiServiceRequestFailure::new(
                request,
                WifiServiceRequestError::MonitorPolicyMismatch,
            ));
        }
        Ok(Self {
            plan,
            station: None,
            monitor: Some(request),
        })
    }

    pub const fn plan(&self) -> WifiPlan {
        self.plan
    }

    pub const fn station_request(&self) -> Option<&StationRequest> {
        self.station.as_ref()
    }

    pub const fn monitor_request(&self) -> Option<&MonitorRequest> {
        self.monitor.as_ref()
    }

    pub fn into_parts(self) -> (WifiPlan, Option<StationRequest>, Option<MonitorRequest>) {
        (self.plan, self.station, self.monitor)
    }
}

impl fmt::Debug for WifiServiceRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WifiServiceRequest")
            .field("plan", &self.plan)
            .field("station", &self.station)
            .field("monitor", &self.monitor)
            .finish()
    }
}

/// Runtime request does not match its independently checked owner topology.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiServiceRequestError {
    MissingStationTopology,
    UnexpectedTopologyRole,
    NotStandaloneMonitorTopology,
    MonitorPolicyMismatch,
}

impl fmt::Display for WifiServiceRequestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::MissingStationTopology => {
                "station service request requires a checked station topology"
            }
            Self::UnexpectedTopologyRole => {
                "station-only service request cannot contain another active Wi-Fi role"
            }
            Self::NotStandaloneMonitorTopology => {
                "standalone monitor request requires an exclusive checked monitor topology"
            }
            Self::MonitorPolicyMismatch => {
                "monitor runtime policy differs from the checked monitor topology"
            }
        })
    }
}

impl core::error::Error for WifiServiceRequestError {}

/// Request/topology mismatch retaining the exact role request.
#[derive(Debug)]
pub struct WifiServiceRequestFailure<R> {
    pub request: R,
    pub error: WifiServiceRequestError,
}

impl<R> WifiServiceRequestFailure<R> {
    const fn new(request: R, error: WifiServiceRequestError) -> Self {
        Self { request, error }
    }
}

/// Wi-Fi roles for which a supervisor composition owns complete static
/// resources.
///
/// This is not the currently active topology. In particular, provisioning
/// both station and standalone monitor allows sequential STA -> stopped ->
/// monitor transitions even on hardware which cannot run both concurrently.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiSupervisorConfiguration {
    capabilities: MacServiceCapabilities,
    station: Option<WifiStationConfig>,
    standalone_monitor: bool,
}

impl WifiSupervisorConfiguration {
    pub const fn new(capabilities: MacServiceCapabilities) -> Self {
        Self {
            capabilities,
            station: None,
            standalone_monitor: false,
        }
    }

    pub const fn with_station(mut self, station: WifiStationConfig) -> Self {
        self.station = Some(station);
        self
    }

    pub const fn with_standalone_monitor(mut self) -> Self {
        self.standalone_monitor = true;
        self
    }

    pub fn plan_station(
        self,
        request: StationRequest,
    ) -> Result<WifiServiceRequest, WifiServicePlanningFailure<StationRequest>> {
        let Some(station) = self.station else {
            return Err(WifiServicePlanningFailure {
                request,
                error: WifiServicePlanningError::StationNotProvisioned,
            });
        };
        let plan = match WifiConfig::station(station).validate(self.capabilities) {
            Ok(plan) => plan,
            Err(error) => {
                return Err(WifiServicePlanningFailure {
                    request,
                    error: WifiServicePlanningError::Topology(error),
                });
            }
        };
        WifiServiceRequest::station(plan, request).map_err(|failure| WifiServicePlanningFailure {
            request: failure.request,
            error: WifiServicePlanningError::Request(failure.error),
        })
    }

    pub fn plan_monitor(
        self,
        request: MonitorRequest,
    ) -> Result<WifiServiceRequest, WifiServicePlanningFailure<MonitorRequest>> {
        if !self.standalone_monitor {
            return Err(WifiServicePlanningFailure {
                request,
                error: WifiServicePlanningError::MonitorNotProvisioned,
            });
        }
        let plan = match WifiConfig::monitor(request.monitor_config()).validate(self.capabilities) {
            Ok(plan) => plan,
            Err(error) => {
                return Err(WifiServicePlanningFailure {
                    request,
                    error: WifiServicePlanningError::Topology(error),
                });
            }
        };
        WifiServiceRequest::standalone_monitor(plan, request).map_err(|failure| {
            WifiServicePlanningFailure {
                request: failure.request,
                error: WifiServicePlanningError::Request(failure.error),
            }
        })
    }
}

/// Why a provisioned supervisor cannot construct a checked service request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiServicePlanningError {
    StationNotProvisioned,
    MonitorNotProvisioned,
    Topology(WifiConfigError),
    Request(WifiServiceRequestError),
}

/// Planning failure retaining the untouched application request.
#[derive(Debug)]
pub struct WifiServicePlanningFailure<R> {
    pub request: R,
    pub error: WifiServicePlanningError,
}

#[cfg(test)]
mod tests {
    use super::*;
    use open_esp_radio_ieee80211::{channel::WifiChannelWidth, station::StaAssociationPreference};
    use open_esp_radio_wifi_softmac::{
        MacInterfaceCapabilities, MacOperationOwner, MacOperationOwnership, MacResourceLimits,
        MacServiceCapabilities, WifiConfig, WifiMacAddress, WifiStationConfig,
    };

    const TEST_CAPABILITIES: MacServiceCapabilities = MacServiceCapabilities {
        interfaces: MacInterfaceCapabilities {
            station_interfaces: 1,
            access_point_interfaces: 0,
            simultaneous_station_access_point: false,
            standalone_monitor: true,
            monitor_with_interfaces: false,
            raw_monitor_tap: false,
            normalized_monitor_tap: true,
            protocol_validated_monitor_tap: false,
        },
        operations: MacOperationOwnership {
            tx_fcs_generation: MacOperationOwner::Hardware,
            immediate_ack_response: MacOperationOwner::Hardware,
            csma_ca_backoff_countdown: MacOperationOwner::Hardware,
            unicast_retry_policy: MacOperationOwner::Software,
            tx_sequence_assignment: MacOperationOwner::Software,
            ccmp_key_selection: MacOperationOwner::Software,
            ccmp_packet_number: MacOperationOwner::Software,
            ccmp_transform: MacOperationOwner::Hardware,
            rx_block_ack_matching: MacOperationOwner::Hardware,
            rx_reorder: MacOperationOwner::Software,
            tx_block_ack_capture: MacOperationOwner::Hardware,
            tx_ampdu_retry_selection: MacOperationOwner::Software,
        },
        resources: MacResourceLimits {
            channel_contexts: 1,
            ordinary_tx_queues: 4,
            rx_block_ack_entries: 8,
            rx_block_ack_max_tid: 7,
            rx_block_ack_max_window: 64,
            tx_block_ack_max_window: 32,
            tx_ampdu_max_subframes: 32,
            station_pairwise_ccmp_slots: 1,
            station_group_ccmp_slots: 1,
        },
    };

    fn station_request() -> StationRequest {
        StationRequest::new(
            WifiSsid::new(b"test-network").unwrap(),
            StationSecurity::wpa2_personal(Pmk::derive(b"password", b"test-network").unwrap()),
            StaReconnectPolicy::new(3, 100, 1_000, 100).unwrap(),
            StationScanPolicy::new(
                StationScanChannels::CHANNELS_1_TO_13,
                NonZeroU16::new(20).unwrap(),
                StaAssociationPreference::Automatic,
            ),
        )
    }

    #[test]
    fn station_debug_never_formats_key_material() {
        let request = station_request();
        let debug = std::format!("{request:?}");
        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains("password"));
    }

    #[test]
    fn monitor_request_separates_capture_from_dma_capacity() {
        let request = MonitorRequest::new(
            WifiChannel::new_2_4_ghz(6, WifiChannelWidth::Mhz20).unwrap(),
            WifiMonitorConfig::normalized(),
        )
        .with_capture_policy(MonitorCapturePolicy::truncate_at(
            NonZeroU16::new(512).unwrap(),
        ));
        assert_eq!(request.channel().primary(), 6);
        assert_eq!(request.capture_policy().snapshot_length(), Some(512));
    }

    #[test]
    fn service_request_checks_runtime_policy_against_topology() {
        let address = WifiMacAddress::new([0x02, 0, 0, 0, 0, 1]).unwrap();
        let station_plan = WifiConfig::station(WifiStationConfig::new(address))
            .validate(TEST_CAPABILITIES)
            .unwrap();
        let service = WifiServiceRequest::station(station_plan, station_request()).unwrap();
        assert!(service.station_request().is_some());
        assert!(service.monitor_request().is_none());

        let channel = WifiChannel::mhz20(6).unwrap();
        let monitor = MonitorRequest::new(channel, WifiMonitorConfig::normalized());
        let failure = WifiServiceRequest::standalone_monitor(station_plan, monitor).unwrap_err();
        assert_eq!(
            failure.error,
            WifiServiceRequestError::NotStandaloneMonitorTopology
        );
        assert_eq!(failure.request.channel().primary(), 6);
    }

    #[test]
    fn supervisor_provisions_sequential_station_and_monitor_topologies() {
        let address = WifiMacAddress::new([0x02, 0, 0, 0, 0, 1]).unwrap();
        let configuration = WifiSupervisorConfiguration::new(TEST_CAPABILITIES)
            .with_station(WifiStationConfig::new(address))
            .with_standalone_monitor();

        let station = configuration.plan_station(station_request()).unwrap();
        assert!(station.plan().station().is_some());
        assert!(station.plan().monitor().is_none());

        let monitor_request = MonitorRequest::new(
            WifiChannel::mhz20(11).unwrap(),
            WifiMonitorConfig::normalized(),
        );
        let monitor = configuration.plan_monitor(monitor_request).unwrap();
        assert!(monitor.plan().station().is_none());
        assert!(monitor.plan().standalone_monitor().is_some());
    }

    #[test]
    fn unprovisioned_role_rejection_returns_the_exact_request() {
        let configuration = WifiSupervisorConfiguration::new(TEST_CAPABILITIES);
        let request = MonitorRequest::new(
            WifiChannel::mhz20(9).unwrap(),
            WifiMonitorConfig::normalized(),
        );
        let failure = configuration.plan_monitor(request).unwrap_err();
        assert_eq!(
            failure.error,
            WifiServicePlanningError::MonitorNotProvisioned
        );
        assert_eq!(failure.request.channel().primary(), 9);
    }
}
