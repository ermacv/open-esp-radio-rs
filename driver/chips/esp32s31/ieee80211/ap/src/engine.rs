//! One bounded AP MAC epoch above DMA/IRQ and below the Embassy actor.

mod management;
mod power_save;
mod rx;
mod tx;

use crate::{
    beacon::Esp32s31ApBeacon,
    rx::{
        Esp32s31ApOrdinaryPairwiseRxRequest, Esp32s31ApRxAdmission, Esp32s31ApRxAdmissionOperation,
        Esp32s31ApRxAdmissionRequest, Esp32s31ApRxDuplicateOwner, Esp32s31ApRxError,
        Esp32s31ApRxPreparedCandidate, Esp32s31ApRxPreparedReplay,
    },
    security::{
        Esp32s31ApPairwiseBinding, Esp32s31ApPairwiseKeyStorage, Esp32s31ApSecurity,
        Esp32s31ApSecurityError, Esp32s31ApSecurityStopReport,
    },
};
use open_esp_radio_esp32s31_wifi_mac::{
    ap_policy::{ApRxPolicyHardware, configure_ap_receive_policy, disable_ap_receive_policy},
    ap_tsf::{ApTsfHardware, reset_and_start_access_point_tsf, stop_access_point_tsf},
    crypto::{CcmpKeyHardware, CryptoKeyError},
    tx_protection::{ErpProtectionMode, HtProtectionMode, WifiTxProtectionPolicy},
};
use open_esp_radio_ieee80211::block_ack::{
    OperationalTxBlockAck, TxBlockAckAlarm, TxBlockAckResponse,
};
use open_esp_radio_ieee80211::{
    ap::{
        ApActionFrame, ApAmsduFrame, ApAssociationResponseError, ApDataFrame, ApDataFrameError,
        ApManagementRequest, ApPeerDisconnectKind, ApPowerSaveObservation, ApProtectedDataFrame,
        ApUnprotectedDataFrame, EncodedApFrame, observe_ap_power_save_for_access_point,
        parse_ap_management_request, write_ap_peer_disconnect,
        write_ht_association_response_frame_for_security, write_open_authentication_response,
    },
    beacon::{ApBeaconBuildError, WPA2_BEACON_CAPACITY, dtim, write_ht_beacon},
    ccmp::{CcmpKeyId, CcmpReplayLane},
    channel::WifiChannel,
    data::{IEEE80211_LEGACY_DATA_HEADER_LEN, IEEE80211_QOS_DATA_HEADER_LEN},
    security::WifiSecurityMode,
    ssid::WifiSsid,
};
use open_esp_radio_wifi_ap::{
    AccessPointService, ApAssociationCapabilities, ApAssociationIdentity, ApBufferedGroupRelease,
    ApBufferedUnicastRelease, ApDownlinkAdmission, ApDownlinkDisposition, ApMlmeAction,
    ApPeerBinding, ApPeerClose, ApPeerCloseKind, ApPeerPhase, ApPeerPowerState, ApPeerStatus,
    ApPowerSaveAction, ApServiceError, ApWpa2Error, ApWpa2Progress, ApWpa2RetryProgress,
};
use open_esp_radio_wpa2::{OwnedEapolFrame, frames::Wpa2TxFrame};

pub trait Esp32s31ApRuntimeHardware: CcmpKeyHardware + ApRxPolicyHardware + ApTsfHardware {}

impl<T> Esp32s31ApRuntimeHardware for T where T: CcmpKeyHardware + ApRxPolicyHardware + ApTsfHardware
{}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31ApEngineError {
    Beacon(ApBeaconBuildError),
    Crypto(CryptoKeyError),
    Security(Esp32s31ApSecurityError),
    Service(ApServiceError),
    Frame(ApAssociationResponseError),
    DataFrame(ApDataFrameError),
    Wpa2(ApWpa2Error),
}

impl From<ApAssociationResponseError> for Esp32s31ApEngineError {
    fn from(error: ApAssociationResponseError) -> Self {
        Self::Frame(error)
    }
}

impl From<ApDataFrameError> for Esp32s31ApEngineError {
    fn from(error: ApDataFrameError) -> Self {
        Self::DataFrame(error)
    }
}

