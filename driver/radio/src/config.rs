//! Application-facing radio subsystem selection.
//!
//! `RadioConfig` is a value-only request. It does not acquire peripherals or
//! allocate protocol storage. A chip/runtime composition first validates the
//! request into a `RadioPlan`, then consumes its unique hardware and storage
//! owners while materializing that plan.

use core::fmt;

use open_esp_radio_wifi_softmac::MacServiceCapabilities;
pub use open_esp_radio_wifi_softmac::{
    WifiAccessPointConfig, WifiConfig, WifiConfigError, WifiMacAddress, WifiMacAddressError,
    WifiMonitorConfig, WifiPlan, WifiStandaloneMonitorPlan, WifiStationConfig,
};

const WIFI_BIT: u8 = 1 << 0;
const BLUETOOTH_BIT: u8 = 1 << 1;
const IEEE802154_BIT: u8 = 1 << 2;

/// Independently selectable protocol subsystem sharing a physical radio.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RadioSubsystem {
    Wifi,
    /// Bluetooth subsystem family. Concrete capabilities later distinguish
    /// Low Energy from BR/EDR controller support; they are not separate
    /// physical-radio roots at this layer.
    Bluetooth,
    Ieee802154,
}

/// Small no-allocation set of radio subsystems.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(transparent)]
pub struct RadioSubsystems(u8);

impl RadioSubsystems {
    pub const NONE: Self = Self(0);
    pub const WIFI: Self = Self(WIFI_BIT);
    pub const BLUETOOTH: Self = Self(BLUETOOTH_BIT);
    pub const IEEE802154: Self = Self(IEEE802154_BIT);

    pub const fn contains(self, subsystem: RadioSubsystem) -> bool {
        self.0 & bit(subsystem) != 0
    }

    pub const fn with(self, subsystem: RadioSubsystem) -> Self {
        Self(self.0 | bit(subsystem))
    }

    pub const fn count(self) -> u8 {
        self.0.count_ones() as u8
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

const fn bit(subsystem: RadioSubsystem) -> u8 {
    match subsystem {
        RadioSubsystem::Wifi => WIFI_BIT,
        RadioSubsystem::Bluetooth => BLUETOOTH_BIT,
        RadioSubsystem::Ieee802154 => IEEE802154_BIT,
    }
}

/// Complete source-owned capability profile used to validate a radio request.
///
/// Coexistence describes implemented scheduler/ownership support, not merely
/// the protocols listed by a chip datasheet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RadioCapabilities {
    pub supported_subsystems: RadioSubsystems,
    pub coexistence: RadioCoexistenceCapabilities,
    pub wifi: Option<MacServiceCapabilities>,
}

impl RadioCapabilities {
    pub const fn wifi_only(wifi: MacServiceCapabilities) -> Self {
        Self {
            supported_subsystems: RadioSubsystems::WIFI,
            coexistence: RadioCoexistenceCapabilities::EXCLUSIVE,
            wifi: Some(wifi),
        }
    }
}

/// Concurrent protocol combinations implemented by a radio scheduler.
///
/// Pair capabilities remain explicit because a numeric subsystem count cannot
/// distinguish Wi-Fi+Bluetooth from Wi-Fi+IEEE 802.15.4. The maximum also
/// states whether an otherwise pairwise-compatible three-protocol topology is
/// implemented as one coherent owner graph.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RadioCoexistenceCapabilities {
    pub maximum_concurrent_subsystems: u8,
    pub wifi_bluetooth: bool,
    pub wifi_ieee802154: bool,
    pub bluetooth_ieee802154: bool,
}

impl RadioCoexistenceCapabilities {
    pub const EXCLUSIVE: Self = Self {
        maximum_concurrent_subsystems: 1,
        wifi_bluetooth: false,
        wifi_ieee802154: false,
        bluetooth_ieee802154: false,
    };

    const fn supports_pair(self, first: RadioSubsystem, second: RadioSubsystem) -> bool {
        match (first, second) {
            (RadioSubsystem::Wifi, RadioSubsystem::Bluetooth)
            | (RadioSubsystem::Bluetooth, RadioSubsystem::Wifi) => self.wifi_bluetooth,
            (RadioSubsystem::Wifi, RadioSubsystem::Ieee802154)
            | (RadioSubsystem::Ieee802154, RadioSubsystem::Wifi) => self.wifi_ieee802154,
            (RadioSubsystem::Bluetooth, RadioSubsystem::Ieee802154)
            | (RadioSubsystem::Ieee802154, RadioSubsystem::Bluetooth) => self.bluetooth_ieee802154,
            _ => true,
        }
    }
}

/// Requested protocol subsystems and their topology configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RadioConfig {
    wifi: Option<WifiConfig>,
    bluetooth: bool,
    ieee802154: bool,
}

impl RadioConfig {
    pub const fn wifi(wifi: WifiConfig) -> Self {
        Self {
            wifi: Some(wifi),
            bluetooth: false,
            ieee802154: false,
        }
    }

