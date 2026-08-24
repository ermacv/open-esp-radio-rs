//! One bounded AP MAC epoch above DMA/IRQ and below the Embassy actor.

use crate::{
    beacon::Esp32s31ApBeacon,
    rx::{
        Esp32s31ApRxAdmission, Esp32s31ApRxAdmissionRequest, Esp32s31ApRxDuplicateOwner,
        Esp32s31ApRxError,
    },
    security::{
        Esp32s31ApPairwiseBinding, Esp32s31ApPairwiseKeyStorage, Esp32s31ApSecurity,
        Esp32s31ApSecurityError, Esp32s31ApSecurityStopReport,
    },
};
use open_esp_radio_esp32s31_wifi_mac::{
    ap_policy::{ApRxPolicyHardware, configure_ap_receive_policy},
    ap_tsf::{ApTsfHardware, reset_and_start_access_point_tsf, stop_access_point_tsf},
    crypto::{CcmpKeyHardware, CryptoKeyError},
};
use open_esp_radio_ieee80211::block_ack::{
    OperationalTxBlockAck, TxBlockAckAlarm, TxBlockAckResponse,
};
use open_esp_radio_ieee80211::{
    ap::{
        ApActionFrame, ApAssociationResponseError, ApDataFrame, ApDataFrameError,
        ApManagementRequest, ApPeerDisconnectKind, ApPowerSaveObservation, ApProtectedDataFrame,
        ApUnprotectedDataFrame, EncodedApFrame, observe_ap_power_save_for_access_point,
        parse_ap_management_request, write_ap_peer_disconnect,
        write_ht_association_response_frame_for_security, write_open_authentication_response,
    },
    beacon::{ApBeaconBuildError, WPA2_BEACON_CAPACITY, dtim, write_ht_beacon},
    ccmp::{CcmpKeyId, CcmpReplayLane},
    channel::WifiChannel,
    security::WifiSecurityMode,
    ssid::WifiSsid,
};
use open_esp_radio_wifi_ap::{
    AccessPointService, ApAssociationCapabilities, ApBufferedGroupRelease,
    ApBufferedUnicastRelease, ApDownlinkDisposition, ApMlmeAction, ApPeerBinding, ApPeerClose,
    ApPeerCloseKind, ApPeerPhase, ApPeerStatus, ApPowerSaveAction, ApServiceError, ApWpa2Error,
    ApWpa2Progress, ApWpa2RetryProgress,
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

    /// Build handshake message one after the successful Association Response
    /// has reached TX completion.
    pub fn begin_wpa2<const N: usize>(
        &self,
        peer: [u8; 6],
    ) -> Result<Wpa2TxFrame<N>, Esp32s31ApEngineError> {
        Ok(self.service.begin_wpa2_frame(peer)?)
    }

    /// Encode one AP recipient response to a peer-originated BlockAck action.
    /// Agreement state, hardware publication and TX completion remain owned
    /// by the caller; the engine owns only AP addressing and sequence space.
    pub fn encode_rx_block_ack_response(
        &mut self,
        peer: [u8; 6],
        body: &[u8],
        output: &mut [u8],
    ) -> Result<usize, Esp32s31ApEngineError> {
        let sequence = self.service.next_management_sequence();
        Ok(ApActionFrame {
            access_point: self.service.address(),
            peer,
            sequence_number: sequence,
            body,
        }
        .encode(output)?)
    }

    pub fn handle_management<H: Esp32s31ApRuntimeHardware>(
        &mut self,
        hardware: &mut H,
        frame: &[u8],
        authenticator_nonce: [u8; 32],
        initial_replay_counter: u64,
        now_micros: u64,
        output: &mut [u8],
    ) -> Result<Esp32s31ApManagementOutcome, Esp32s31ApEngineError> {
        let Some(request) = parse_ap_management_request(frame, self.service.address()) else {
            return Ok(Esp32s31ApManagementOutcome::Ignored);
        };
        let retry = frame.get(1).is_some_and(|byte| byte & 0x08 != 0);
        match request {
            ApManagementRequest::OpenAuthentication { peer } => {
                if retry
                    && self
                        .service
                        .peer_status(peer)
                        .is_some_and(|status| status.phase != ApPeerPhase::Authenticated)
                {
                    // libnet80211 routes retry/duplicate management frames as
                    // ordinary receive outcomes; it does not tear down its
                    // station node merely because an earlier response ACK was
                    // lost. Re-emit success while preserving the current WPA2
                    // or authorized key epoch. A non-retry authentication
                    // below still owns the explicit reauthentication reset.
                    let sequence = self.service.next_management_sequence();
                    let len = write_open_authentication_response(
                        output,
                        self.service.address(),
                        peer,
                        0,
                        sequence,
                    )?;
                    #[cfg(any(feature = "diagnostics", test))]
                    self.observe(Esp32s31ApEngineObservationEvent::AuthenticationResponsePrepared);
                    return Ok(Esp32s31ApManagementOutcome::Response {
                        len,
                        begin_wpa2: false,
                    });
                }
                // A peer may restart authentication without first sending a
                // deauthentication frame. End its old pairwise-key epoch
                // before the portable service resets the handshake state;
                // otherwise the stable AID would still own a stale hardware
                // entry and the next authorization could not install its PTK.
                if self.service.peer_status(peer).is_some() {
                    self.security.clear_peer(hardware, peer)?;
                }
                let ApMlmeAction::AuthenticationResponse { status, .. } =
                    self.service.authenticate_open(peer, now_micros)
                else {
                    unreachable!("authenticate_open has one response action")
                };
                let sequence = self.service.next_management_sequence();
                let len = write_open_authentication_response(
                    output,
                    self.service.address(),
                    peer,
                    status,
                    sequence,
                )?;
                #[cfg(any(feature = "diagnostics", test))]
                self.observe(Esp32s31ApEngineObservationEvent::AuthenticationResponsePrepared);
                Ok(Esp32s31ApManagementOutcome::Response {
                    len,
                    begin_wpa2: false,
                })
            }
            ApManagementRequest::Association {
                peer,
                security,
                maximum_legacy_rate_500kbps,
                ht_capabilities,
                qos_supported,
            } => {
                let Some(peer_status) = self.service.peer_status(peer) else {
                    // Vendor `hostap_recv_mgmt` treats a peer lookup miss as
                    // an on-air management outcome (including explicit
                    // deauthentication paths), never as a task failure. This
                    // bounded port has no deauthentication response for the
                    // class-2 case yet, so retain the owner and ignore it.
                    return Ok(Esp32s31ApManagementOutcome::Ignored);
                };
                if self.service.matches_association_security(security)
                    && (peer_status.phase == ApPeerPhase::Securing
                        || (self.service.security_mode() == WifiSecurityMode::Open
                            && peer_status.phase == ApPeerPhase::Authorized))
                {
                    // A station can repeat Association Request when the first
                    // response ACK was lost. Preserve the in-flight WPA2
                    // state, retransmit the same successful association and
                    // do not start a second Message-1 transaction.
                    let sequence = self.service.next_management_sequence();
                    let len = write_ht_association_response_frame_for_security(
                        output,
                        self.service.address(),
                        peer,
                        0,
                        peer_status.association_id,
                        sequence,
                        self.channel,
                        peer_status.ht,
                        self.service.security_mode(),
                    )?;
                    #[cfg(any(feature = "diagnostics", test))]
                    self.observe(
                        Esp32s31ApEngineObservationEvent::AssociationResponsePrepared {
                            associated_peers: self.service.associated_count(),
                        },
                    );
                    return Ok(Esp32s31ApManagementOutcome::Response {
                        len,
                        begin_wpa2: false,
                    });
                }
                if peer_status.phase != ApPeerPhase::Authenticated {
                    return Ok(Esp32s31ApManagementOutcome::Ignored);
                }
                let capabilities = ApAssociationCapabilities {
                    maximum_legacy_rate_500kbps,
                    ht: ht_capabilities,
                    qos_supported,
                };
                let action = match self.service.security_mode() {
                    WifiSecurityMode::Open => {
                        self.service
                            .associate_open(peer, security, capabilities, now_micros)?
                    }
                    WifiSecurityMode::Wpa2Personal => self.service.associate_wpa2(
                        peer,
                        security,
                        capabilities,
                        authenticator_nonce,
                        initial_replay_counter,
                        now_micros,
                    )?,
                };
                let ApMlmeAction::AssociationResponse {
                    status,
                    association_id,
                    ..
                } = action
                else {
                    unreachable!("AP association has one response action")
                };
                let sequence = self.service.next_management_sequence();
                let len = write_ht_association_response_frame_for_security(
                    output,
                    self.service.address(),
                    peer,
                    status,
                    association_id.unwrap_or(0),
                    sequence,
                    self.channel,
                    ht_capabilities,
                    self.service.security_mode(),
                )?;
                if association_id.is_some() {
                    // Recovered `ic_set_sta` evidence gives the legacy B/G
                    // path a software transmit-rate context, represented here
                    // by the peer's negotiated rate and stable AID. Its extra
                    // station-programming calls are HE-only. Pairwise hardware
                    // state is installed later through the AID-owned key slot,
                    // so the current B/G AP has no unevidenced station-table
                    // MMIO operation to imitate.
                }
                #[cfg(any(feature = "diagnostics", test))]
                self.observe(
                    Esp32s31ApEngineObservationEvent::AssociationResponsePrepared {
                        associated_peers: self.service.associated_count(),
                    },
                );
                #[cfg(any(feature = "diagnostics", test))]
                if association_id.is_some()
                    && self.service.security_mode() == WifiSecurityMode::Open
                {
                    self.observe(Esp32s31ApEngineObservationEvent::PeerAuthorized {
                        authorized_peers: self.service.authorized_count(),
                    });
                }
                Ok(Esp32s31ApManagementOutcome::Response {
                    len,
                    begin_wpa2: association_id.is_some()
                        && self.service.security_mode() == WifiSecurityMode::Wpa2Personal,
                })
            }
            ApManagementRequest::Disassociation { peer, .. }
            | ApManagementRequest::Deauthentication { peer, .. } => {
                let Some(peer_status) = self.service.peer_status(peer) else {
                    return Ok(Esp32s31ApManagementOutcome::Ignored);
                };
                // Once local timeout/stop teardown owns the peer, its ordered
                // disassociation -> deauthentication -> key clear transaction
                // is authoritative. A peer response racing that transaction
                // must not remove the state below the in-flight TX owner.
                if peer_status.phase == ApPeerPhase::Closing {
                    return Ok(Esp32s31ApManagementOutcome::Ignored);
                }
                self.security.clear_peer(hardware, peer)?;
                self.service.remove_peer(peer)?;
                #[cfg(any(feature = "diagnostics", test))]
                self.observe(Esp32s31ApEngineObservationEvent::PeerRemoved);
                Ok(Esp32s31ApManagementOutcome::PeerRemoved { peer })
            }
            ApManagementRequest::BlockAck { peer, action } => {
                if self.service.peer_status(peer).is_none() {
                    return Ok(Esp32s31ApManagementOutcome::Ignored);
                }
                if let Some(response) = self.service.on_tx_block_ack_action(peer, action)? {
                    #[cfg(any(feature = "diagnostics", test))]
                    self.observe(Esp32s31ApEngineObservationEvent::TxBlockAckResponseObserved);
                    match response {
                        TxBlockAckResponse::Operational(_) => {
                            #[cfg(any(feature = "diagnostics", test))]
                            self.observe(Esp32s31ApEngineObservationEvent::TxBlockAckOperational);
                        }
                        TxBlockAckResponse::Rejected(_) => {
                            #[cfg(any(feature = "diagnostics", test))]
                            self.observe(Esp32s31ApEngineObservationEvent::TxBlockAckRejected);
                        }
                    }
                }
                Ok(Esp32s31ApManagementOutcome::Ignored)
            }
        }
    }

    /// Prepare the AP-originated TID-0 ADDBA request for an authorized HT
    /// peer. The peer table owns both the negotiation and its timer token.
    pub fn prepare_tx_block_ack_request(
        &mut self,
        peer: [u8; 6],
        now_micros: u64,
        output: &mut [u8],
    ) -> Result<Option<(usize, TxBlockAckAlarm)>, Esp32s31ApEngineError> {
        let Some(request) = self.service.begin_tx_block_ack(peer, now_micros)? else {
            return Ok(None);
        };
        let sequence = self.service.next_management_sequence();
        let length = ApActionFrame {
            access_point: self.service.address(),
            peer,
            sequence_number: sequence,
            body: &request.body,
        }
        .encode(output)?;
        #[cfg(any(feature = "diagnostics", test))]
        self.observe(Esp32s31ApEngineObservationEvent::TxBlockAckRequestPrepared);
        Ok(Some((length, request.alarm)))
    }

    pub fn tx_block_ack_agreement(&self, peer: [u8; 6]) -> Option<OperationalTxBlockAck> {
        self.service.peer_status(peer)?.tx_block_ack
    }

    pub fn bind_aggregate_peer(
        &self,
        peer: [u8; 6],
    ) -> Result<(Esp32s31ApAggregateBinding, ApPeerStatus), Esp32s31ApEngineError> {
        let service_binding = self
            .service
            .bind_peer(peer)
            .ok_or(ApServiceError::UnknownPeer)?;
        let status = self
            .service
            .bound_peer_status(service_binding)
            .ok_or(ApServiceError::UnknownPeer)?;
        let security = self.security.bind_pairwise(peer, status.association_id)?;
        Ok((
            Esp32s31ApAggregateBinding {
                peer: service_binding,
                security,
            },
            status,
        ))
    }

    pub fn has_operational_tx_block_ack(&self) -> bool {
        self.service.has_operational_tx_block_ack()
    }

    pub fn smallest_operational_tx_block_ack_window(&self) -> Option<u16> {
        self.service.smallest_operational_tx_block_ack_window()
    }

    pub fn observe_tx_block_ack_alarm(
        &mut self,
        peer: [u8; 6],
        alarm: TxBlockAckAlarm,
    ) -> Result<bool, Esp32s31ApEngineError> {
        let expired = self.service.on_tx_block_ack_alarm(peer, alarm)?;
        if expired {
            #[cfg(any(feature = "diagnostics", test))]
            self.observe(Esp32s31ApEngineObservationEvent::TxBlockAckNegotiationTimeout);
        }
        Ok(expired)
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

    /// Encode one authenticator EAPOL action as an unprotected AP data MPDU.
    /// The sequence number is consumed only when the complete frame fits.
    pub fn encode_eapol<const N: usize>(
        &mut self,
        peer: [u8; 6],
        frame: &Wpa2TxFrame<N>,
        output: &mut [u8],
    ) -> Result<usize, Esp32s31ApEngineError> {
        let sequence_number = self.service.current_data_sequence();
        let len = ApDataFrame {
            access_point: self.service.address(),
            destination: peer,
            sequence_number,
            ether_type: 0x888e,
            payload: frame.as_bytes(),
        }
        .encode(output)?;
        // Consume protocol state in ordinary code. Keeping this call inside
        // `debug_assert_eq!` made release builds transmit every AP data MPDU
        // with sequence number zero because the assertion expression is
        // compiled out.
        let consumed_sequence_number = self.service.next_data_sequence();
        debug_assert_eq!(consumed_sequence_number, sequence_number);
        Ok(len)
    }

    /// Encode one network-owned Ethernet frame under the exact AP security
    /// mode. Open yields a plaintext non-QoS MPDU and no key selector; WPA2
    /// retains the original CCMP path.
    pub fn encode_protected_ethernet(
        &mut self,
        destination: [u8; 6],
        ethernet: &[u8],
        output: &mut [u8],
    ) -> Result<Esp32s31ApProtectedFrame, Esp32s31ApEngineError> {
        self.encode_protected_ethernet_with_more_data(destination, ethernet, output, false)
    }

    /// Encode one protected downlink and explicitly own the More Data bit.
    ///
    /// Ordinary network traffic uses [`Self::encode_protected_ethernet`].
    /// AP power-save dequeue is the only production caller that may set this
    /// bit, after reserving the corresponding buffered-frame count.
    pub fn encode_protected_ethernet_with_more_data(
        &mut self,
        destination: [u8; 6],
        ethernet: &[u8],
        output: &mut [u8],
        more_data: bool,
    ) -> Result<Esp32s31ApProtectedFrame, Esp32s31ApEngineError> {
        if self.service.security_mode() == WifiSecurityMode::Open {
            let group = destination[0] & 1 != 0;
            if group {
                if self.service.authorized_count() == 0 {
                    return Err(ApServiceError::WrongPeerPhase.into());
                }
            } else if self.service.peer_status(destination).is_none() {
                return Err(ApServiceError::UnknownPeer.into());
            } else if !self.service.is_authorized(destination) {
                return Err(ApServiceError::WrongPeerPhase.into());
            }
            let sequence_number = self.service.current_data_sequence();
            let length = ApUnprotectedDataFrame {
                access_point: self.service.address(),
                peer: destination,
                sequence_number,
                more_data,
                ethernet,
            }
            .encode(output)?;
            let consumed = self.service.next_data_sequence();
            debug_assert_eq!(consumed, sequence_number);
            return Ok(Esp32s31ApProtectedFrame {
                length,
                hardware_key_selector: None,
            });
        }
        let group = destination[0] & 1 != 0;
        let (hardware_key_selector, ccmp_header, peer_qos) = if group {
            if self.service.authorized_count() == 0 {
                return Err(ApServiceError::WrongPeerPhase.into());
            }
            (
                self.security.group_hardware_index()?,
                self.security.next_group_tx_ccmp_header()?,
                false,
            )
        } else {
            if self.service.peer_status(destination).is_none() {
                return Err(ApServiceError::UnknownPeer.into());
            }
            if !self.service.is_authorized(destination) {
                return Err(ApServiceError::WrongPeerPhase.into());
            }
            let status = self
                .service
                .peer_status(destination)
                .ok_or(ApServiceError::UnknownPeer)?;
            (
                self.security.pairwise_hardware_index(destination)?,
                self.security.next_pairwise_tx_ccmp_header(destination)?,
                status.qos_supported,
            )
        };
        let sequence_number = if peer_qos {
            self.service
                .current_qos_sequence(open_esp_radio_wifi_ap::AP_TX_BLOCK_ACK_TID)
                .expect("AP TX data TID is representable")
        } else {
            self.service.current_data_sequence()
        };
        let length = ApProtectedDataFrame {
            access_point: self.service.address(),
            peer: destination,
            sequence_number,
            user_priority: 0,
            peer_qos,
            more_data,
            ccmp_header,
            ethernet,
        }
        .encode(output)?;
        // Advance only after the complete protected frame fits, but never as
        // an assertion side effect: release qualification must own the same
        // monotonic sequence space as debug tests.
        let consumed_sequence_number = if peer_qos {
            self.service
                .next_qos_sequence(open_esp_radio_wifi_ap::AP_TX_BLOCK_ACK_TID)
                .expect("AP TX data TID is representable")
        } else {
            self.service.next_data_sequence()
        };
        debug_assert_eq!(consumed_sequence_number, sequence_number);
        Ok(Esp32s31ApProtectedFrame {
            length,
            hardware_key_selector: Some(hardware_key_selector),
        })
    }

    /// AP-specific adapter from a network allocation to the role-neutral
    /// retained A-MPDU backing contract.
    ///
    /// Saturated AP TX executes this leaf once per MPDU (roughly 150,000
    /// calls per 16-second BA16 HIL interval). The PSRAM-code profile keeps
    /// only this measured synchronous encoder leaf in the semantic hot-text
    /// class; the linker, not the portable AP model, selects physical SRAM.
    #[cfg_attr(
        target_arch = "riscv32",
        unsafe(link_section = ".hot.text.open_radio_ap_tx_encode")
    )]
    #[inline(never)]
    pub fn encode_aggregate_ethernet_in_place(
        &mut self,
        binding: Esp32s31ApAggregateBinding,
        storage: &mut [u8],
        ethernet_offset: usize,
        ethernet_length: usize,
    ) -> Result<Esp32s31ApAggregateFrame, Esp32s31ApEngineError> {
        let status = self
            .service
            .bound_peer_status(binding.peer)
            .ok_or(ApServiceError::UnknownPeer)?;
        if status.phase != ApPeerPhase::Authorized
            || !status.qos_supported
            || status.tx_block_ack.is_none()
        {
            return Err(ApServiceError::WrongPeerPhase.into());
        }
        let peer = binding.peer();
        let hardware_key_selector = binding.security.hardware_index();
        let ccmp_header = self
            .security
            .next_bound_pairwise_tx_ccmp_header(binding.security)?;
        let sequence_number = self
            .service
            .current_qos_sequence(open_esp_radio_wifi_ap::AP_TX_BLOCK_ACK_TID)
            .expect("AP TX data TID is representable");
        let encoded = ApProtectedDataFrame {
            access_point: self.service.address(),
            peer,
            sequence_number,
            user_priority: open_esp_radio_wifi_ap::AP_TX_BLOCK_ACK_TID,
            peer_qos: true,
            more_data: false,
            ccmp_header,
            ethernet: &[],
        }
        .encode_in_place(storage, ethernet_offset, ethernet_length)?;
        let consumed = self
            .service
            .next_qos_sequence(open_esp_radio_wifi_ap::AP_TX_BLOCK_ACK_TID)
            .expect("AP TX data TID is representable");
        debug_assert_eq!(consumed, sequence_number);
        Ok(Esp32s31ApAggregateFrame {
            encoded,
            hardware_key_selector,
            sequence_number,
        })
    }

    #[cfg(any(feature = "diagnostics", test))]
    pub fn observation(&self) -> Esp32s31ApEngineObservation {
        self.observer.observation
    }

    pub const fn service_address(&self) -> [u8; 6] {
        self.service.address()
    }

    pub fn peer_status(&self, peer: [u8; 6]) -> Option<ApPeerStatus> {
        self.service.peer_status(peer)
    }

    /// Admit one AP data MPDU against the live controlled port and exact PTK
    /// generation, committing its CCMP PN before Ethernet publication.
    ///
    /// The RX dispatcher calls this only after software BlockAck release and
    /// hardware MIC verification. Keeping replay state beside the installed
    /// key makes PTK install, clear and reinstall the only reset edges.
    pub fn admit_rx_data(
        &mut self,
        request: Esp32s31ApRxAdmissionRequest,
    ) -> Esp32s31ApRxAdmission {
        let peer = request.peer();
        let Some(status) = self
            .service
            .peer_status(peer)
            .filter(|status| status.phase == ApPeerPhase::Authorized)
        else {
            return Esp32s31ApRxAdmission::unauthorized();
        };
        if matches!(request.lane(), CcmpReplayLane::Tid(_)) && !status.qos_supported {
            return Esp32s31ApRxAdmission::rejected(Esp32s31ApRxError::PeerQosMismatch);
        }
        // Resolve the infallible fixed duplicate slot before WPA2 replay can
        // commit its PN. A validated owner always has space; AID reuse changes
        // the epoch and atomically replaces stale duplicate history.
        let Some(duplicate_owner) =
            Esp32s31ApRxDuplicateOwner::new(status.association_id, status.association_epoch)
        else {
            return Esp32s31ApRxAdmission::rejected(Esp32s31ApRxError::KeyGenerationMismatch);
        };
        match (self.service.security_mode(), request.ccmp_header()) {
            (WifiSecurityMode::Open, None) => Esp32s31ApRxAdmission::authorized(duplicate_owner),
            (WifiSecurityMode::Wpa2Personal, Some(header)) => {
                if header.key_id() != CcmpKeyId::PAIRWISE {
                    return Esp32s31ApRxAdmission::rejected(Esp32s31ApRxError::PairwiseKeyId(
                        header.key_id().value(),
                    ));
                }
                let binding = match self.security.bind_pairwise(peer, status.association_id) {
                    Ok(binding) => binding,
                    Err(error) => return rejected_rx_security(error),
                };
                let candidate = match self.security.prepare_bound_pairwise_rx(
                    binding,
                    request.lane(),
                    header.packet_number(),
                ) {
                    Ok(candidate) => candidate,
                    Err(error) => return rejected_rx_security(error),
                };
                match self.security.commit_bound_pairwise_rx(candidate) {
                    Ok(()) => Esp32s31ApRxAdmission::authorized(duplicate_owner),
                    Err(error) => rejected_rx_security(error),
                }
            }
            _ => Esp32s31ApRxAdmission::rejected(Esp32s31ApRxError::SecurityModeMismatch),
        }
    }

    pub fn downlink_disposition(
        &self,
        peer: [u8; 6],
    ) -> Result<ApDownlinkDisposition, Esp32s31ApEngineError> {
        Ok(self.service.downlink_disposition(peer)?)
    }

    pub fn group_downlink_disposition(&self) -> ApDownlinkDisposition {
        self.service.group_downlink_disposition()
    }

    pub fn commit_buffered_unicast(&mut self, peer: [u8; 6]) -> Result<u16, Esp32s31ApEngineError> {
        Ok(self.service.commit_buffered_unicast(peer)?)
    }

    pub fn begin_buffered_unicast_release(
        &mut self,
        peer: [u8; 6],
    ) -> Result<Option<ApBufferedUnicastRelease>, Esp32s31ApEngineError> {
        Ok(self.service.begin_buffered_unicast_release(peer)?)
    }

    pub fn complete_buffered_unicast_release(
        &mut self,
        release: ApBufferedUnicastRelease,
        delivered: bool,
    ) -> Result<u16, Esp32s31ApEngineError> {
        Ok(self
            .service
            .complete_buffered_unicast_release(release, delivered)?)
    }

    pub fn observe_power_save(
        &mut self,
        observation: ApPowerSaveObservation,
        now_micros: u64,
    ) -> Result<ApPowerSaveAction, Esp32s31ApEngineError> {
        Ok(self.service.observe_power_save(observation, now_micros)?)
    }

    /// Parse and apply an AP power-save edge from one complete 802.11 MPDU.
    /// Non-PM frames are left to the ordinary receive classifier.
    pub fn observe_power_save_frame(
        &mut self,
        frame: &[u8],
        now_micros: u64,
    ) -> Result<Option<ApPowerSaveAction>, Esp32s31ApEngineError> {
        let Some(observation) =
            observe_ap_power_save_for_access_point(frame, self.service.address())
        else {
            return Ok(None);
        };
        self.observe_power_save(observation, now_micros).map(Some)
    }

    pub fn commit_buffered_group(&mut self) -> Result<u16, Esp32s31ApEngineError> {
        Ok(self.service.commit_buffered_group()?)
    }

    pub fn begin_buffered_group_release(
        &mut self,
    ) -> Result<Option<ApBufferedGroupRelease>, Esp32s31ApEngineError> {
        Ok(self.service.begin_buffered_group_release()?)
    }

    pub fn complete_buffered_group_release(
        &mut self,
        release: ApBufferedGroupRelease,
        delivered: bool,
    ) -> Result<u16, Esp32s31ApEngineError> {
        Ok(self
            .service
            .complete_buffered_group_release(release, delivered)?)
    }

    pub fn complete_buffered_group(
        &mut self,
        delivered: bool,
    ) -> Result<u16, Esp32s31ApEngineError> {
        Ok(self.service.complete_buffered_group(delivered)?)
    }

    pub fn discard_buffered_groups(&mut self) -> Result<u16, Esp32s31ApEngineError> {
        Ok(self.service.discard_buffered_groups()?)
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
mod tests {
    use super::*;
    use open_esp_radio_esp32s31_hal::types::MacKeyInstallOutcome;
    use open_esp_radio_ieee80211::ccmp::{CcmpHeader, CcmpPacketNumber, CcmpReplayError};
    use open_esp_radio_wpa2::{
        OwnedEapolFrame, Pmk, PtkContext, Wpa2Interface,
        frames::{OwnedRsnIe, Wpa2Gtk, Wpa2TxFrame},
    };

    #[derive(Default)]
    struct Hardware {
        policy: Option<[u8; 6]>,
        installed: std::vec::Vec<u8>,
        cleared: std::vec::Vec<u8>,
        tsf_started: bool,
        tsf_stopped: bool,
    }

    impl ApRxPolicyHardware for Hardware {
        fn apply_ap_link_policy(&mut self, address: [u8; 6]) {
            self.policy = Some(address);
        }
    }

    impl CcmpKeyHardware for Hardware {
        fn install_sta_ccmp_entry(&mut self, index: u8, _words: &[u32; 6]) -> MacKeyInstallOutcome {
            self.installed.push(index);
            MacKeyInstallOutcome::Installed
        }

        fn clear_ccmp_entry(&mut self, index: u8) {
            self.cleared.push(index);
        }
    }

    impl ApTsfHardware for Hardware {
        fn reset_and_start_access_point_tsf(&mut self) {
            self.tsf_started = true;
        }

        fn stop_access_point_tsf(&mut self) {
            self.tsf_stopped = true;
        }
    }

    fn service(
        ap: [u8; 6],
        storage: &mut open_esp_radio_wifi_ap::AccessPointPeerStorage,
    ) -> AccessPointService<'_> {
        AccessPointService::new(
            ap,
            Pmk::derive(b"password", b"ap").unwrap(),
            Wpa2Gtk::new(1, true, [0x55; 16]).unwrap(),
            open_esp_radio_wifi_ap::AccessPointClientLimit::new(2).unwrap(),
            open_esp_radio_wifi_ap::AccessPointInactiveTimeout::default(),
            storage,
        )
    }

    #[test]
    fn active_epoch_owns_policy_group_key_management_and_stop_frontier() {
        let ap = [2, 0, 0, 0, 0, 1];
        let peer = [2, 0, 0, 0, 0, 2];
        let mut beacon = [0; WPA2_BEACON_CAPACITY];
        let mut peers = open_esp_radio_wifi_ap::AccessPointPeerStorage::new();
        let mut pairwise = Esp32s31ApPairwiseKeyStorage::new();
        let ssid = WifiSsid::new(b"ap").unwrap();
        let mut hardware = Hardware::default();
        let mut engine = Esp32s31ApEngine::start(
            &mut hardware,
            service(ap, &mut peers),
            &mut beacon,
            &mut pairwise,
            &ssid,
            WifiChannel::mhz20(6).unwrap(),
            100,
            2,
        )
        .unwrap_or_else(|_| panic!("AP start"));

        let mut request = [0; 30];
        request[..2].copy_from_slice(&0x00b0_u16.to_le_bytes());
        request[4..10].copy_from_slice(&ap);
        request[10..16].copy_from_slice(&peer);
        request[16..22].copy_from_slice(&ap);
        request[26..28].copy_from_slice(&1_u16.to_le_bytes());
        let mut response = [0; 160];
        assert_eq!(
            engine
                .handle_management(&mut hardware, &request, [1; 32], 7, 1, &mut response)
                .unwrap(),
            Esp32s31ApManagementOutcome::Response {
                len: 30,
                begin_wpa2: false
            }
        );
        assert!(engine.prepare_beacon(102_400).is_some());

        let observation = engine.observation();
        let _stopped = engine.stop(&mut hardware);
        assert_eq!(hardware.policy, Some(ap));
        assert_eq!(hardware.installed, [2]);
        assert_eq!(hardware.cleared, [2]);
        assert!(hardware.tsf_started);
        assert!(hardware.tsf_stopped);
        assert_eq!(
            observation,
            Esp32s31ApEngineObservation {
                beacons_prepared: 1,
                authentication_responses_prepared: 1,
                ..Esp32s31ApEngineObservation::default()
            }
        );
    }

    #[test]
    fn associated_peer_stop_emits_vendor_ordered_disconnects_before_removal() {
        const RSN: [u8; 22] = [
            0x30, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0, 0,
        ];
        let ap = [2, 0, 0, 0, 0, 1];
        let peer = [2, 0, 0, 0, 0, 2];
        let mut beacon = [0; WPA2_BEACON_CAPACITY];
        let mut peers = open_esp_radio_wifi_ap::AccessPointPeerStorage::new();
        let mut pairwise = Esp32s31ApPairwiseKeyStorage::new();
        let ssid = WifiSsid::new(b"ap").unwrap();
        let mut hardware = Hardware::default();
        let mut engine = Esp32s31ApEngine::start(
            &mut hardware,
            service(ap, &mut peers),
            &mut beacon,
            &mut pairwise,
            &ssid,
            WifiChannel::mhz20(6).unwrap(),
            100,
            2,
        )
        .unwrap_or_else(|_| panic!("AP start"));

        let mut authentication = [0; 30];
        authentication[..2].copy_from_slice(&0x00b0_u16.to_le_bytes());
        authentication[4..10].copy_from_slice(&ap);
        authentication[10..16].copy_from_slice(&peer);
        authentication[16..22].copy_from_slice(&ap);
        authentication[26..28].copy_from_slice(&1_u16.to_le_bytes());
        let mut output = [0; 160];
        engine
            .handle_management(&mut hardware, &authentication, [7; 32], 9, 1, &mut output)
            .unwrap();

        let mut association = [0; 56];
        association[24..26].copy_from_slice(&0x0010_u16.to_le_bytes());
        association[4..10].copy_from_slice(&ap);
        association[10..16].copy_from_slice(&peer);
        association[16..22].copy_from_slice(&ap);
        association[28..34].copy_from_slice(&[1, 4, 12, 24, 48, 108]);
        association[34..].copy_from_slice(&RSN);
        engine
            .handle_management(&mut hardware, &association, [7; 32], 9, 2, &mut output)
            .unwrap();
        assert!(matches!(
            engine
                .handle_management(&mut hardware, &association, [8; 32], 10, 3, &mut output)
                .unwrap(),
            Esp32s31ApManagementOutcome::Response {
                begin_wpa2: false,
                ..
            }
        ));
        assert_eq!(
            engine.service.peer_status(peer).unwrap().phase,
            ApPeerPhase::Securing,
            "a duplicate association must not replace the in-flight WPA2 owner"
        );
        let mut retried_authentication = authentication;
        retried_authentication[1] |= 0x08;
        assert!(matches!(
            engine
                .handle_management(
                    &mut hardware,
                    &retried_authentication,
                    [9; 32],
                    11,
                    4,
                    &mut output,
                )
                .unwrap(),
            Esp32s31ApManagementOutcome::Response {
                begin_wpa2: false,
                ..
            }
        ));
        assert_eq!(
            engine.service.peer_status(peer).unwrap().phase,
            ApPeerPhase::Securing,
            "an authentication retry must not erase the in-flight WPA2 owner"
        );

        let close = engine.begin_stop_peer().expect("associated peer to close");
        assert!(close.was_associated);

        let mut peer_deauthentication = [0; 26];
        peer_deauthentication[..2].copy_from_slice(&0x00c0_u16.to_le_bytes());
        peer_deauthentication[4..10].copy_from_slice(&ap);
        peer_deauthentication[10..16].copy_from_slice(&peer);
        peer_deauthentication[16..22].copy_from_slice(&ap);
        peer_deauthentication[24..26].copy_from_slice(&3_u16.to_le_bytes());
        assert_eq!(
            engine
                .handle_management(
                    &mut hardware,
                    &peer_deauthentication,
                    [9; 32],
                    10,
                    3,
                    &mut output,
                )
                .unwrap(),
            Esp32s31ApManagementOutcome::Ignored
        );
        assert_eq!(
            engine.service.peer_status(peer).unwrap().phase,
            ApPeerPhase::Closing
        );

        let disassociation = engine
            .encode_peer_disconnect(close, ApPeerDisconnectKind::Disassociation, 2, &mut output)
            .unwrap();
        assert_eq!(
            disassociation,
            open_esp_radio_ieee80211::ap::AP_PEER_DISCONNECT_LEN
        );
        assert_eq!(&output[..2], &0x00a0_u16.to_le_bytes());
        assert_eq!(&output[24..26], &2_u16.to_le_bytes());

        let deauthentication = engine
            .encode_peer_disconnect(
                close,
                ApPeerDisconnectKind::Deauthentication,
                2,
                &mut output,
            )
            .unwrap();
        assert_eq!(
            deauthentication,
            open_esp_radio_ieee80211::ap::AP_PEER_DISCONNECT_LEN
        );
        assert_eq!(&output[..2], &0x00c0_u16.to_le_bytes());
        assert_eq!(&output[24..26], &2_u16.to_le_bytes());

        engine.complete_peer_close(&mut hardware, close).unwrap();
        assert!(engine.service_status().peers.iter().all(Option::is_none));
        assert_eq!(engine.observation().peer_removals, 1);
        assert_eq!(engine.observation().disassociations_prepared, 1);
        assert_eq!(engine.observation().deauthentications_prepared, 1);
        let _ = engine.stop(&mut hardware);
    }

    #[test]
    fn message_four_installs_pairwise_key_before_authorization_is_reported() {
        const RSN: [u8; 22] = [
            0x30, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0, 0,
        ];
        const ANONCE: [u8; 32] = [7; 32];
        const SNONCE: [u8; 32] = [8; 32];
        let ap = [2, 0, 0, 0, 0, 1];
        let peer = [2, 0, 0, 0, 0, 2];
        let mut beacon = [0; WPA2_BEACON_CAPACITY];
        let mut peers = open_esp_radio_wifi_ap::AccessPointPeerStorage::new();
        let mut pairwise = Esp32s31ApPairwiseKeyStorage::new();
        let ssid = WifiSsid::new(b"ap").unwrap();
        let mut hardware = Hardware::default();
        let mut engine = Esp32s31ApEngine::start(
            &mut hardware,
            service(ap, &mut peers),
            &mut beacon,
            &mut pairwise,
            &ssid,
            WifiChannel::mhz20(6).unwrap(),
            100,
            2,
        )
        .unwrap_or_else(|_| panic!("AP start"));

        let mut authentication = [0; 30];
        authentication[..2].copy_from_slice(&0x00b0_u16.to_le_bytes());
        authentication[4..10].copy_from_slice(&ap);
        authentication[10..16].copy_from_slice(&peer);
        authentication[16..22].copy_from_slice(&ap);
        authentication[26..28].copy_from_slice(&1_u16.to_le_bytes());
        let mut response = [0; 160];
        engine
            .handle_management(&mut hardware, &authentication, ANONCE, 9, 1, &mut response)
            .unwrap();

        let mut association = [0; 56];
        association[24..26].copy_from_slice(&0x0010_u16.to_le_bytes());
        association[4..10].copy_from_slice(&ap);
        association[10..16].copy_from_slice(&peer);
        association[16..22].copy_from_slice(&ap);
        association[28..34].copy_from_slice(&[1, 4, 12, 24, 48, 108]);
        association[34..].copy_from_slice(&RSN);
        assert!(matches!(
            engine
                .handle_management(&mut hardware, &association, ANONCE, 9, 2, &mut response)
                .unwrap(),
            Esp32s31ApManagementOutcome::Response {
                begin_wpa2: true,
                ..
            }
        ));
        engine.begin_wpa2::<512>(peer).unwrap();

        let ptk = Pmk::derive(b"password", b"ap")
            .unwrap()
            .derive_ptk(PtkContext {
                authenticator_address: ap,
                supplicant_address: peer,
                authenticator_nonce: ANONCE,
                supplicant_nonce: SNONCE,
            });
        let rsn = OwnedRsnIe::<64>::try_copy(&RSN).unwrap();
        let message2 = Wpa2TxFrame::<512>::message2(ap, 9, SNONCE, &rsn)
            .unwrap()
            .authenticate(&ptk);
        let message2 =
            OwnedEapolFrame::<512>::try_copy(Wpa2Interface::AccessPoint, peer, message2.as_bytes())
                .unwrap();
        let Esp32s31ApWpa2Outcome::Transmit(message3) = engine
            .handle_eapol(&mut hardware, peer, message2, 3)
            .unwrap()
        else {
            panic!("message two must produce message three");
        };
        let mut message3_mpdu = [0; 768];
        let message3_len = engine
            .encode_eapol(peer, &message3, &mut message3_mpdu)
            .unwrap();
        assert!(message3_len > message3.as_bytes().len());
        assert_eq!(&message3_mpdu[4..10], &peer);
        assert_eq!(&message3_mpdu[10..16], &ap);
        assert_eq!(&message3_mpdu[22..24], &[0, 0]);
        assert_eq!(&message3_mpdu[30..32], &[0x88, 0x8e]);
        let message4 = Wpa2TxFrame::<512>::message4(ap, 10)
            .unwrap()
            .authenticate(&ptk);
        let message4 =
            OwnedEapolFrame::<512>::try_copy(Wpa2Interface::AccessPoint, peer, message4.as_bytes())
                .unwrap();
        assert!(matches!(
            engine
                .handle_eapol(&mut hardware, peer, message4, 4)
                .unwrap(),
            Esp32s31ApWpa2Outcome::PeerAuthorized { peer: authorized } if authorized == peer
        ));
        assert_eq!(engine.observation().authorized_peers, 1);

        let rx_pn3 = CcmpPacketNumber::new(3).unwrap();
        let rx_request = Esp32s31ApRxAdmissionRequest::new(
            peer,
            CcmpReplayLane::NonQos,
            Some(CcmpHeader::new(rx_pn3, CcmpKeyId::PAIRWISE)),
        );
        let duplicate_owner = Esp32s31ApRxDuplicateOwner::new(
            engine.service.peer_status(peer).unwrap().association_id,
            engine.service.peer_status(peer).unwrap().association_epoch,
        )
        .unwrap();
        assert_eq!(
            engine.admit_rx_data(rx_request),
            Esp32s31ApRxAdmission::authorized(duplicate_owner)
        );
        assert_eq!(
            engine.admit_rx_data(rx_request),
            Esp32s31ApRxAdmission::rejected(Esp32s31ApRxError::Replay(CcmpReplayError::Replayed {
                packet_number: rx_pn3,
                highest: rx_pn3,
            })),
        );

        let repeated_message4 = Wpa2TxFrame::<512>::message4(ap, 10)
            .unwrap()
            .authenticate(&ptk);
        let repeated_message4 = OwnedEapolFrame::<512>::try_copy(
            Wpa2Interface::AccessPoint,
            peer,
            repeated_message4.as_bytes(),
        )
        .unwrap();
        assert!(matches!(
            engine
                .handle_eapol(&mut hardware, peer, repeated_message4, 5)
                .unwrap(),
            Esp32s31ApWpa2Outcome::None
        ));
        assert_eq!(engine.observation().authorized_peers, 1);

        let mut ethernet = [0_u8; 18];
        ethernet[..6].copy_from_slice(&peer);
        ethernet[6..12].copy_from_slice(&ap);
        ethernet[12..14].copy_from_slice(&0x0800_u16.to_be_bytes());
        ethernet[14..].copy_from_slice(&[1, 2, 3, 4]);
        let mut protected = [0_u8; 96];
        let encoded = engine
            .encode_protected_ethernet(peer, &ethernet, &mut protected)
            .unwrap();
        assert_eq!(encoded.hardware_key_selector, Some(8));
        assert_eq!(&protected[..2], &0x4208_u16.to_le_bytes());
        assert_eq!(&protected[22..24], &0x0010_u16.to_le_bytes());
        assert_eq!(&protected[24..32], &[3, 0, 0, 0x20, 0, 0, 0, 0]);
        assert_eq!(&protected[40..44], &[1, 2, 3, 4]);
        assert_eq!(engine.service.current_data_sequence(), 2);

        ethernet[..6].fill(0xff);
        let encoded = engine
            .encode_protected_ethernet([0xff; 6], &ethernet, &mut protected)
            .unwrap();
        assert_eq!(encoded.hardware_key_selector, Some(2));
        assert_eq!(&protected[24..32], &[3, 0, 0, 0x60, 0, 0, 0, 0]);
        assert_eq!(engine.service.current_data_sequence(), 3);

        // Supplicants may restart authentication without a preceding
        // deauthentication. The old PTK must leave hardware before the same
        // AID begins a new handshake.
        engine
            .handle_management(&mut hardware, &authentication, ANONCE, 11, 5, &mut response)
            .unwrap();
        assert_eq!(hardware.cleared, [8]);
        assert!(!engine.is_authorized_peer(peer));
        assert_eq!(
            engine.admit_rx_data(rx_request),
            Esp32s31ApRxAdmission::unauthorized(),
            "reauthentication closes the controlled port before old-PN admission"
        );
        assert_eq!(engine.service.peer_status(peer).unwrap().association_id, 1);

        let _stopped = engine.stop(&mut hardware);
        assert_eq!(hardware.installed, [2, 8]);
        assert_eq!(hardware.cleared, [8, 2]);
    }
}