impl From<ApServiceError> for Esp32s31ApEngineError {
    fn from(error: ApServiceError) -> Self {
        Self::Service(error)
    }
}

impl From<Esp32s31ApSecurityError> for Esp32s31ApEngineError {
    fn from(error: Esp32s31ApSecurityError) -> Self {
        Self::Security(error)
    }
}

impl From<ApWpa2Error> for Esp32s31ApEngineError {
    fn from(error: ApWpa2Error) -> Self {
        Self::Wpa2(error)
    }
}

fn rejected_rx_security(error: Esp32s31ApSecurityError) -> Esp32s31ApRxAdmission {
    match error {
        Esp32s31ApSecurityError::Replay(error) => {
            Esp32s31ApRxAdmission::rejected(Esp32s31ApRxError::Replay(error))
        }
        Esp32s31ApSecurityError::SecurityModeMismatch => {
            Esp32s31ApRxAdmission::rejected(Esp32s31ApRxError::SecurityModeMismatch)
        }
        Esp32s31ApSecurityError::Crypto(_)
        | Esp32s31ApSecurityError::PacketNumber(_)
        | Esp32s31ApSecurityError::PairwiseStorageNotEmpty
        | Esp32s31ApSecurityError::PairwiseAlreadyInstalled
        | Esp32s31ApSecurityError::AssociationIdAlreadyInstalled
        | Esp32s31ApSecurityError::WrongPeer => {
            Esp32s31ApRxAdmission::rejected(Esp32s31ApRxError::KeyGenerationMismatch)
        }
    }
}

/// One executor-stamped beacon and the exact group queue prefix it advertised.
///
/// `dtim_group_frames` is nonzero only when this concrete frame carries
/// DTIM-count zero and TIM bitmap-control bit zero. It is still only a
/// publication candidate: the MAC must wait for terminal beacon TX success
/// before handing this count to the caller-owned group queue.
pub struct Esp32s31ApBeaconPublication<'frame> {
    pub frame: &'frame mut [u8],
    pub dtim_group_frames: u16,
}

pub struct Esp32s31ApEngineStartFailure<'storage> {
    pub service: AccessPointService<'storage>,
    pub beacon_storage: &'storage mut [u8; WPA2_BEACON_CAPACITY],
    pub pairwise_storage: &'storage mut Esp32s31ApPairwiseKeyStorage,
    pub error: Esp32s31ApEngineError,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31ApManagementOutcome {
    Ignored,
    Response { len: usize, begin_wpa2: bool },
    PeerRemoved { peer: [u8; 6] },
}

pub enum Esp32s31ApWpa2Outcome<const N: usize> {
    None,
    Transmit(Wpa2TxFrame<N>),
    PeerAuthorized { peer: [u8; 6] },
    DeauthenticatePeer { peer: [u8; 6] },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31ApProtectedFrame {
    pub length: usize,
    /// `None` is a plaintext Open MPDU and must use the zero-MIC ordinary TX
    /// path. A selector is present only for the WPA2 CCMP path.
    pub hardware_key_selector: Option<u8>,
}

/// Fully encoded A-MSDU whose output geometry and peer/key identity are
/// admitted, but whose monotonic QoS sequence and CCMP PN are not consumed.
///
/// The AP MAC holds this value only across its synchronous TX-protection
/// preflight. No public constructor can bypass the capacity/peer checks.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "prepared A-MSDU must pass TX admission before commit or be discarded"]
pub(crate) struct Esp32s31ApPreparedAmsduFrame {
    pub(crate) length: usize,
    pub(crate) peer: [u8; 6],
    sequence_number: u16,
    hardware_key_selector: Option<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Esp32s31ApDataSequenceSpace {
    NonQos,
    Qos,
}

/// Capacity-admitted ordinary AP data frame before sequence/CCMP commit.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "prepared AP data must pass TX admission before commit or be discarded"]
pub(crate) struct Esp32s31ApPreparedDataFrame {
    pub(crate) length: usize,
    pub(crate) peer: [u8; 6],
    sequence_number: u16,
    sequence_space: Esp32s31ApDataSequenceSpace,
    hardware_key_selector: Option<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31ApAggregateFrame {
    pub encoded: EncodedApFrame,
    pub hardware_key_selector: u8,
    pub sequence_number: u16,
}

/// Peer and key-slot identity captured once before an aggregate claims its
/// first DMA lease. Every MPDU validates these O(1) generation-bound owners.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31ApAggregateBinding {
    peer: ApPeerBinding,
    security: Esp32s31ApPairwiseBinding,
}

impl Esp32s31ApAggregateBinding {
    pub const fn peer(self) -> [u8; 6] {
        self.peer.address()
    }
}

/// Generation-bound AP RX context cached across one transmitter burst.
///
/// The portable peer binding validates slot generation and address in O(1),
/// while the pairwise binding independently validates the installed PTK
/// generation. The public status revision invalidates authorization/QoS
/// snapshots without putting a peer-table scan back on every data MPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Esp32s31ApRxPeerBinding {
    peer: ApPeerBinding,
    pairwise: Option<Esp32s31ApPairwiseBinding>,
    duplicate_owner: Esp32s31ApRxDuplicateOwner,
    qos_supported: bool,
    status_revision: u32,
}

pub struct Esp32s31ApEngineStop<'storage> {
    pub service: AccessPointService<'storage>,
    pub beacon_storage: &'storage mut [u8; WPA2_BEACON_CAPACITY],
    pub pairwise_storage: &'storage mut Esp32s31ApPairwiseKeyStorage,
    pub security: Esp32s31ApSecurityStopReport,
}

