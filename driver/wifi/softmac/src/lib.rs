#![no_std]
#![forbid(unsafe_code)]

//! Portable SoftMAC contract between IEEE 802.11 protocol code and a chip MAC
//! backend.
//!
//! A single `hardware_ampdu` or `hardware_retry` flag is too coarse for a
//! split-MAC device.  Hardware may capture a BlockAck bitmap while software
//! owns MPDU retention, or execute one timed transmit attempt while software
//! owns the retry ladder.  This module therefore describes each operation
//! independently.
//!
//! The contract describes observable ownership, not a Rust module layout.
//! [`MacOperationOwner::Software`] may be implemented by portable SoftMAC
//! policy, a chip-specific MAC state machine, or their explicit composition. It
//! means only that a caller must not assume the operation is an offload.

pub mod configuration;
pub mod egress;
pub mod esp_now;
pub mod esp_now_security;
pub mod interface;
pub mod monitor;

pub use open_esp_radio_ieee80211::channel::WifiChannel;

pub use configuration::{
    WifiAccessPointConfig, WifiConfig, WifiConfigError, WifiMacAddress, WifiMacAddressError,
    WifiMonitorConfig, WifiPlan, WifiStandaloneEspNowPlan, WifiStandaloneMonitorPlan,
    WifiStationConfig,
};
pub use egress::{
    WifiAirtimeUnits, WifiEgressAdmission, WifiEgressAdmissionObservation, WifiEgressAirtimeConfig,
    WifiEgressAirtimeError, WifiEgressAirtimeScheduler, WifiEgressDemand, WifiEgressDemandId,
    WifiEgressDemandLevel, WifiEgressOpportunity, WifiEgressSelection,
};
pub use esp_now::{
    ESP_NOW_DEFAULT_PEER_CAPACITY, ESP_NOW_RX_DUPLICATE_HISTORY_CAPACITY, EspNowConfig,
    EspNowConfigError, EspNowHt20Rate, EspNowHtGuardInterval, EspNowHtMcs, EspNowOfdmRate,
    EspNowOwnedReceivedV1, EspNowOwnedReceivedV2, EspNowPeerCapability, EspNowPeerChannelPolicy,
    EspNowPeerConfig, EspNowPeerId, EspNowPeerSecurity, EspNowPeerTable, EspNowPeerTableError,
    EspNowPeers, EspNowPhyMode, EspNowPreparedV1Tx, EspNowPreparedV2Tx, EspNowProtocol,
    EspNowReceiveError, EspNowReceivedV1, EspNowReceivedV2, EspNowRxEpoch, EspNowRxOutcome,
    EspNowSendError, EspNowV2ReceiveError, EspNowV2RxOutcome, EspNowV2SendError,
};
pub use esp_now_security::{
    ESP_NOW_DEFAULT_ENCRYPTED_PEER_CAPACITY, ESP_NOW_KEY_LEN, ESP_NOW_RX_REPLAY_WINDOW_BITS,
    EspNowEncryptedPeerConfig, EspNowEncryptedPeerDiagnostics, EspNowEncryptedPeerError,
    EspNowEncryptedPeerId, EspNowEncryptedPeerMutationFailure, EspNowEncryptedPeerReplacement,
    EspNowEncryptedPeerRestoreFailure, EspNowEncryptedPeerTable, EspNowEncryptedPeerView,
    EspNowEncryptedProtocol, EspNowEncryptedReceiveError, EspNowEncryptedRxCandidate,
    EspNowEncryptedSendError, EspNowLmk, EspNowPmk, EspNowPmkError, EspNowPmkId,
    EspNowPmkMutationFailure, EspNowPmkOwner, EspNowPreparedEncryptedV1Tx,
    EspNowRemovedEncryptedPeer, EspNowRxReplayCandidate, encrypted_peer_destination,
    esp_now_encrypted_v1_codec_status,
};
pub use monitor::{
    MONITOR_CHANNEL_SEQUENCE_CAPACITY, MonitorChannelPolicy, MonitorChannelSequence,
    MonitorChannelSequenceError, MonitorDropReason, MonitorFilter, MonitorFrame, MonitorFrameType,
    MonitorFrameTypeMask, MonitorInjectionBindingError, MonitorInjectionChannelBinding,
    MonitorInjectionFrameError, MonitorInjectionFrameType, MonitorInjectionMpdu,
    MonitorInjectionRate, MonitorInjectionRequest, MonitorPublishOutcome, MonitorSink,
};