    /// Request a Bluetooth subsystem.
    ///
    /// Protocol-specific configuration will be added with the first real
    /// Bluetooth owner graph. Until then a backend must reject this request
    /// unless it explicitly advertises source-owned Bluetooth support.
    pub const fn bluetooth() -> Self {
        Self {
            wifi: None,
            bluetooth: true,
            ieee802154: false,
        }
    }

    /// Request an IEEE 802.15.4 subsystem.
    pub const fn ieee802154() -> Self {
        Self {
            wifi: None,
            bluetooth: false,
            ieee802154: true,
        }
    }

    pub const fn with_wifi(mut self, wifi: WifiConfig) -> Self {
        self.wifi = Some(wifi);
        self
    }

    pub const fn with_bluetooth(mut self) -> Self {
        self.bluetooth = true;
        self
    }

    pub const fn with_ieee802154(mut self) -> Self {
        self.ieee802154 = true;
        self
    }

    pub const fn selected_subsystems(self) -> RadioSubsystems {
        let mut selected = RadioSubsystems::NONE;
        if self.wifi.is_some() {
            selected = selected.with(RadioSubsystem::Wifi);
        }
        if self.bluetooth {
            selected = selected.with(RadioSubsystem::Bluetooth);
        }
        if self.ieee802154 {
            selected = selected.with(RadioSubsystem::Ieee802154);
        }
        selected
    }

    /// Validate protocol availability, coexistence and Wi-Fi owner topology.
    pub fn validate(self, capabilities: RadioCapabilities) -> Result<RadioPlan, RadioConfigError> {
        let selected = self.selected_subsystems();
        if selected.is_empty() {
            return Err(RadioConfigError::NoSubsystems);
        }
        for subsystem in [
            RadioSubsystem::Wifi,
            RadioSubsystem::Bluetooth,
            RadioSubsystem::Ieee802154,
        ] {
            if selected.contains(subsystem)
                && !capabilities.supported_subsystems.contains(subsystem)
            {
                return Err(RadioConfigError::UnsupportedSubsystem(subsystem));
            }
        }
        if selected.count() > capabilities.coexistence.maximum_concurrent_subsystems {
            return Err(RadioConfigError::TooManyConcurrentSubsystems {
                requested: selected.count(),
                supported: capabilities.coexistence.maximum_concurrent_subsystems,
            });
        }
        for (first, second) in [
            (RadioSubsystem::Wifi, RadioSubsystem::Bluetooth),
            (RadioSubsystem::Wifi, RadioSubsystem::Ieee802154),
            (RadioSubsystem::Bluetooth, RadioSubsystem::Ieee802154),
        ] {
            if selected.contains(first)
                && selected.contains(second)
                && !capabilities.coexistence.supports_pair(first, second)
            {
                return Err(RadioConfigError::UnsupportedCoexistence { first, second });
            }
        }

        let wifi = match self.wifi {
            Some(config) => {
                let capabilities = capabilities
                    .wifi
                    .ok_or(RadioConfigError::MissingWifiCapabilities)?;
                Some(
                    config
                        .validate(capabilities)
                        .map_err(RadioConfigError::Wifi)?,
                )
            }
            None => None,
        };
        Ok(RadioPlan { selected, wifi })
    }
}

/// Capability-checked radio topology ready for a concrete composition root.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RadioPlan {
    selected: RadioSubsystems,
    wifi: Option<WifiPlan>,
}

impl RadioPlan {
    pub const fn selected_subsystems(self) -> RadioSubsystems {
        self.selected
    }

    pub const fn wifi(self) -> Option<WifiPlan> {
        self.wifi
    }

    /// Extract exclusive Wi-Fi monitor ownership from this checked topology.
    pub const fn standalone_wifi_monitor(self) -> Option<WifiStandaloneMonitorPlan> {
        match self.wifi {
            Some(wifi) => wifi.standalone_monitor(),
            None => None,
        }
    }
}

/// Why the requested radio topology cannot be materialized by a backend.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RadioConfigError {
    NoSubsystems,
    UnsupportedSubsystem(RadioSubsystem),
    TooManyConcurrentSubsystems {
        requested: u8,
        supported: u8,
    },
    UnsupportedCoexistence {
        first: RadioSubsystem,
        second: RadioSubsystem,
    },
    MissingWifiCapabilities,
    Wifi(WifiConfigError),
}

impl fmt::Display for RadioConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSubsystems => formatter.write_str("no radio subsystem was selected"),
            Self::UnsupportedSubsystem(subsystem) => {
                write!(formatter, "the radio does not implement {subsystem:?}")
            }
            Self::TooManyConcurrentSubsystems {
                requested,
                supported,
            } => write!(
                formatter,
                "requested {requested} concurrent radio subsystems, but the radio supports {supported}",
            ),
            Self::UnsupportedCoexistence { first, second } => write!(
                formatter,
                "the radio does not implement coexistence between {first:?} and {second:?}",
            ),
            Self::MissingWifiCapabilities => {
                formatter.write_str("the radio did not provide its Wi-Fi capability profile")
            }
            Self::Wifi(error) => write!(formatter, "invalid Wi-Fi configuration: {error}"),
        }
    }
}