/// Value-only observations from one AP ownership epoch.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Esp32s31ApEngineObservation {
    pub beacons_prepared: u32,
    pub authentication_responses_prepared: u32,
    pub association_responses_prepared: u32,
    /// Successful controlled-port openings during the epoch.
    pub authorized_peers: u32,
    /// Largest number of simultaneously admitted peers during the epoch.
    pub maximum_associated_peers: u8,
    /// Largest number of simultaneously open controlled ports during the epoch.
    pub maximum_authorized_peers: u8,
    pub peer_removals: u32,
    pub authentication_timeouts: u32,
    pub wpa2_response_windows: u32,
    pub wpa2_pending_on_stop: u32,
    pub wpa2_retransmissions: u32,
    pub wpa2_handshake_failures: u32,
    pub wpa2_handshake_timeouts: u32,
    pub inactivity_timeouts: u32,
    pub disassociations_prepared: u32,
    pub deauthentications_prepared: u32,
    pub tx_block_ack_requests_prepared: u32,
    pub tx_block_ack_responses_observed: u32,
    pub tx_block_ack_agreements_operational: u32,
    pub tx_block_ack_responses_rejected: u32,
    pub tx_block_ack_negotiation_timeouts: u32,
}

#[cfg(any(feature = "diagnostics", test))]
#[derive(Clone, Copy)]
enum Esp32s31ApEngineObservationEvent {
    BeaconPrepared,
    AuthenticationResponsePrepared,
    AssociationResponsePrepared { associated_peers: u8 },
    PeerRemoved,
    TxBlockAckResponseObserved,
    TxBlockAckOperational,
    TxBlockAckRejected,
    TxBlockAckRequestPrepared,
    TxBlockAckNegotiationTimeout,
    PeerAuthorized { authorized_peers: u8 },
    Wpa2ResponseWindow,
    Wpa2Retransmission,
    Wpa2HandshakeTimeout,
    AuthenticationTimeout,
    Wpa2HandshakeFailure,
    InactivityTimeout,
    Wpa2PendingOnStop,
    DisassociationPrepared,
    DeauthenticationPrepared,
}

#[cfg(any(feature = "diagnostics", test))]
#[derive(Default)]
struct Esp32s31ApEngineObserver {
    observation: Esp32s31ApEngineObservation,
}