use open_esp_radio_ieee80211::wmm::WmmAccessCategory;

/// Owner that must perform one indivisible MAC operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacOperationOwner {
    /// The MAC service delegates the operation to radio hardware.
    Hardware,
    /// Source-owned software performs the operation.
    Software,
    /// The complete MAC service does not currently provide the operation.
    Unsupported,
}

/// Independently stated offload boundaries for a split-MAC implementation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacOperationOwnership {
    /// Append the transmit FCS to an encoded MPDU.
    pub tx_fcs_generation: MacOperationOwner,
    /// Send the immediate ACK response required by the receive exchange.
    pub immediate_ack_response: MacOperationOwner,
    /// Count down an already selected CSMA/CA backoff and arbitrate the medium.
    pub csma_ca_backoff_countdown: MacOperationOwner,
    /// Update contention state, choose retry rates and decide whether to retry.
    pub unicast_retry_policy: MacOperationOwner,
    /// Allocate per-interface/per-TID 802.11 sequence numbers.
    pub tx_sequence_assignment: MacOperationOwner,
    /// Select the logical key used by an outgoing protected MPDU.
    pub ccmp_key_selection: MacOperationOwner,
    /// Allocate and encode the outgoing CCMP packet number.
    pub ccmp_packet_number: MacOperationOwner,
    /// Perform the CCMP payload transform and append/verify its MIC.
    pub ccmp_transform: MacOperationOwner,
    /// Match incoming A-MPDU sequence spaces against an active BA agreement.
    pub rx_block_ack_matching: MacOperationOwner,
    /// Reorder received MPDUs and decide when gaps may be released.
    pub rx_reorder: MacOperationOwner,
    /// Capture the transmit BlockAck starting sequence and bitmap.
    pub tx_block_ack_capture: MacOperationOwner,
    /// Select and retain missing MPDUs for a subsequent A-MPDU attempt.
    pub tx_ampdu_retry_selection: MacOperationOwner,
}

/// End-to-end resource limits visible to portable HMAC policy.
///
/// These are service limits, after hardware and source-owned storage limits
/// have both been applied.  They are not a claim about the total resources
/// physically present in a chip.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacResourceLimits {
    /// Simultaneously active radio channel contexts.
    pub channel_contexts: u8,
    /// Ordinary WMM/EDCA transmit queues exposed by the service.
    pub ordinary_tx_queues: u8,
    /// Receive BlockAck agreement banks exposed by the service.
    pub rx_block_ack_entries: u8,
    /// Highest receive BlockAck TID accepted by the service.
    pub rx_block_ack_max_tid: u8,
    /// Maximum receive reorder window retained by the complete service.
    pub rx_block_ack_max_window: u16,
    /// Maximum transmit BlockAck window retained by the complete service.
    pub tx_block_ack_max_window: u16,
    /// Maximum MPDUs retained in one transmit aggregate.
    pub tx_ampdu_max_subframes: u16,
    /// Pairwise CCMP slots exposed for one station interface.
    pub station_pairwise_ccmp_slots: u8,
    /// Group CCMP slots exposed for one station interface.
    pub station_group_ccmp_slots: u8,
    /// Pairwise CCMP slots exposed for one access-point interface.
    pub access_point_pairwise_ccmp_slots: u8,
    /// Group CCMP slots exposed for one access-point interface.
    pub access_point_group_ccmp_slots: u8,
    /// Simultaneously associated peers owned by the AP protocol service.
    pub access_point_association_entries: u8,
    /// Associated peers which may own independent protected data ports.
    pub access_point_encrypted_clients: u8,
}

