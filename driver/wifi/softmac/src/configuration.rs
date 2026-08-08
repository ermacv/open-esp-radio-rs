//! Application-facing Wi-Fi service topology.
//!
//! This module describes which protocol owners an application wants to
//! create. It deliberately excludes credentials, scan policy, DMA storage and
//! executor resources: those belong to a station/AP service request or to a
//! runtime adapter. Validation assigns backend-facing VIF identities only
//! after the complete MAC service capability profile is known.

use core::fmt;

use crate::{
    MacServiceCapabilities,
    interface::{
        BoundVirtualInterface, ChannelContextId, MonitorTapPoint, VifId, VifRole, VirtualInterface,
    },
};

/// A valid unicast MAC address used to create one Wi-Fi protocol interface.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct WifiMacAddress([u8; 6]);

impl WifiMacAddress {
    pub const fn new(bytes: [u8; 6]) -> Result<Self, WifiMacAddressError> {
        if bytes[0] == 0
            && bytes[1] == 0
            && bytes[2] == 0
            && bytes[3] == 0
            && bytes[4] == 0
            && bytes[5] == 0
        {
            return Err(WifiMacAddressError::Unspecified);
        }
        if bytes[0] & 1 != 0 {
            return Err(WifiMacAddressError::Multicast);
        }
        Ok(Self(bytes))
    }

    pub const fn bytes(self) -> [u8; 6] {
        self.0
    }
}

/// Why an address cannot identify a Wi-Fi protocol interface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiMacAddressError {
    Unspecified,
    Multicast,
}

impl fmt::Display for WifiMacAddressError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Unspecified => "a Wi-Fi interface address cannot be all zero",
            Self::Multicast => "a Wi-Fi interface address must be unicast",
        })
    }
}

impl core::error::Error for WifiMacAddressError {}

/// Configuration needed when creating a station protocol owner.
///
/// SSID and credentials intentionally do not live here. A station interface
/// may scan, reconnect or select another network without being recreated.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiStationConfig {
    address: WifiMacAddress,
}

impl WifiStationConfig {
    pub const fn new(address: WifiMacAddress) -> Self {
        Self { address }
    }

    pub const fn address(self) -> WifiMacAddress {
        self.address
    }
}

/// Configuration needed when creating an access-point protocol owner.
///
/// Beacon contents, SSID, authentication and channel policy belong to the AP
/// service started on this interface, rather than to physical radio creation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiAccessPointConfig {
    address: WifiMacAddress,
}

impl WifiAccessPointConfig {
    pub const fn new(address: WifiMacAddress) -> Self {
        Self { address }
    }

    pub const fn address(self) -> WifiMacAddress {
        self.address
    }
}

/// Best-effort passive observation requested from the MAC receive path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiMonitorConfig {
    tap: MonitorTapPoint,
}

impl WifiMonitorConfig {
    pub const fn raw() -> Self {
        Self {
            tap: MonitorTapPoint::Raw,
        }
    }

    pub const fn normalized() -> Self {
        Self {
            tap: MonitorTapPoint::Normalized,
        }
    }

    pub const fn protocol_validated() -> Self {
        Self {
            tap: MonitorTapPoint::ProtocolValidated,
        }
    }

    pub const fn tap(self) -> MonitorTapPoint {
        self.tap
    }
}

/// Requested Wi-Fi owner topology.
///
/// A monitor is independent of the protocol VIF count. It observes a bounded
/// point in the receive path and must never acquire the normal RX owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiConfig {
    station: Option<WifiStationConfig>,
    access_point: Option<WifiAccessPointConfig>,
    monitor: Option<WifiMonitorConfig>,
}

impl WifiConfig {
    pub const fn station(station: WifiStationConfig) -> Self {
        Self {
            station: Some(station),
            access_point: None,
            monitor: None,
        }
    }