#[cfg(any(feature = "diagnostics", test))]
impl Esp32s31ApEngineObserver {
    fn observe(&mut self, event: Esp32s31ApEngineObservationEvent) {
        let observation = &mut self.observation;
        match event {
            Esp32s31ApEngineObservationEvent::BeaconPrepared => {
                observation.beacons_prepared = observation.beacons_prepared.saturating_add(1);
            }
            Esp32s31ApEngineObservationEvent::AuthenticationResponsePrepared => {
                observation.authentication_responses_prepared = observation
                    .authentication_responses_prepared
                    .saturating_add(1);
            }
            Esp32s31ApEngineObservationEvent::AssociationResponsePrepared { associated_peers } => {
                observation.association_responses_prepared =
                    observation.association_responses_prepared.saturating_add(1);
                observation.maximum_associated_peers =
                    observation.maximum_associated_peers.max(associated_peers);
            }
            Esp32s31ApEngineObservationEvent::PeerRemoved => {
                observation.peer_removals = observation.peer_removals.saturating_add(1);
            }
            Esp32s31ApEngineObservationEvent::TxBlockAckResponseObserved => {
                observation.tx_block_ack_responses_observed = observation
                    .tx_block_ack_responses_observed
                    .saturating_add(1);
            }
            Esp32s31ApEngineObservationEvent::TxBlockAckOperational => {
                observation.tx_block_ack_agreements_operational = observation
                    .tx_block_ack_agreements_operational
                    .saturating_add(1);
            }
            Esp32s31ApEngineObservationEvent::TxBlockAckRejected => {
                observation.tx_block_ack_responses_rejected = observation
                    .tx_block_ack_responses_rejected
                    .saturating_add(1);
            }
            Esp32s31ApEngineObservationEvent::TxBlockAckRequestPrepared => {
                observation.tx_block_ack_requests_prepared =
                    observation.tx_block_ack_requests_prepared.saturating_add(1);
            }
            Esp32s31ApEngineObservationEvent::TxBlockAckNegotiationTimeout => {
                observation.tx_block_ack_negotiation_timeouts = observation
                    .tx_block_ack_negotiation_timeouts
                    .saturating_add(1);
            }
            Esp32s31ApEngineObservationEvent::PeerAuthorized { authorized_peers } => {
                observation.authorized_peers = observation.authorized_peers.saturating_add(1);
                observation.maximum_authorized_peers =
                    observation.maximum_authorized_peers.max(authorized_peers);
            }
            Esp32s31ApEngineObservationEvent::Wpa2ResponseWindow => {
                observation.wpa2_response_windows =
                    observation.wpa2_response_windows.saturating_add(1);
            }
            Esp32s31ApEngineObservationEvent::Wpa2Retransmission => {
                observation.wpa2_retransmissions =
                    observation.wpa2_retransmissions.saturating_add(1);
            }
            Esp32s31ApEngineObservationEvent::Wpa2HandshakeTimeout => {
                observation.wpa2_handshake_timeouts =
                    observation.wpa2_handshake_timeouts.saturating_add(1);
            }
            Esp32s31ApEngineObservationEvent::AuthenticationTimeout => {
                observation.authentication_timeouts =
                    observation.authentication_timeouts.saturating_add(1);
            }
            Esp32s31ApEngineObservationEvent::Wpa2HandshakeFailure => {
                observation.wpa2_handshake_failures =
                    observation.wpa2_handshake_failures.saturating_add(1);
            }
            Esp32s31ApEngineObservationEvent::InactivityTimeout => {
                observation.inactivity_timeouts = observation.inactivity_timeouts.saturating_add(1);
            }
            Esp32s31ApEngineObservationEvent::Wpa2PendingOnStop => {
                observation.wpa2_pending_on_stop =
                    observation.wpa2_pending_on_stop.saturating_add(1);
            }
            Esp32s31ApEngineObservationEvent::DisassociationPrepared => {
                observation.disassociations_prepared =
                    observation.disassociations_prepared.saturating_add(1);
            }
            Esp32s31ApEngineObservationEvent::DeauthenticationPrepared => {
                observation.deauthentications_prepared =
                    observation.deauthentications_prepared.saturating_add(1);
            }
        }
    }
}

/// Active AP policy and hardware-key owner.
///
/// Dropping this value loses the only route back to role-neutral Wi-Fi, so a
/// supervisor can only acknowledge stop after consuming [`stop`](Self::stop).
#[must_use = "an active AP engine must be consumed through stop before radio reuse"]
pub struct Esp32s31ApEngine<'storage> {
    service: AccessPointService<'storage>,
    beacon: Esp32s31ApBeacon<'storage>,
    security: Esp32s31ApSecurity<'storage>,
    rx_peer: Option<Esp32s31ApRxPeerBinding>,
    channel: WifiChannel,
    #[cfg(any(feature = "diagnostics", test))]
    observer: Esp32s31ApEngineObserver,
}