impl core::error::Error for RadioConfigError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Wifi(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use open_esp_radio_wifi_softmac::{
        MacInterfaceCapabilities, MacOperationOwner, MacOperationOwnership, MacResourceLimits,
        WifiMacAddress, WifiStationConfig,
    };

    const WIFI_CAPABILITIES: MacServiceCapabilities = MacServiceCapabilities {
        interfaces: MacInterfaceCapabilities {
            station_interfaces: 1,
            access_point_interfaces: 0,
            simultaneous_station_access_point: false,
            standalone_monitor: false,
            monitor_with_interfaces: false,
            raw_monitor_tap: false,
            normalized_monitor_tap: false,
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

    fn station_config() -> WifiConfig {
        let address = WifiMacAddress::new([0x02, 0, 0, 0, 0, 1]).unwrap();
        WifiConfig::station(WifiStationConfig::new(address))
    }

    #[test]
    fn wifi_only_plan_exposes_one_station_owner() {
        let plan = RadioConfig::wifi(station_config())
            .validate(RadioCapabilities::wifi_only(WIFI_CAPABILITIES))
            .unwrap();
        assert_eq!(plan.selected_subsystems(), RadioSubsystems::WIFI);
        assert!(plan.wifi().unwrap().station().is_some());
    }

    #[test]
    fn unsupported_protocol_is_rejected_before_hardware_ownership_moves() {
        assert_eq!(
            RadioConfig::bluetooth().validate(RadioCapabilities::wifi_only(WIFI_CAPABILITIES)),
            Err(RadioConfigError::UnsupportedSubsystem(
                RadioSubsystem::Bluetooth
            ))
        );
    }

    #[test]
    fn coex_must_be_an_implemented_capability() {
        let config = RadioConfig::wifi(station_config()).with_bluetooth();
        let capabilities = RadioCapabilities {
            supported_subsystems: RadioSubsystems::WIFI.with(RadioSubsystem::Bluetooth),
            coexistence: RadioCoexistenceCapabilities::EXCLUSIVE,
            wifi: Some(WIFI_CAPABILITIES),
        };
        assert_eq!(
            config.validate(capabilities),
            Err(RadioConfigError::TooManyConcurrentSubsystems {
                requested: 2,
                supported: 1,
            })
        );
    }

    #[test]
    fn pairwise_coexistence_is_not_inferred_from_the_numeric_limit() {
        let config = RadioConfig::wifi(station_config()).with_bluetooth();
        let capabilities = RadioCapabilities {
            supported_subsystems: RadioSubsystems::WIFI.with(RadioSubsystem::Bluetooth),
            coexistence: RadioCoexistenceCapabilities {
                maximum_concurrent_subsystems: 2,
                wifi_bluetooth: false,
                wifi_ieee802154: false,
                bluetooth_ieee802154: false,
            },
            wifi: Some(WIFI_CAPABILITIES),
        };
        assert_eq!(
            config.validate(capabilities),
            Err(RadioConfigError::UnsupportedCoexistence {
                first: RadioSubsystem::Wifi,
                second: RadioSubsystem::Bluetooth,
            })
        );
    }

    #[cfg(feature = "esp32s31-wifi")]
    #[test]
    fn current_esp32s31_profile_accepts_only_its_real_owner_graph() {
        let station = RadioConfig::wifi(station_config())
            .validate(crate::esp32s31::RADIO_CAPABILITIES)
            .unwrap();
        assert!(station.wifi().unwrap().station().is_some());

        let access_point = WifiConfig::access_point(WifiAccessPointConfig::new(
            WifiMacAddress::new([0x02, 0, 0, 0, 0, 2]).unwrap(),
        ));
        assert_eq!(
            RadioConfig::wifi(access_point).validate(crate::esp32s31::RADIO_CAPABILITIES),
            Err(RadioConfigError::Wifi(
                WifiConfigError::UnsupportedAccessPoint
            ))
        );

        let monitor = RadioConfig::wifi(WifiConfig::monitor(WifiMonitorConfig::normalized()))
            .validate(crate::esp32s31::RADIO_CAPABILITIES)
            .unwrap();
        assert_eq!(
            monitor.wifi().unwrap().monitor(),
            Some(WifiMonitorConfig::normalized())
        );
        let standalone = monitor.standalone_wifi_monitor().unwrap();
        assert_eq!(standalone.monitor(), WifiMonitorConfig::normalized());
        assert_eq!(
            standalone.channel_context(),
            open_esp_radio_wifi_softmac::interface::ChannelContextId::PRIMARY
        );
        assert!(station.standalone_wifi_monitor().is_none());
    }
}