/// Virtual-interface roles implemented by the complete driver today.
///
/// This advertises source-owned service, not latent hardware ability. For
/// example, a chip with several address filters still reports zero AP VIFs
/// until the AP owner graph and lifecycle exist.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacInterfaceCapabilities {
    pub station_interfaces: u8,
    pub access_point_interfaces: u8,
    pub simultaneous_station_access_point: bool,
    /// A monitor tap can own the radio without a protocol VIF.
    pub standalone_monitor: bool,
    /// A monitor tap can remain active while a protocol VIF runs.
    pub monitor_with_interfaces: bool,
    pub raw_monitor_tap: bool,
    pub normalized_monitor_tap: bool,
    pub protocol_validated_monitor_tap: bool,
}

impl MacInterfaceCapabilities {
    pub const fn supports_role(self, role: interface::VifRole) -> bool {
        match role {
            interface::VifRole::Station => self.station_interfaces != 0,
            interface::VifRole::AccessPoint => self.access_point_interfaces != 0,
        }
    }

    pub const fn supports_monitor_tap(self, point: interface::MonitorTapPoint) -> bool {
        match point {
            interface::MonitorTapPoint::Raw => self.raw_monitor_tap,
            interface::MonitorTapPoint::Normalized => self.normalized_monitor_tap,
            interface::MonitorTapPoint::ProtocolValidated => self.protocol_validated_monitor_tap,
        }
    }
}

/// Complete portable description of one MAC service implementation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacServiceCapabilities {
    pub interfaces: MacInterfaceCapabilities,
    pub operations: MacOperationOwnership,
    pub resources: MacResourceLimits,
}

/// Portable policy for one logical ordinary-MPDU exchange.
///
/// The encoded frame and its ownership lease are deliberately not embedded in
/// this copyable value. A concrete MAC backend combines this policy with an
/// owned TX lease and translates the access category and typed PHY rate into
/// its private queue/register representation. Descriptor capacity, hardware
/// key indices, coexistence priorities and DMA metadata are therefore not
/// part of the HMAC contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacTxPlan<Rate> {
    /// Standard WMM/EDCA category selected by protocol policy.
    pub access_category: WmmAccessCategory,
    /// Initial typed PHY policy selected for this logical exchange.
    pub initial_rate: Rate,
    /// Maximum hardware publications, including the first publication.
    pub publication_limit: u8,
    /// Executor watchdog applied independently to each publication.
    ///
    /// This is not the on-air MPDU lifetime and not a blocking delay.
    pub publication_timeout_micros: u64,
}

/// Observable state of a MAC transmit queue at the SoftMAC/backend boundary.
///
/// `Backpressured` is a normal ownership condition: an earlier lease or
/// hardware transaction is still live and the caller must wait for progress.
/// `ResetRequired` is terminal for the current radio epoch and must never be
/// treated as ordinary queue pressure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacTxQueueState {
    Ready,
    Backpressured,
    ResetRequired,
}

/// Provenance of one normalized receive value.
///
/// Missing data is explicit because deriving a value from an active BA
/// agreement, a protected frame-control bit, or another adjacent condition
/// is not equivalent to observing it in hardware or validating it in the
/// protocol parser.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacRxEvidence<T> {
    /// The receive backend decoded this value from a documented hardware
    /// status field.
    HardwareObserved(T),
    /// Portable protocol processing established this value while validating
    /// the frame.
    ProtocolValidated(T),
    /// Neither layer has evidence for this value at the current boundary.
    Unavailable,
}