impl<'storage> Esp32s31ApEngine<'storage> {
    // Start failure must return the affine service and caller-owned beacon
    // storage. This no-alloc driver cannot box that rollback value merely to
    // shrink the Result discriminant.
    #[allow(clippy::result_large_err, clippy::too_many_arguments)]
    pub fn start<H: Esp32s31ApRuntimeHardware>(
        hardware: &mut H,
        service: AccessPointService<'storage>,
        beacon_storage: &'storage mut [u8; WPA2_BEACON_CAPACITY],
        pairwise_storage: &'storage mut Esp32s31ApPairwiseKeyStorage,
        ssid: &WifiSsid,
        channel: WifiChannel,
        beacon_interval_tu: u16,
        dtim_period: u8,
    ) -> Result<Self, Esp32s31ApEngineStartFailure<'storage>> {
        let security_mode = service.security_mode();
        let beacon_len = match write_ht_beacon(
            &crate::profile::ADVERTISEMENT,
            beacon_storage,
            service.address(),
            ssid,
            channel,
            beacon_interval_tu,
            dtim_period,
            0,
            security_mode,
        ) {
            Ok(len) => len,
            Err(error) => {
                return Err(Esp32s31ApEngineStartFailure {
                    service,
                    beacon_storage,
                    pairwise_storage,
                    error: Esp32s31ApEngineError::Beacon(error),
                });
            }
        };
        let beacon =
            Esp32s31ApBeacon::from_initialized(beacon_storage, beacon_len, beacon_interval_tu);
        configure_ap_receive_policy(hardware, service.address());
        let security = match security_mode {
            WifiSecurityMode::Open => Esp32s31ApSecurity::open(pairwise_storage),
            WifiSecurityMode::Wpa2Personal => Esp32s31ApSecurity::install_group(
                hardware,
                service
                    .gtk()
                    .expect("WPA2 service mode owns one GTK for this epoch"),
                pairwise_storage,
            ),
        };
        let security = match security {
            Ok(security) => security,
            Err(failure) => {
                disable_ap_receive_policy(hardware);
                return Err(Esp32s31ApEngineStartFailure {
                    service,
                    beacon_storage: beacon.into_storage(),
                    pairwise_storage: failure.storage,
                    error: Esp32s31ApEngineError::Security(failure.error),
                });
            }
        };
        // This is the first irreversible timing edge. Beacon construction and
        // group-key installation have succeeded, so a failed start never
        // leaves an unowned hardware TSF epoch behind.
        reset_and_start_access_point_tsf(hardware);
        Ok(Self {
            service,
            beacon,
            security,
            rx_peer: None,
            channel,
            #[cfg(any(feature = "diagnostics", test))]
            observer: Esp32s31ApEngineObserver::default(),
        })
    }

    pub const fn channel(&self) -> WifiChannel {
        self.channel
    }

    pub const fn security_mode(&self) -> WifiSecurityMode {
        self.service.security_mode()
    }

    #[cfg(any(feature = "diagnostics", test))]
    #[inline(always)]
    fn observe(&mut self, event: Esp32s31ApEngineObservationEvent) {
        self.observer.observe(event);
    }

    pub fn prepare_beacon(&mut self, executor_timestamp_micros: u64) -> Option<&mut [u8]> {
        self.prepare_beacon_publication(executor_timestamp_micros)
            .map(|publication| publication.frame)
    }

    /// Prepare one beacon together with the exact DTIM group prefix encoded in
    /// that frame. The returned count must not be released until terminal
    /// beacon publication succeeds.
    pub fn prepare_beacon_publication(
        &mut self,
        executor_timestamp_micros: u64,
    ) -> Option<Esp32s31ApBeaconPublication<'_>> {
        let group_pending = self.service.group_traffic_pending();
        let buffered_group_frames = self.service.buffered_group_frames();
        let unicast_tim_bitmap = self.service.unicast_tim_bitmap().ok()?;
        let unicast_tim_bitmap = unicast_tim_bitmap.partial();
        let management_sequence = self.service.next_management_sequence();
        let beacon = self.beacon.prepare(
            executor_timestamp_micros,
            management_sequence,
            group_pending,
            unicast_tim_bitmap,
        );
        if beacon.is_some() {
            #[cfg(any(feature = "diagnostics", test))]
            self.observer
                .observe(Esp32s31ApEngineObservationEvent::BeaconPrepared);
        }
        let frame = beacon?;
        let (tim_offset, dtim_count, _) = dtim(frame)?;
        let group_indicated = dtim_count == 0 && frame.get(tim_offset + 4).copied()? & 1 != 0;
        Some(Esp32s31ApBeaconPublication {
            frame,
            dtim_group_frames: if group_indicated {
                buffered_group_frames
            } else {
                0
            },
        })
    }

    pub const fn next_beacon_delay(&self, now_micros: u32) -> Option<(u32, u32)> {
        self.beacon.next_delay(now_micros)
    }

    pub const fn beacon_publication_due(&self, now_micros: u32) -> bool {
        self.beacon.publication_due(now_micros)
    }

    pub const fn beacon_publication_lateness(&self, now_micros: u32) -> (u32, u32) {
        self.beacon.publication_lateness(now_micros)
    }

    /// Install the PTK only after message four has been MIC-verified, then
    /// atomically open the portable controlled port.
    pub fn authorize_peer<H: Esp32s31ApRuntimeHardware>(
        &mut self,
        hardware: &mut H,
        peer: [u8; 6],
        now_micros: u64,
    ) -> Result<(), Esp32s31ApEngineError> {
        if !self.service.wpa2_authorized(peer)? {
            return Err(Esp32s31ApEngineError::Service(
                ApServiceError::WrongPeerPhase,
            ));
        }
        let ptk = self.service.pending_ptk(peer)?;
        let association_id = self
            .service
            .peer_status(peer)
            .ok_or(ApServiceError::UnknownPeer)?
            .association_id;
        self.security
            .install_pairwise(hardware, peer, association_id, ptk)?;
        self.service.authorize(peer, now_micros)?;
        #[cfg(any(feature = "diagnostics", test))]
        self.observe(Esp32s31ApEngineObservationEvent::PeerAuthorized {
            authorized_peers: self.service.authorized_count(),
        });
        Ok(())
    }

    /// Resolve one received EAPOL-Key frame and publish the pairwise hardware
    /// key before reporting that the controlled port may open.
    pub fn handle_eapol<H: Esp32s31ApRuntimeHardware, const N: usize>(
        &mut self,
        hardware: &mut H,
        peer: [u8; 6],
        frame: OwnedEapolFrame<N>,
        now_micros: u64,
    ) -> Result<Esp32s31ApWpa2Outcome<N>, Esp32s31ApEngineError> {
        let Some(status) = self.service.peer_status(peer) else {
            return Ok(Esp32s31ApWpa2Outcome::None);
        };
        if status.phase != ApPeerPhase::Securing {
            // Late/replayed EAPOL is an untrusted peer event. Vendor receive
            // dispatch returns from such state branches; it does not abort
            // the PP task or surrender MAC ownership.
            return Ok(Esp32s31ApWpa2Outcome::None);
        }
        match self.service.on_eapol(peer, frame)? {
            ApWpa2Progress::None => Ok(Esp32s31ApWpa2Outcome::None),
            ApWpa2Progress::Transmit(frame) => Ok(Esp32s31ApWpa2Outcome::Transmit(frame)),
            ApWpa2Progress::AuthorizePeer => {
                self.authorize_peer(hardware, peer, now_micros)?;
                Ok(Esp32s31ApWpa2Outcome::PeerAuthorized { peer })
            }
            ApWpa2Progress::DeauthenticatePeer => {
                Ok(Esp32s31ApWpa2Outcome::DeauthenticatePeer { peer })
            }
        }
    }

    #[cfg(any(feature = "diagnostics", test))]
    pub fn observation(&self) -> Esp32s31ApEngineObservation {
        self.observer.observation
    }

    pub const fn service_address(&self) -> [u8; 6] {
        self.service.address()
    }

    #[cfg(test)]
    pub(crate) fn current_data_sequence(&self) -> u16 {
        self.service.current_data_sequence()
    }

    #[cfg(test)]
    pub(crate) fn current_qos_sequence(&self, peer: [u8; 6], tid: u8) -> Option<u16> {
        self.service.current_qos_sequence(peer, tid)
    }

    pub fn peer_status(&self, peer: [u8; 6]) -> Option<ApPeerStatus> {
        self.service.peer_status(peer)
    }

    /// Derive the BSS-wide HT protection requirement from associated peers.
    ///
    /// A peer which reached Association without HT capabilities places an HT
    /// BSS in non-HT mixed mode. A maximum advertised legacy rate no faster
    /// than 11 Mbit/s cannot prove ERP membership, so that peer conservatively
    /// enables ERP Use Protection. Any OFDM maximum is positive ERP proof.
    pub fn tx_protection_policy(&self) -> WifiTxProtectionPolicy {
        let mut non_erp_member = false;
        let mut non_ht_member = false;
        for peer in self
            .service
            .peers()
            .filter(|peer| peer.phase != ApPeerPhase::Authenticated)
        {
            non_erp_member |= peer.maximum_legacy_rate_500kbps <= 22;
            non_ht_member |= peer.ht.is_none();
        }
        WifiTxProtectionPolicy::new(
            if non_erp_member {
                ErpProtectionMode::CtsToSelf
            } else {
                ErpProtectionMode::None
            },
            if non_ht_member {
                HtProtectionMode::NonHtMixed
            } else {
                HtProtectionMode::None
            },
            None,
        )
    }

    #[inline(always)]
    pub fn is_authorized_peer(&self, peer: [u8; 6]) -> bool {
        self.service.is_authorized(peer)
    }

    pub fn authorized_peer_count(&self) -> u8 {
        self.service.authorized_count()
    }

    pub fn associated_peer_count(&self) -> u8 {
        self.service.associated_count()
    }

    pub fn service_status(&self) -> open_esp_radio_wifi_ap::AccessPointServiceStatus {
        self.service.status()
    }

    pub const fn service_status_revision(&self) -> u32 {
        self.service.status_revision()
    }

    pub fn next_peer_deadline(&self) -> Option<u64> {
        self.service.next_peer_deadline()
    }

    pub fn next_wpa2_retry_deadline(&self) -> Option<u64> {
        self.service.next_wpa2_retry_deadline()
    }

    pub fn observe_wpa2_transmit(
        &mut self,
        peer: [u8; 6],
        retransmission: bool,
        acknowledged: bool,
        now_micros: u64,
    ) -> Result<(), Esp32s31ApEngineError> {
        if self
            .service
            .observe_wpa2_transmit(peer, retransmission, acknowledged, now_micros)?
        {
            #[cfg(any(feature = "diagnostics", test))]
            self.observe(Esp32s31ApEngineObservationEvent::Wpa2ResponseWindow);
        }
        Ok(())
    }

    pub fn take_due_wpa2_retry<const N: usize>(
        &mut self,
        now_micros: u64,
    ) -> Result<ApWpa2RetryProgress<N>, Esp32s31ApEngineError> {
        let progress = self.service.take_due_wpa2_retry(now_micros)?;
        match progress {
            ApWpa2RetryProgress::Transmit { .. } => {
                #[cfg(any(feature = "diagnostics", test))]
                self.observe(Esp32s31ApEngineObservationEvent::Wpa2Retransmission);
            }
            ApWpa2RetryProgress::Close(_) => {
                #[cfg(any(feature = "diagnostics", test))]
                self.observe(Esp32s31ApEngineObservationEvent::Wpa2HandshakeTimeout);
            }
            ApWpa2RetryProgress::None => {}
        }
        Ok(progress)
    }

    pub fn observe_peer_activity(
        &mut self,
        peer: [u8; 6],
        now_micros: u64,
    ) -> Result<(), Esp32s31ApEngineError> {
        if let Some(binding) = self.rx_peer
            && binding.peer.address() == peer
            && binding.status_revision == self.service.status_revision()
        {
            self.service
                .observe_bound_data_activity(binding.peer, now_micros)?;
            return Ok(());
        }
        self.service.observe_activity(peer, now_micros)?;
        Ok(())
    }

    pub fn begin_due_peer_close(&mut self, now_micros: u64) -> Option<ApPeerClose> {
        let close = self.service.begin_due_peer_close(now_micros)?;
        match close.kind {
            ApPeerCloseKind::AuthenticationTimeout => {
                #[cfg(any(feature = "diagnostics", test))]
                self.observe(Esp32s31ApEngineObservationEvent::AuthenticationTimeout);
            }
            ApPeerCloseKind::Wpa2HandshakeTimeout => {
                #[cfg(any(feature = "diagnostics", test))]
                self.observe(Esp32s31ApEngineObservationEvent::Wpa2HandshakeTimeout);
            }
            ApPeerCloseKind::Wpa2HandshakeFailure => {
                #[cfg(any(feature = "diagnostics", test))]
                self.observe(Esp32s31ApEngineObservationEvent::Wpa2HandshakeFailure);
            }
            ApPeerCloseKind::InactivityTimeout => {
                #[cfg(any(feature = "diagnostics", test))]
                self.observe(Esp32s31ApEngineObservationEvent::InactivityTimeout);
            }
            ApPeerCloseKind::AccessPointStop => {}
        }
        Some(close)
    }

    pub fn begin_wpa2_failure_close(
        &mut self,
        peer: [u8; 6],
    ) -> Result<ApPeerClose, Esp32s31ApEngineError> {
        let close = self.service.begin_wpa2_failure_close(peer)?;
        #[cfg(any(feature = "diagnostics", test))]
        self.observe(Esp32s31ApEngineObservationEvent::Wpa2HandshakeFailure);
        Ok(close)
    }

    pub fn begin_stop_peer(&mut self) -> Option<ApPeerClose> {
        if self.service.next_wpa2_retry_deadline().is_some() {
            #[cfg(any(feature = "diagnostics", test))]
            self.observe(Esp32s31ApEngineObservationEvent::Wpa2PendingOnStop);
        }
        self.service.begin_stop_peer()
    }

    pub fn encode_peer_disconnect(
        &mut self,
        close: ApPeerClose,
        kind: ApPeerDisconnectKind,
        reason: u16,
        output: &mut [u8],
    ) -> Result<usize, Esp32s31ApEngineError> {
        let status = self
            .service
            .peer_status(close.peer)
            .ok_or(ApServiceError::UnknownPeer)?;
        if status.phase != ApPeerPhase::Closing {
            return Err(ApServiceError::WrongPeerPhase.into());
        }
        let sequence = self.service.next_management_sequence();
        let length = write_ap_peer_disconnect(
            output,
            self.service.address(),
            close.peer,
            kind,
            reason,
            sequence,
        )?;
        match kind {
            ApPeerDisconnectKind::Disassociation => {
                #[cfg(any(feature = "diagnostics", test))]
                self.observe(Esp32s31ApEngineObservationEvent::DisassociationPrepared);
            }
            ApPeerDisconnectKind::Deauthentication => {
                #[cfg(any(feature = "diagnostics", test))]
                self.observe(Esp32s31ApEngineObservationEvent::DeauthenticationPrepared);
            }
        }
        Ok(length)
    }

    pub fn complete_peer_close<H: Esp32s31ApRuntimeHardware>(
        &mut self,
        hardware: &mut H,
        close: ApPeerClose,
    ) -> Result<(), Esp32s31ApEngineError> {
        let status = self
            .service
            .peer_status(close.peer)
            .ok_or(ApServiceError::UnknownPeer)?;
        if status.phase != ApPeerPhase::Closing {
            return Err(ApServiceError::WrongPeerPhase.into());
        }
        self.security.clear_peer(hardware, close.peer)?;
        self.service.remove_peer(close.peer)?;
        #[cfg(any(feature = "diagnostics", test))]
        self.observe(Esp32s31ApEngineObservationEvent::PeerRemoved);
        Ok(())
    }

    pub fn stop<H: Esp32s31ApRuntimeHardware>(
        self,
        hardware: &mut H,
    ) -> Esp32s31ApEngineStop<'storage> {
        // RX admission closes before the outer descriptor epoch is allowed
        // to stop. A composition may have already performed this idempotent
        // pre-quiesce leaf while it still owned the live runner.
        disable_ap_receive_policy(hardware);
        let (security, pairwise_storage) = self.security.stop(hardware);
        stop_access_point_tsf(hardware);
        Esp32s31ApEngineStop {
            service: self.service,
            beacon_storage: self.beacon.into_storage(),
            pairwise_storage,
            security,
        }
    }
}

#[cfg(test)]
mod tests;