    pub const fn access_point(access_point: WifiAccessPointConfig) -> Self {
        Self {
            station: None,
            access_point: Some(access_point),
            monitor: None,
        }
    }

    pub const fn station_access_point(
        station: WifiStationConfig,
        access_point: WifiAccessPointConfig,
    ) -> Self {
        Self {
            station: Some(station),
            access_point: Some(access_point),
            monitor: None,
        }
    }

    pub const fn monitor(monitor: WifiMonitorConfig) -> Self {
        Self {
            station: None,
            access_point: None,
            monitor: Some(monitor),
        }
    }

    pub const fn with_monitor(mut self, monitor: WifiMonitorConfig) -> Self {
        self.monitor = Some(monitor);
        self
    }

    pub const fn station_config(self) -> Option<WifiStationConfig> {
        self.station
    }

    pub const fn access_point_config(self) -> Option<WifiAccessPointConfig> {
        self.access_point
    }

    pub const fn monitor_config(self) -> Option<WifiMonitorConfig> {
        self.monitor
    }

    /// Validate the requested topology and assign deterministic backend IDs.
    pub fn validate(
        self,
        capabilities: MacServiceCapabilities,
    ) -> Result<WifiPlan, WifiConfigError> {
        if capabilities.resources.channel_contexts == 0 {
            return Err(WifiConfigError::NoChannelContext);
        }
        if self.station.is_some() && capabilities.interfaces.station_interfaces == 0 {
            return Err(WifiConfigError::UnsupportedStation);
        }
        if self.access_point.is_some() && capabilities.interfaces.access_point_interfaces == 0 {
            return Err(WifiConfigError::UnsupportedAccessPoint);
        }
        if self.station.is_some()
            && self.access_point.is_some()
            && !capabilities.interfaces.simultaneous_station_access_point
        {
            return Err(WifiConfigError::UnsupportedStationAccessPoint);
        }
        if let Some(monitor) = self.monitor
            && !capabilities.interfaces.supports_monitor_tap(monitor.tap)
        {
            return Err(WifiConfigError::UnsupportedMonitorTap(monitor.tap));
        }
        let has_interface = self.station.is_some() || self.access_point.is_some();
        if self.monitor.is_some()
            && has_interface
            && !capabilities.interfaces.monitor_with_interfaces
        {
            return Err(WifiConfigError::UnsupportedMonitorWithInterfaces);
        }
        if self.monitor.is_some() && !has_interface && !capabilities.interfaces.standalone_monitor {
            return Err(WifiConfigError::UnsupportedStandaloneMonitor);
        }
        if let (Some(station), Some(access_point)) = (self.station, self.access_point)
            && station.address == access_point.address
        {
            return Err(WifiConfigError::DuplicateInterfaceAddress);
        }

        let station = self.station.map(|config| {
            BoundVirtualInterface::new(
                VirtualInterface::new(VifId::PRIMARY, VifRole::Station, config.address.bytes()),
                ChannelContextId::PRIMARY,
            )
        });
        let access_point = self.access_point.map(|config| {
            let id = if station.is_some() {
                VifId::new(1)
            } else {
                VifId::PRIMARY
            };
            BoundVirtualInterface::new(
                VirtualInterface::new(id, VifRole::AccessPoint, config.address.bytes()),
                ChannelContextId::PRIMARY,
            )
        });
        Ok(WifiPlan {
            station,
            access_point,
            monitor: self.monitor,
            monitor_channel_context: self.monitor.map(|_| ChannelContextId::PRIMARY),
        })
    }
}

/// A capability-checked Wi-Fi topology ready for chip/runtime composition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiPlan {
    station: Option<BoundVirtualInterface>,
    access_point: Option<BoundVirtualInterface>,
    monitor: Option<WifiMonitorConfig>,
    monitor_channel_context: Option<ChannelContextId>,
}