impl<T> MacRxEvidence<T> {
    pub const fn as_ref(&self) -> MacRxEvidence<&T> {
        match self {
            Self::HardwareObserved(value) => MacRxEvidence::HardwareObserved(value),
            Self::ProtocolValidated(value) => MacRxEvidence::ProtocolValidated(value),
            Self::Unavailable => MacRxEvidence::Unavailable,
        }
    }

    pub const fn is_available(&self) -> bool {
        !matches!(self, Self::Unavailable)
    }
}

/// Crypto result visible for an accepted receive MPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacRxCryptoStatus {
    Unprotected,
    /// The payload is plaintext and its integrity check has succeeded. Key
    /// identity and cipher negotiation remain HMAC/security state.
    DecryptedAndIntegrityVerified,
}

/// Portable metadata carried with one received MPDU.
///
/// `Rate` is a backend-selected typed PHY record, following the same rule as
/// [`MacTxStatus`]. A backend may publish the physical fields immediately and
/// leave semantic fields unavailable until protocol validation has run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacRxMetadata<Rate> {
    pub channel: MacRxEvidence<u8>,
    pub rate: MacRxEvidence<Rate>,
    pub rssi_dbm: MacRxEvidence<i8>,
    pub crypto: MacRxEvidence<MacRxCryptoStatus>,
    /// Whether the hardware marked this as an IEEE VHT/HE S-MPDU (an MPDU
    /// carried alone in an A-MPDU subframe with the delimiter EOF bit set).
    /// This is not a synonym for an ordinary non-aggregated MPDU.
    pub s_mpdu: MacRxEvidence<bool>,
    /// Whether this MPDU was carried in an A-MPDU container.
    ///
    /// This does not state how many MPDUs the container carried. In
    /// particular, an S-MPDU is the sole MPDU in a VHT/HE A-MPDU and
    /// therefore has both `s_mpdu=true` and `ampdu=true`. Provenance remains
    /// explicit because HT supplies an RXVECTOR Aggregation bit, while the
    /// VHT/HE PPDU format establishes A-MPDU containment by protocol rule.
    pub ampdu: MacRxEvidence<bool>,
    /// Whether this MPDU carries an A-MSDU payload.
    ///
    /// A-MPDU and A-MSDU are independent dimensions and deliberately retain
    /// independent evidence. Protocol parsing can prove the latter without
    /// proving the former.
    pub amsdu: MacRxEvidence<bool>,
}

impl<Rate> MacRxMetadata<Rate> {
    /// Metadata for a synthetic event or a boundary that has not observed any
    /// receive status. This is not a successful/zero-valued hardware sample.
    pub const fn unavailable() -> Self {
        Self {
            channel: MacRxEvidence::Unavailable,
            rate: MacRxEvidence::Unavailable,
            rssi_dbm: MacRxEvidence::Unavailable,
            crypto: MacRxEvidence::Unavailable,
            s_mpdu: MacRxEvidence::Unavailable,
            ampdu: MacRxEvidence::Unavailable,
            amsdu: MacRxEvidence::Unavailable,
        }
    }
}

/// Terminal result of one logical MPDU exchange.
///
/// A logical exchange may contain several hardware publications.  This is
/// intentionally different from a raw completion status for one attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacTxResult {
    /// Hardware completed the exchange successfully.
    Transmitted,
    /// Hardware returned a terminal chip-specific status after retries.
    HardwareFailure(u8),
    /// Every permitted publication ended at the hardware ACK/CTS timeout edge.
    HardwareTimeout,
    /// Every permitted publication lost contention before an on-air attempt.
    CollisionLimit,
}