/// Capability-checked standalone monitor topology.
///
/// This value proves that no STA/AP VIF shares the RX owner. A chip backend
/// still checks the requested tap against the concrete view it implements.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiStandaloneMonitorPlan {
    monitor: WifiMonitorConfig,
    channel_context: ChannelContextId,
}

impl WifiStandaloneMonitorPlan {
    pub const fn monitor(self) -> WifiMonitorConfig {
        self.monitor
    }

    pub const fn channel_context(self) -> ChannelContextId {
        self.channel_context
    }
}

impl WifiPlan {
    pub const fn station(self) -> Option<BoundVirtualInterface> {
        self.station
    }

    pub const fn access_point(self) -> Option<BoundVirtualInterface> {
        self.access_point
    }

    pub const fn monitor(self) -> Option<WifiMonitorConfig> {
        self.monitor
    }

    pub const fn monitor_channel_context(self) -> Option<ChannelContextId> {
        self.monitor_channel_context
    }

    /// Narrow this checked topology to exclusive monitor ownership.
    pub const fn standalone_monitor(self) -> Option<WifiStandaloneMonitorPlan> {
        if self.station.is_some() || self.access_point.is_some() {
            return None;
        }
        match (self.monitor, self.monitor_channel_context) {
            (Some(monitor), Some(channel_context)) => Some(WifiStandaloneMonitorPlan {
                monitor,
                channel_context,
            }),
            _ => None,
        }
    }
}

/// A requested owner graph that the complete MAC service cannot construct.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiConfigError {
    NoChannelContext,
    UnsupportedStation,
    UnsupportedAccessPoint,
    UnsupportedStationAccessPoint,
    UnsupportedMonitorTap(MonitorTapPoint),
    UnsupportedStandaloneMonitor,
    UnsupportedMonitorWithInterfaces,
    DuplicateInterfaceAddress,
}

impl fmt::Display for WifiConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoChannelContext => {
                formatter.write_str("the Wi-Fi service exposes no channel context")
            }
            Self::UnsupportedStation => {
                formatter.write_str("the Wi-Fi service does not implement a station interface")
            }
            Self::UnsupportedAccessPoint => formatter
                .write_str("the Wi-Fi service does not implement an access-point interface"),
            Self::UnsupportedStationAccessPoint => formatter.write_str(
                "the Wi-Fi service cannot run station and access-point interfaces together",
            ),
            Self::UnsupportedMonitorTap(tap) => {
                write!(
                    formatter,
                    "the Wi-Fi service does not implement the {tap:?} monitor tap"
                )
            }
            Self::UnsupportedStandaloneMonitor => formatter
                .write_str("the Wi-Fi service cannot run a monitor without a protocol interface"),
            Self::UnsupportedMonitorWithInterfaces => formatter.write_str(
                "the Wi-Fi service cannot keep a monitor active with a protocol interface",
            ),
            Self::DuplicateInterfaceAddress => formatter
                .write_str("station and access-point interfaces must have distinct addresses"),
        }
    }
}