/// Normalized terminal status returned from a backend to portable MAC policy.
///
/// `Rate` remains a backend-selected typed rate.  This keeps the portable
/// contract independent of one chip's register encoding without reducing the
/// final rate to an untyped integer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacTxStatus<Rate> {
    pub result: MacTxResult,
    /// Total hardware publications, including the initial attempt.
    pub attempts: u8,
    /// Rate used for the terminal hardware publication.
    pub final_rate: Rate,
    /// `Some(true/false)` only when the receiver class has ACK semantics.
    pub acknowledged: Option<bool>,
    /// Signed ACK SNR sample when the hardware reports a valid one.
    pub ack_snr_db: Option<i8>,
    /// Measured on-air duration when the backend can report it.
    pub airtime_micros: Option<u32>,
}

/// Terminal result of one logical A-MPDU exchange.
///
/// A successful aggregate may include several aggregate publications and one
/// final ordinary retry for a detached MPDU.  `Delivered` therefore describes
/// the complete HMAC-visible exchange, not just receipt of one BlockAck.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacAmpduTxResult {
    /// Every original MPDU was acknowledged, including an optional ordinary
    /// retry of one detached MPDU.
    Delivered,
    /// The retry policy ended while at least one original MPDU was unacknowledged.
    Incomplete,
    /// The aggregate publication reached its terminal hardware timeout edge.
    HardwareTimeout,
    /// The aggregate publication reached its terminal collision limit.
    CollisionLimit,
}

/// Normalized terminal status for one logical A-MPDU exchange.
///
/// The aggregate rate is kept separately from the optional ordinary retry's
/// terminal rate.  Collapsing those into one `final_rate` would lose which
/// part of the exchange used which PHY policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacAmpduTxStatus<Rate> {
    pub result: MacAmpduTxResult,
    pub original_subframes: u16,
    /// Number of A-MPDU hardware publications, including the first one.
    pub aggregate_attempts: u8,
    pub aggregate_rate: Rate,
    /// Original MPDUs acknowledged by one or more BlockAck responses.
    pub block_acknowledged_subframes: u16,
    /// Terminal status of a detached one-MPDU retry, when the backend used
    /// that fallback after the last aggregate publication.
    pub ordinary_retry: Option<MacTxStatus<Rate>>,
}

impl<Rate: Copy> MacAmpduTxStatus<Rate> {
    /// Original MPDUs proved delivered by BlockAck plus an optional successful
    /// ordinary retry.
    pub const fn delivered_subframes(&self) -> u16 {
        let ordinary_delivered = match self.ordinary_retry {
            Some(MacTxStatus {
                result: MacTxResult::Transmitted,
                ..
            }) => 1,
            _ => 0,
        };
        self.block_acknowledged_subframes
            .saturating_add(ordinary_delivered)
    }

    pub const fn fully_delivered(&self) -> bool {
        matches!(self.result, MacAmpduTxResult::Delivered)
            && self.delivered_subframes() == self.original_subframes
    }

    /// Aggregate and ordinary hardware publications made for the complete
    /// exchange.
    pub const fn total_publication_attempts(&self) -> u16 {
        let ordinary_attempts = match self.ordinary_retry {
            Some(status) => status.attempts as u16,
            None => 0,
        };
        self.aggregate_attempts as u16 + ordinary_attempts
    }
}

impl MacServiceCapabilities {
    /// Whether a requested receive BA window fits the complete service.
    pub const fn supports_rx_block_ack_window(self, window: u16) -> bool {
        window != 0 && window <= self.resources.rx_block_ack_max_window
    }

    /// Whether a requested transmit BA window fits the complete service.
    pub const fn supports_tx_block_ack_window(self, window: u16) -> bool {
        window != 0 && window <= self.resources.tx_block_ack_max_window
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAPABILITIES: MacServiceCapabilities = MacServiceCapabilities {
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
            access_point_pairwise_ccmp_slots: 1,
            access_point_group_ccmp_slots: 1,
            access_point_association_entries: 1,
            access_point_encrypted_clients: 1,
        },
    };