impl core::error::Error for WifiConfigError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        MacInterfaceCapabilities, MacOperationOwner, MacOperationOwnership, MacResourceLimits,
    };

    const fn capabilities(interfaces: MacInterfaceCapabilities) -> MacServiceCapabilities {
        MacServiceCapabilities {
            interfaces,
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
        }
    }

    const ALL_INTERFACES: MacInterfaceCapabilities = MacInterfaceCapabilities {
        station_interfaces: 1,
        access_point_interfaces: 1,
        simultaneous_station_access_point: true,
        standalone_monitor: true,
        monitor_with_interfaces: true,
        raw_monitor_tap: false,
        normalized_monitor_tap: true,
        protocol_validated_monitor_tap: false,
    };

    fn address(last: u8) -> WifiMacAddress {
        WifiMacAddress::new([0x02, 0, 0, 0, 0, last]).unwrap()
    }

    #[test]
    fn rejects_unspecified_and_multicast_addresses() {
        assert_eq!(
            WifiMacAddress::new([0; 6]),
            Err(WifiMacAddressError::Unspecified)
        );
        assert_eq!(
            WifiMacAddress::new([1, 0, 0, 0, 0, 1]),
            Err(WifiMacAddressError::Multicast)
        );
    }

    #[test]
    fn assigns_distinct_vifs_to_station_and_ap_on_one_channel_context() {
        let plan = WifiConfig::station_access_point(
            WifiStationConfig::new(address(1)),
            WifiAccessPointConfig::new(address(2)),
        )
        .with_monitor(WifiMonitorConfig::normalized())
        .validate(capabilities(ALL_INTERFACES))
        .unwrap();

        let station = plan.station().unwrap();
        let access_point = plan.access_point().unwrap();
        assert_eq!(station.interface.id, VifId::PRIMARY);
        assert_eq!(access_point.interface.id, VifId::new(1));
        assert_eq!(station.channel_context, access_point.channel_context);
        assert_eq!(
            plan.monitor_channel_context(),
            Some(ChannelContextId::PRIMARY)
        );
        assert!(plan.standalone_monitor().is_none());
    }

    #[test]
    fn standalone_monitor_plan_proves_exclusive_rx_ownership() {
        let plan = WifiConfig::monitor(WifiMonitorConfig::normalized())
            .validate(capabilities(ALL_INTERFACES))
            .unwrap()
            .standalone_monitor()
            .unwrap();
        assert_eq!(plan.monitor(), WifiMonitorConfig::normalized());
        assert_eq!(plan.channel_context(), ChannelContextId::PRIMARY);
    }

    #[test]
    fn rejects_roles_and_taps_not_owned_by_the_complete_service() {
        let station_only = MacInterfaceCapabilities {
            station_interfaces: 1,
            access_point_interfaces: 0,
            simultaneous_station_access_point: false,
            standalone_monitor: false,
            monitor_with_interfaces: false,
            raw_monitor_tap: false,
            normalized_monitor_tap: false,
            protocol_validated_monitor_tap: false,
        };
        assert_eq!(
            WifiConfig::access_point(WifiAccessPointConfig::new(address(2)))
                .validate(capabilities(station_only)),
            Err(WifiConfigError::UnsupportedAccessPoint)
        );
        assert_eq!(
            WifiConfig::station(WifiStationConfig::new(address(1)))
                .with_monitor(WifiMonitorConfig::normalized())
                .validate(capabilities(station_only)),
            Err(WifiConfigError::UnsupportedMonitorTap(
                MonitorTapPoint::Normalized
            ))
        );
    }

    #[test]
    fn rejects_duplicate_station_and_ap_addresses() {
        let shared = address(1);
        assert_eq!(
            WifiConfig::station_access_point(
                WifiStationConfig::new(shared),
                WifiAccessPointConfig::new(shared),
            )
            .validate(capabilities(ALL_INTERFACES)),
            Err(WifiConfigError::DuplicateInterfaceAddress)
        );
    }

    #[test]
    fn distinguishes_standalone_monitor_from_concurrent_interface_tap() {
        let standalone_only = MacInterfaceCapabilities {
            station_interfaces: 1,
            access_point_interfaces: 0,
            simultaneous_station_access_point: false,
            standalone_monitor: true,
            monitor_with_interfaces: false,
            raw_monitor_tap: false,
            normalized_monitor_tap: true,
            protocol_validated_monitor_tap: false,
        };
        assert!(
            WifiConfig::monitor(WifiMonitorConfig::normalized())
                .validate(capabilities(standalone_only))
                .is_ok()
        );
        assert_eq!(
            WifiConfig::station(WifiStationConfig::new(address(1)))
                .with_monitor(WifiMonitorConfig::normalized())
                .validate(capabilities(standalone_only)),
            Err(WifiConfigError::UnsupportedMonitorWithInterfaces)
        );
    }
}