    #[test]
    fn zero_and_oversized_block_ack_windows_are_not_supported() {
        assert!(!CAPABILITIES.supports_rx_block_ack_window(0));
        assert!(CAPABILITIES.supports_rx_block_ack_window(64));
        assert!(!CAPABILITIES.supports_rx_block_ack_window(65));
        assert!(CAPABILITIES.supports_tx_block_ack_window(32));
        assert!(!CAPABILITIES.supports_tx_block_ack_window(33));
    }

    #[test]
    fn implemented_roles_and_monitor_taps_are_explicit() {
        assert!(
            CAPABILITIES
                .interfaces
                .supports_role(interface::VifRole::Station)
        );
        assert!(
            !CAPABILITIES
                .interfaces
                .supports_role(interface::VifRole::AccessPoint)
        );
        assert!(
            !CAPABILITIES
                .interfaces
                .supports_monitor_tap(interface::MonitorTapPoint::Raw)
        );
    }

    #[test]
    fn terminal_status_distinguishes_an_exchange_from_one_attempt() {
        let status = MacTxStatus {
            result: MacTxResult::Transmitted,
            attempts: 3,
            final_rate: 7_u8,
            acknowledged: Some(true),
            ack_snr_db: Some(18),
            airtime_micros: None,
        };
        assert_eq!(status.attempts, 3);
        assert_eq!(status.result, MacTxResult::Transmitted);
    }

    #[test]
    fn tx_plan_contains_protocol_policy_but_no_hardware_queue_encoding() {
        let plan = MacTxPlan {
            access_category: WmmAccessCategory::Video,
            initial_rate: 7_u8,
            publication_limit: 4,
            publication_timeout_micros: 250_000,
        };
        assert_eq!(plan.access_category, WmmAccessCategory::Video);
        assert_eq!(plan.initial_rate, 7);
        assert_eq!(plan.publication_limit, 4);
    }

    #[test]
    fn receive_metadata_keeps_absence_and_provenance_distinct() {
        let staged = MacRxMetadata {
            channel: MacRxEvidence::HardwareObserved(6),
            rate: MacRxEvidence::HardwareObserved(11_u8),
            rssi_dbm: MacRxEvidence::HardwareObserved(-47),
            crypto: MacRxEvidence::Unavailable,
            s_mpdu: MacRxEvidence::Unavailable,
            ampdu: MacRxEvidence::Unavailable,
            amsdu: MacRxEvidence::Unavailable,
        };
        assert!(staged.channel.is_available());
        assert!(!staged.crypto.is_available());

        let validated = MacRxMetadata {
            crypto: MacRxEvidence::ProtocolValidated(
                MacRxCryptoStatus::DecryptedAndIntegrityVerified,
            ),
            s_mpdu: MacRxEvidence::HardwareObserved(true),
            amsdu: MacRxEvidence::ProtocolValidated(false),
            ..staged
        };
        assert_ne!(validated.crypto, staged.crypto);
        assert_eq!(
            validated.s_mpdu.as_ref(),
            MacRxEvidence::HardwareObserved(&true)
        );
        assert_eq!(validated.ampdu, MacRxEvidence::Unavailable);
        assert_eq!(validated.amsdu, MacRxEvidence::ProtocolValidated(false));
    }

    #[test]
    fn ampdu_status_joins_block_ack_and_one_ordinary_retry() {
        let status = MacAmpduTxStatus {
            result: MacAmpduTxResult::Delivered,
            original_subframes: 3,
            aggregate_attempts: 2,
            aggregate_rate: 7_u8,
            block_acknowledged_subframes: 2,
            ordinary_retry: Some(MacTxStatus {
                result: MacTxResult::Transmitted,
                attempts: 2,
                final_rate: 5,
                acknowledged: Some(true),
                ack_snr_db: Some(12),
                airtime_micros: None,
            }),
        };
        assert_eq!(status.delivered_subframes(), 3);
        assert_eq!(status.total_publication_attempts(), 4);
        assert!(status.fully_delivered());
    }
}
