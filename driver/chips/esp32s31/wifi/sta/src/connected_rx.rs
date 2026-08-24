//! Connected-station receive dispatch without an executor or platform policy.
//!
//! This module owns the frame-classification and protocol-routing state that
//! used to be duplicated by the ordinary and TX-interleaved HIL receive loops.
//! It deliberately does not log, access unrelated peripherals, enqueue into a
//! network stack or await an executor primitive. Those effects are published
//! through [`ConnectedRxSink`] and belong to the integration runner.

use core::cell::RefCell;

use critical_section::Mutex;
use open_esp_radio_esp32s31_wifi::protected_data_rx::{
    ProtectedDataFragmentRxError, UnprotectedDataFragmentRxError, view_protected_data,
    view_protected_data_fragment, view_unprotected_data, view_unprotected_data_fragment,
};
use open_esp_radio_esp32s31_wifi_dma::rx_ring::RxSegment;
use open_esp_radio_ieee80211::{
    ccmp::{
        CcmpHeader, CcmpKeyId, CcmpReplayError, CcmpReplayLane, CcmpRxReplayCandidate,
        CcmpRxReplayState,
    },
    data::{DataDecapError, DataInterfaceRole, EthernetFrameParts, RxDuplicateFilter},
    esp_now::{
        ESP_NOW_ACTION_CATEGORY, ESP_NOW_ORGANIZATION_IDENTIFIER, EspNowVersionError,
        EspNowWireVersion, esp_now_wire_version,
    },
    fragmentation::{
        OPEN_DATA_FRAGMENT_TIMEOUT_MICROS, OPEN_DATA_REASSEMBLY_CAPACITY, OpenDataDefragmentation,
        OpenDataDefragmenter, OpenDataFragmentError, OpenDataFragmentPreflight,
        OpenDataUnfragmentedAdmission, parse_ccmp_data_identity, parse_open_data_identity,
    },
    ndpa::{HeNdpa, HeNdpaError},
    security::WifiSecurityMode,
    station::{StaDisconnect, parse_sta_disconnect},
    station_beacon::{StaBeaconError, StaBeaconObservation, parse_sta_beacon},
    trigger::{TriggerCommonInfo, TriggerParseError, parse_trigger_frame},
    twt::{
        IndividualTwtAction, TwtWireError, is_individual_twt_action_candidate,
        parse_individual_twt_action,
    },
};
use open_esp_radio_wifi_softmac::{
    EspNowPeerId, EspNowReceiveError, EspNowReceivedV1, EspNowReceivedV2, EspNowRxEpoch,
    EspNowRxOutcome, EspNowV2ReceiveError, EspNowV2RxOutcome, MacRxMetadata,
};
#[cfg(test)]
use open_esp_radio_wifi_softmac::{MacRxCryptoStatus, MacRxEvidence};
use open_esp_radio_wifi_sta::power_save::StaPsPollDelivery;
use open_esp_radio_wpa2::{EapolKeyFrame, EapolParseError};

use open_esp_radio_esp32s31_wifi_mac::{
    rx::{
        PUBLIC_HEADER_SIZE, RxError, RxIngressConfig, RxPhyInfo, decode_normalized_rx_metadata,
        extract_control, extract_management,
    },
    rx_ampdu::{RxBlockAckMpduKey, rx_block_ack_mpdu_key},
    tx::{HeTriggerScheduledRate, HeTriggerScheduledRateError},
    tx_ampdu::{BlockAckAction, parse_block_ack_action},
};

use open_esp_radio_esp32s31_wifi::esp_now::normalize_esp_now_rx_metadata;

const TRIGGER_FRAME_CONTROL: u16 = 0x0024;
const NDPA_FRAME_CONTROL: u16 = 0x0054;
const ACTION_FRAME_CONTROL: u16 = 0x00d0;
const PROBE_RESPONSE_FRAME_CONTROL: u16 = 0x0050;
const BEACON_FRAME_CONTROL: u16 = 0x0080;
const DISASSOCIATION_FRAME_CONTROL: u16 = 0x00a0;
const DEAUTHENTICATION_FRAME_CONTROL: u16 = 0x00c0;
const DATA_TYPE_MASK: u16 = 0x000c;
const DATA_TYPE: u16 = 0x0008;
const PROTECTED: u16 = 0x4000;
const QOS_SUBTYPE: u16 = 0x0080;
const TO_FROM_DS: u16 = 0x0300;
const MORE_DATA: u16 = 0x2000;
const MORE_FRAGMENTS: u16 = 0x0400;
const QOS_AMSDU_PRESENT: u8 = 0x80;
const OPEN_FRAGMENT_CONTEXTS: usize = 2;

/// Immutable identity and descriptor-ingress policy for one connected STA.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConnectedRxConfig {
    pub station_address: [u8; 6],
    pub bssid: [u8; 6],
    pub association_id: u16,
    pub ingress: RxIngressConfig,
    pub security: WifiSecurityMode,
    /// Peer-negotiated receive geometry. Open TX may deliberately remain
    /// non-QoS while an HT/WMM AP legitimately sends plaintext QoS Data.
    pub peer_qos: bool,
}

/// Destination class observed before protected-data extraction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectedRxProtection {
    Pairwise,
    Group,
    Other,
}

/// Association-scoped software anti-replay owner for the installed PTK and
/// GTK. It is moved into the one connected RX dispatcher and is never cloned.
/// BlockAck release happens before dispatcher entry, so each per-TID frontier
/// observes sequence-ordered frames even when DMA completion order differed.
#[derive(Debug, Eq, PartialEq)]
pub struct StaCcmpRxReplayEpoch {
    pairwise: CcmpRxReplayState,
    group: CcmpRxReplayState,
    group_key_id: CcmpKeyId,
    group_revision: u32,
}

impl StaCcmpRxReplayEpoch {
    pub fn new(
        pairwise_receive_sequence: [u8; 8],
        group_key_id: u8,
        group_receive_sequence: [u8; 8],
    ) -> Result<Self, StaCcmpRxReplayError> {
        let pairwise = CcmpRxReplayState::from_receive_sequence(pairwise_receive_sequence)
            .map_err(|_| StaCcmpRxReplayError::InvalidPairwiseReceiveSequence)?;
        let group = CcmpRxReplayState::from_receive_sequence(group_receive_sequence)
            .map_err(|_| StaCcmpRxReplayError::InvalidGroupReceiveSequence)?;
        let group_key_id = CcmpKeyId::new(group_key_id)
            .ok_or(StaCcmpRxReplayError::InvalidGroupKeyId(group_key_id))?;
        Ok(Self {
            pairwise,
            group,
            group_key_id,
            group_revision: 0,
        })
    }

    pub const fn group_key_id(&self) -> CcmpKeyId {
        self.group_key_id
    }

    fn merge_group_frontiers(
        &self,
        group: &mut CcmpRxReplayState,
    ) -> Result<(), StaCcmpRxReplayError> {
        for lane in core::iter::once(CcmpReplayLane::NonQos).chain((0..16).map(CcmpReplayLane::Tid))
        {
            let current = self
                .group
                .highest(lane)
                .ok_or(StaCcmpRxReplayError::Replay(CcmpReplayError::InvalidTid))?;
            let incoming = group
                .highest(lane)
                .ok_or(StaCcmpRxReplayError::Replay(CcmpReplayError::InvalidTid))?;
            if current > incoming {
                let candidate = group
                    .prepare(lane, current)
                    .map_err(StaCcmpRxReplayError::Replay)?;
                group
                    .commit(candidate)
                    .map_err(StaCcmpRxReplayError::Replay)?;
            }
        }
        Ok(())
    }

    fn prepare_group_rotation(
        &self,
        group_key_id: u8,
        group_receive_sequence: [u8; 8],
    ) -> Result<StaCcmpGroupReplayReplacement, StaCcmpRxReplayError> {
        let mut group = CcmpRxReplayState::from_receive_sequence(group_receive_sequence)
            .map_err(|_| StaCcmpRxReplayError::InvalidGroupReceiveSequence)?;
        let group_key_id = CcmpKeyId::new(group_key_id)
            .ok_or(StaCcmpRxReplayError::InvalidGroupKeyId(group_key_id))?;
        if group_key_id == self.group_key_id {
            // Reinstalling the identical logical key must not roll any lane
            // back to a lower descriptor RSC. The control owner separately
            // proves temporal-key equality before admitting this same-KeyID
            // path; retaining each lane's maximum therefore preserves every
            // authenticated frontier while allowing a higher RSC to raise
            // all lanes.
            self.merge_group_frontiers(&mut group)?;
        }
        Ok(StaCcmpGroupReplayReplacement {
            expected_key_id: self.group_key_id,
            expected_revision: self.group_revision,
            group,
            group_key_id,
        })
    }

    fn commit_group_rotation(
        &mut self,
        replacement: StaCcmpGroupReplayReplacement,
    ) -> Result<(), StaCcmpRxReplayError> {
        if self.group_key_id != replacement.expected_key_id
            || self.group_revision != replacement.expected_revision
        {
            return Err(StaCcmpRxReplayError::StaleGroupRotation);
        }
        let revision = self
            .group_revision
            .checked_add(1)
            .ok_or(StaCcmpRxReplayError::GroupRevisionExhausted)?;
        self.group = replacement.group;
        self.group_key_id = replacement.group_key_id;
        self.group_revision = revision;
        Ok(())
    }

    fn prepare(
        &self,
        protection: ConnectedRxProtection,
        tid: Option<u8>,
        header: CcmpHeader,
    ) -> Result<StaCcmpRxReplayCandidate, StaCcmpRxReplayError> {
        let lane = match tid {
            Some(tid) => CcmpReplayLane::Tid(tid),
            None => CcmpReplayLane::NonQos,
        };
        match protection {
            ConnectedRxProtection::Pairwise => {
                if header.key_id() != CcmpKeyId::PAIRWISE {
                    return Err(StaCcmpRxReplayError::UnexpectedKeyId {
                        protection,
                        expected: CcmpKeyId::PAIRWISE,
                        observed: header.key_id(),
                    });
                }
                self.pairwise
                    .prepare(lane, header.packet_number())
                    .map(StaCcmpRxReplayCandidate::Pairwise)
                    .map_err(StaCcmpRxReplayError::Replay)
            }
            ConnectedRxProtection::Group => {
                if header.key_id() != self.group_key_id {
                    return Err(StaCcmpRxReplayError::UnexpectedKeyId {
                        protection,
                        expected: self.group_key_id,
                        observed: header.key_id(),
                    });
                }
                self.group
                    .prepare(lane, header.packet_number())
                    .map(StaCcmpRxReplayCandidate::Group)
                    .map_err(StaCcmpRxReplayError::Replay)
            }
            ConnectedRxProtection::Other => Err(StaCcmpRxReplayError::ForeignDestination),
        }
    }

    fn commit(&mut self, candidate: StaCcmpRxReplayCandidate) -> Result<(), StaCcmpRxReplayError> {
        match candidate {
            StaCcmpRxReplayCandidate::Pairwise(candidate) => self
                .pairwise
                .commit(candidate)
                .map_err(StaCcmpRxReplayError::Replay),
            StaCcmpRxReplayCandidate::Group(candidate) => self
                .group
                .commit(candidate)
                .map_err(StaCcmpRxReplayError::Replay),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
struct StaCcmpGroupReplayReplacement {
    expected_key_id: CcmpKeyId,
    expected_revision: u32,
    group: CcmpRxReplayState,
    group_key_id: CcmpKeyId,
}

/// Static association replay arena shared by the finite connected-control and
/// RX protocol owners.
///
/// The endpoints returned by [`Self::start`] are generation-bound and
/// non-copyable. Group-key replacement first quarantines group publication;
/// pairwise replay remains live. A group RX permit keeps an in-flight
/// publication visible until the synchronous sink callback has returned.
pub struct StaCcmpRxReplayResource {
    state: Mutex<RefCell<StaCcmpRxReplayResourceState>>,
}

struct StaCcmpRxReplayResourceState {
    generation: u32,
    next_rotation_ticket: u32,
    replay: Option<StaCcmpRxReplayEpoch>,
    pending_rotation: Option<u32>,
    group_publications: u32,
    rx_active: bool,
    control_active: bool,
    group_quarantined: bool,
}

impl StaCcmpRxReplayResource {
    pub const fn new() -> Self {
        Self {
            state: Mutex::new(RefCell::new(StaCcmpRxReplayResourceState {
                generation: 0,
                next_rotation_ticket: 1,
                replay: None,
                pending_rotation: None,
                group_publications: 0,
                rx_active: false,
                control_active: false,
                group_quarantined: false,
            })),
        }
    }

    /// Start one disjoint association epoch in statically located storage.
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc failure must return the exact affine replay epoch for hardware teardown"
    )]
    pub fn start(
        &'static self,
        replay: StaCcmpRxReplayEpoch,
    ) -> Result<
        (StaCcmpRxReplayRxEndpoint, StaCcmpRxReplayControlEndpoint),
        StaCcmpRxReplayStartFailure,
    > {
        critical_section::with(|cs| {
            let mut state = self.state.borrow(cs).borrow_mut();
            if state.replay.is_some() || state.rx_active || state.control_active {
                return Err(StaCcmpRxReplayStartFailure {
                    error: StaCcmpRxReplayStartError::Busy,
                    replay,
                });
            }
            let Some(generation) = state.generation.checked_add(1) else {
                return Err(StaCcmpRxReplayStartFailure {
                    error: StaCcmpRxReplayStartError::GenerationExhausted,
                    replay,
                });
            };
            state.generation = generation;
            state.replay = Some(replay);
            state.pending_rotation = None;
            state.group_publications = 0;
            state.rx_active = true;
            state.control_active = true;
            state.group_quarantined = false;
            let generation = state.generation;
            Ok((
                StaCcmpRxReplayRxEndpoint {
                    resource: self,
                    generation,
                    stopped: false,
                },
                StaCcmpRxReplayControlEndpoint {
                    resource: self,
                    generation,
                    stopped: false,
                },
            ))
        })
    }

    fn release_if_stopped(state: &mut StaCcmpRxReplayResourceState) {
        if !state.rx_active && !state.control_active && state.group_publications == 0 {
            state.replay = None;
            state.pending_rotation = None;
            state.group_quarantined = false;
        }
    }
}

impl Default for StaCcmpRxReplayResource {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaCcmpRxReplayStartError {
    Busy,
    GenerationExhausted,
}

/// Owner-preserving failure to publish an association replay epoch.
///
/// Hardware key teardown still needs the exact replay owner on every rejected
/// start edge, so the input is never consumed into a bare status code.
#[derive(Debug, Eq, PartialEq)]
pub struct StaCcmpRxReplayStartFailure {
    pub error: StaCcmpRxReplayStartError,
    replay: StaCcmpRxReplayEpoch,
}

impl StaCcmpRxReplayStartFailure {
    pub fn into_parts(self) -> (StaCcmpRxReplayStartError, StaCcmpRxReplayEpoch) {
        (self.error, self.replay)
    }
}

/// RX half of one shared replay epoch. It is moved into exactly one connected
/// dispatcher and explicitly stopped after that protocol task is quiescent.
pub struct StaCcmpRxReplayRxEndpoint {
    resource: &'static StaCcmpRxReplayResource,
    generation: u32,
    stopped: bool,
}

impl core::fmt::Debug for StaCcmpRxReplayRxEndpoint {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("StaCcmpRxReplayRxEndpoint")
            .field("generation", &self.generation)
            .field("stopped", &self.stopped)
            .finish_non_exhaustive()
    }
}

impl PartialEq for StaCcmpRxReplayRxEndpoint {
    fn eq(&self, other: &Self) -> bool {
        core::ptr::eq(self.resource, other.resource)
            && self.generation == other.generation
            && self.stopped == other.stopped
    }
}

impl Eq for StaCcmpRxReplayRxEndpoint {}

impl StaCcmpRxReplayRxEndpoint {
    fn prepare_candidate(
        &self,
        protection: ConnectedRxProtection,
        tid: Option<u8>,
        header: CcmpHeader,
    ) -> Result<StaCcmpPreparedRxPublication, StaCcmpRxReplayError> {
        critical_section::with(|cs| {
            let mut state = self.resource.state.borrow(cs).borrow_mut();
            if state.generation != self.generation || !state.rx_active || self.stopped {
                return Err(StaCcmpRxReplayError::StaleEpoch);
            }
            if protection == ConnectedRxProtection::Group
                && (state.group_quarantined || state.pending_rotation.is_some())
            {
                return Err(StaCcmpRxReplayError::GroupRotationInProgress);
            }
            let replay = state
                .replay
                .as_ref()
                .ok_or(StaCcmpRxReplayError::OwnerUnavailable)?;
            let candidate = replay.prepare(protection, tid, header)?;
            let group = protection == ConnectedRxProtection::Group;
            if group {
                state.group_publications = state
                    .group_publications
                    .checked_add(1)
                    .ok_or(StaCcmpRxReplayError::PublicationCountExhausted)?;
            }
            Ok(StaCcmpPreparedRxPublication {
                resource: self.resource,
                generation: self.generation,
                candidate,
                group,
                group_gate_armed: group,
            })
        })
    }

    #[cfg(test)]
    fn prepare_publication(
        &self,
        protection: ConnectedRxProtection,
        tid: Option<u8>,
        header: CcmpHeader,
    ) -> Result<StaCcmpRxPublicationPermit, StaCcmpRxReplayError> {
        self.prepare_candidate(protection, tid, header)?.commit()
    }

    pub fn stop(&mut self) -> Result<(), StaCcmpRxReplayError> {
        let result = critical_section::with(|cs| {
            let mut state = self.resource.state.borrow(cs).borrow_mut();
            if state.generation != self.generation || !state.rx_active {
                return Err(StaCcmpRxReplayError::StaleEpoch);
            }
            if state.group_publications != 0 {
                state.group_quarantined = true;
                return Err(StaCcmpRxReplayError::PublicationInFlight);
            }
            state.rx_active = false;
            state.group_quarantined = true;
            StaCcmpRxReplayResource::release_if_stopped(&mut state);
            Ok(())
        });
        if result.is_ok() {
            self.stopped = true;
        }
        result
    }
}

impl Drop for StaCcmpRxReplayRxEndpoint {
    fn drop(&mut self) {
        if self.stopped {
            return;
        }
        critical_section::with(|cs| {
            let mut state = self.resource.state.borrow(cs).borrow_mut();
            if state.generation == self.generation {
                state.rx_active = false;
                state.group_quarantined = true;
                StaCcmpRxReplayResource::release_if_stopped(&mut state);
            }
        });
    }
}

/// Control half of one shared replay epoch.
pub struct StaCcmpRxReplayControlEndpoint {
    resource: &'static StaCcmpRxReplayResource,
    generation: u32,
    stopped: bool,
}

impl StaCcmpRxReplayControlEndpoint {
    pub fn prepare_group_rotation(
        &self,
        group_key_id: u8,
        group_receive_sequence: [u8; 8],
    ) -> Result<StaCcmpPreparedGroupRotation, StaCcmpRxReplayError> {
        critical_section::with(|cs| {
            let mut state = self.resource.state.borrow(cs).borrow_mut();
            if state.generation != self.generation || !state.control_active || self.stopped {
                return Err(StaCcmpRxReplayError::StaleEpoch);
            }
            if !state.rx_active {
                return Err(StaCcmpRxReplayError::RxStopped);
            }
            if state.group_quarantined || state.pending_rotation.is_some() {
                return Err(StaCcmpRxReplayError::GroupRotationInProgress);
            }
            let replacement = state
                .replay
                .as_ref()
                .ok_or(StaCcmpRxReplayError::OwnerUnavailable)?
                .prepare_group_rotation(group_key_id, group_receive_sequence)?;
            let ticket = state.next_rotation_ticket;
            state.next_rotation_ticket = match state.next_rotation_ticket.checked_add(1) {
                Some(next) => next,
                None => {
                    state.group_quarantined = true;
                    return Err(StaCcmpRxReplayError::GroupRotationTicketExhausted);
                }
            };
            Ok(StaCcmpPreparedGroupRotation {
                generation: self.generation,
                ticket,
                replacement,
            })
        })
    }

    pub fn begin_group_rotation(
        &self,
        mut prepared: StaCcmpPreparedGroupRotation,
    ) -> Result<StaCcmpInstallingGroupRotation, StaCcmpRxReplayError> {
        critical_section::with(|cs| {
            let mut state = self.resource.state.borrow(cs).borrow_mut();
            if prepared.generation != self.generation
                || state.generation != self.generation
                || !state.control_active
            {
                return Err(StaCcmpRxReplayError::StaleGroupRotation);
            }
            if !state.rx_active {
                return Err(StaCcmpRxReplayError::RxStopped);
            }
            if state.group_publications != 0 {
                return Err(StaCcmpRxReplayError::PublicationInFlight);
            }
            if state.group_quarantined || state.pending_rotation.is_some() {
                return Err(StaCcmpRxReplayError::GroupRotationInProgress);
            }
            let replay = state
                .replay
                .as_ref()
                .ok_or(StaCcmpRxReplayError::OwnerUnavailable)?;
            if replay.group_key_id != prepared.replacement.expected_key_id
                || replay.group_revision != prepared.replacement.expected_revision
            {
                return Err(StaCcmpRxReplayError::StaleGroupRotation);
            }
            if replay.group_key_id == prepared.replacement.group_key_id {
                // Group RX may have advanced one or more lane frontiers after
                // prepare returned. Refresh the same-KeyID maximum only after
                // the publication gate is known to be clear, while retaining
                // the prepared revision check above so a completed rotation
                // still makes this candidate stale.
                replay.merge_group_frontiers(&mut prepared.replacement.group)?;
            }
            state.pending_rotation = Some(prepared.ticket);
            Ok(StaCcmpInstallingGroupRotation { prepared })
        })
    }

    pub fn commit_group_rotation(
        &self,
        installing: StaCcmpInstallingGroupRotation,
    ) -> Result<(), StaCcmpRxReplayError> {
        critical_section::with(|cs| {
            let mut state = self.resource.state.borrow(cs).borrow_mut();
            if installing.prepared.generation != self.generation
                || state.generation != self.generation
            {
                return Err(StaCcmpRxReplayError::StaleGroupRotation);
            }
            if state.pending_rotation != Some(installing.prepared.ticket) || !state.control_active {
                state.group_quarantined = true;
                return Err(StaCcmpRxReplayError::StaleGroupRotation);
            }
            if !state.rx_active || state.group_publications != 0 {
                state.group_quarantined = true;
                return Err(if state.rx_active {
                    StaCcmpRxReplayError::PublicationInFlight
                } else {
                    StaCcmpRxReplayError::RxStopped
                });
            }
            state
                .replay
                .as_mut()
                .ok_or(StaCcmpRxReplayError::OwnerUnavailable)?
                .commit_group_rotation(installing.prepared.replacement)?;
            state.pending_rotation = None;
            Ok(())
        })
    }

    /// Abort after hardware restored the exact old GTK. Replay state was not
    /// changed by prepare/begin, so clearing the gate republishes one coherent
    /// old key+frontier epoch.
    pub fn abort_group_rotation(
        &self,
        installing: StaCcmpInstallingGroupRotation,
    ) -> Result<(), StaCcmpRxReplayError> {
        critical_section::with(|cs| {
            let mut state = self.resource.state.borrow(cs).borrow_mut();
            if installing.prepared.generation != self.generation
                || state.generation != self.generation
            {
                return Err(StaCcmpRxReplayError::StaleGroupRotation);
            }
            if state.pending_rotation != Some(installing.prepared.ticket) {
                state.group_quarantined = true;
                return Err(StaCcmpRxReplayError::StaleGroupRotation);
            }
            state.pending_rotation = None;
            Ok(())
        })
    }

    /// Keep group RX permanently closed after rollback could not re-establish
    /// either complete key+replay epoch. Pairwise replay remains intact until
    /// outer disconnect teardown quiesces the association.
    pub fn quarantine_group_rotation(&self, installing: StaCcmpInstallingGroupRotation) {
        critical_section::with(|cs| {
            let mut state = self.resource.state.borrow(cs).borrow_mut();
            if state.generation == self.generation
                && state.pending_rotation == Some(installing.prepared.ticket)
            {
                state.pending_rotation = None;
            }
            if state.generation == self.generation {
                state.group_quarantined = true;
            }
        });
    }

    pub fn stop(&mut self) -> Result<(), StaCcmpRxReplayError> {
        let result = critical_section::with(|cs| {
            let mut state = self.resource.state.borrow(cs).borrow_mut();
            if state.generation != self.generation || !state.control_active {
                return Err(StaCcmpRxReplayError::StaleEpoch);
            }
            state.control_active = false;
            if state.pending_rotation.is_some() {
                state.group_quarantined = true;
            }
            StaCcmpRxReplayResource::release_if_stopped(&mut state);
            Ok(())
        });
        if result.is_ok() {
            self.stopped = true;
        }
        result
    }
}

impl Drop for StaCcmpRxReplayControlEndpoint {
    fn drop(&mut self) {
        if self.stopped {
            return;
        }
        critical_section::with(|cs| {
            let mut state = self.resource.state.borrow(cs).borrow_mut();
            if state.generation == self.generation {
                state.control_active = false;
                state.group_quarantined = true;
                StaCcmpRxReplayResource::release_if_stopped(&mut state);
            }
        });
    }
}

/// Prepared replay candidate. It has no side effect and becomes stale after
/// any other candidate begins.
pub struct StaCcmpPreparedGroupRotation {
    generation: u32,
    ticket: u32,
    replacement: StaCcmpGroupReplayReplacement,
}

/// Group-publication gate held across the hardware replacement transaction.
pub struct StaCcmpInstallingGroupRotation {
    prepared: StaCcmpPreparedGroupRotation,
}

struct StaCcmpRxPublicationPermit {
    resource: &'static StaCcmpRxReplayResource,
    generation: u32,
    group: bool,
}

struct StaCcmpPreparedRxPublication {
    resource: &'static StaCcmpRxReplayResource,
    generation: u32,
    candidate: StaCcmpRxReplayCandidate,
    group: bool,
    group_gate_armed: bool,
}

impl StaCcmpPreparedRxPublication {
    fn commit(mut self) -> Result<StaCcmpRxPublicationPermit, StaCcmpRxReplayError> {
        critical_section::with(|cs| {
            let mut state = self.resource.state.borrow(cs).borrow_mut();
            if state.generation != self.generation || !state.rx_active {
                return Err(StaCcmpRxReplayError::StaleEpoch);
            }
            let replay = state
                .replay
                .as_mut()
                .ok_or(StaCcmpRxReplayError::OwnerUnavailable)?;
            replay.commit(self.candidate)?;
            self.group_gate_armed = false;
            Ok(StaCcmpRxPublicationPermit {
                resource: self.resource,
                generation: self.generation,
                group: self.group,
            })
        })
    }
}

impl Drop for StaCcmpPreparedRxPublication {
    fn drop(&mut self) {
        if !self.group_gate_armed {
            return;
        }
        critical_section::with(|cs| {
            let mut state = self.resource.state.borrow(cs).borrow_mut();
            if state.generation == self.generation {
                state.group_publications = state
                    .group_publications
                    .checked_sub(1)
                    .expect("a prepared group replay candidate decrements exactly once");
                StaCcmpRxReplayResource::release_if_stopped(&mut state);
            }
        });
        self.group_gate_armed = false;
    }
}

impl Drop for StaCcmpRxPublicationPermit {
    fn drop(&mut self) {
        if !self.group {
            return;
        }
        critical_section::with(|cs| {
            let mut state = self.resource.state.borrow(cs).borrow_mut();
            if state.generation == self.generation {
                state.group_publications = state
                    .group_publications
                    .checked_sub(1)
                    .expect("a group publication permit decrements exactly once");
                StaCcmpRxReplayResource::release_if_stopped(&mut state);
            }
        });
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StaCcmpRxReplayCandidate {
    Pairwise(CcmpRxReplayCandidate),
    Group(CcmpRxReplayCandidate),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaCcmpRxReplayError {
    InvalidPairwiseReceiveSequence,
    InvalidGroupReceiveSequence,
    InvalidGroupKeyId(u8),
    OwnerUnavailable,
    StaleEpoch,
    StaleGroupRotation,
    GroupRotationInProgress,
    PublicationInFlight,
    PublicationCountExhausted,
    GroupRotationTicketExhausted,
    GroupRevisionExhausted,
    RxStopped,
    ForeignDestination,
    UnexpectedKeyId {
        protection: ConnectedRxProtection,
        expected: CcmpKeyId,
        observed: CcmpKeyId,
    },
    Replay(CcmpReplayError),
}

/// MAC identity admitted for an associated HE control exchange.
///
/// Construction remains private to [`ConnectedRxDispatcher`]: a published
/// value proves that the transmitter was the associated BSSID and that the
/// receiver was either this station or an IEEE group address.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AssociatedHeControlIdentity {
    duration: u16,
    receiver_address: [u8; 6],
    transmitter_address: [u8; 6],
}

impl AssociatedHeControlIdentity {
    pub const fn duration(self) -> u16 {
        self.duration
    }

    pub const fn receiver_address(self) -> [u8; 6] {
        self.receiver_address
    }

    pub const fn transmitter_address(self) -> [u8; 6] {
        self.transmitter_address
    }
}

/// One semantic event emitted by the connected frame dispatcher.
///
/// Borrowed frame slices remain valid only for the duration of
/// [`ConnectedRxSink::publish`]. A network adapter must copy or otherwise
/// transfer them into storage that it owns before returning.
#[derive(Clone, Copy, Debug)]
pub enum ConnectedRxEvent<'frame> {
    Beacon {
        observation: StaBeaconObservation,
        metadata: MacRxMetadata<RxPhyInfo>,
    },
    ProbeResponse,
    Trigger {
        identity: AssociatedHeControlIdentity,
        common: TriggerCommonInfo,
        schedule: Result<HeTriggerScheduledRate, HeTriggerScheduledRateError>,
        first_user: Option<[u8; 5]>,
        runtime_received_at_micros: Option<u64>,
    },
    Ndpa {
        identity: AssociatedHeControlIdentity,
        dialog_token: u8,
        addressed_to_station: bool,
        runtime_received_at_micros: Option<u64>,
    },
    BlockAck {
        action: BlockAckAction,
        body: &'frame [u8],
    },
    /// Strictly decoded individual TWT Setup or Teardown action from the
    /// associated AP. The fixed action body is copied into the control lane.
    IndividualTwt {
        action: IndividualTwtAction,
        body: &'frame [u8],
    },
    /// Strictly decoded plaintext ESP-NOW datagram from one configured peer.
    ///
    /// `received` borrows the normalized MPDU and must be copied by a sink
    /// which retains it after [`ConnectedRxSink::publish`] returns.
    EspNow {
        received: EspNowReceivedV1<'frame>,
        metadata: MacRxMetadata<RxPhyInfo>,
    },
    /// One plaintext EAPOL-Key packet on an otherwise protected station link.
    ///
    /// This event is constructed only by the narrow connected WPA2 admission
    /// path. Ordinary plaintext Data, A-MSDU, foreign addresses and non-EAPOL
    /// LLC payloads never reach a sink.
    UnprotectedEapol {
        source: [u8; 6],
        payload: &'frame [u8],
    },
    PeerDisconnect(StaDisconnect),
    /// One admitted associated unicast delivery that can complete the
    /// currently armed legacy PS-Poll service transaction. A fragmented Open
    /// MSDU reaches this edge only after complete reassembly.
    PowerSaveDelivery(StaPsPollDelivery),
    Ethernet {
        frame: EthernetFrameParts<'frame>,
        raw: &'frame [u8],
        amsdu: bool,
        metadata: MacRxMetadata<RxPhyInfo>,
    },
}

/// Owned connected-station event that may cross the lifetime of one staged
/// RX frame.
///
/// Ethernet payload is intentionally absent: it must be copied or transferred
/// to the network queue while [`ConnectedRxEvent::Ethernet`] is borrowed.
/// BlockAck carries its complete parsed fixed fields, so no management-frame
/// body or C-style context pointer needs to survive dispatch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectedRxControlEvent {
    Beacon(StaBeaconObservation),
    ProbeResponse,
    Trigger {
        identity: AssociatedHeControlIdentity,
        common: TriggerCommonInfo,
        schedule: Result<HeTriggerScheduledRate, HeTriggerScheduledRateError>,
        first_user: Option<[u8; 5]>,
        runtime_received_at_micros: Option<u64>,
    },
    Ndpa {
        identity: AssociatedHeControlIdentity,
        dialog_token: u8,
        addressed_to_station: bool,
        runtime_received_at_micros: Option<u64>,
    },
    BlockAck(BlockAckAction),
    IndividualTwt(IndividualTwtAction),
    PeerDisconnect(StaDisconnect),
    PowerSaveDelivery(StaPsPollDelivery),
    /// More than one delivery crossed the single outstanding PS-Poll lane.
    /// Connected control must restore PM=0 instead of guessing which poll an
    /// overwritten observation belonged to.
    PowerSaveDeliveryRace,
}

impl ConnectedRxEvent<'_> {
    /// Copy only the protocol/control information that is independent of the
    /// staged RX allocation.
    pub const fn control(self) -> Option<ConnectedRxControlEvent> {
        match self {
            Self::Beacon { observation, .. } => Some(ConnectedRxControlEvent::Beacon(observation)),
            Self::ProbeResponse => Some(ConnectedRxControlEvent::ProbeResponse),
            Self::Trigger {
                identity,
                common,
                schedule,
                first_user,
                runtime_received_at_micros,
            } => Some(ConnectedRxControlEvent::Trigger {
                identity,
                common,
                schedule,
                first_user,
                runtime_received_at_micros,
            }),
            Self::Ndpa {
                identity,
                dialog_token,
                addressed_to_station,
                runtime_received_at_micros,
            } => Some(ConnectedRxControlEvent::Ndpa {
                identity,
                dialog_token,
                addressed_to_station,
                runtime_received_at_micros,
            }),
            Self::BlockAck { action, .. } => Some(ConnectedRxControlEvent::BlockAck(action)),
            Self::IndividualTwt { action, .. } => {
                Some(ConnectedRxControlEvent::IndividualTwt(action))
            }
            Self::EspNow { .. } => None,
            Self::UnprotectedEapol { .. } => None,
            Self::PeerDisconnect(disconnect) => {
                Some(ConnectedRxControlEvent::PeerDisconnect(disconnect))
            }
            Self::PowerSaveDelivery(delivery) => {
                Some(ConnectedRxControlEvent::PowerSaveDelivery(delivery))
            }
            Self::Ethernet { .. } => None,
        }
    }
}

/// Integration boundary for network delivery, diagnostics and PAC effects.
pub trait ConnectedRxSink {
    fn publish(&mut self, event: ConnectedRxEvent<'_>);

    /// Opt in only when the sink copies/reassembles v2 before dispatch returns.
    fn supports_esp_now_v2(&self) -> bool {
        false
    }

    fn publish_esp_now_v2(
        &mut self,
        _received: EspNowReceivedV2<'_>,
        _metadata: MacRxMetadata<RxPhyInfo>,
    ) {
    }
}

/// Closed reason for a frame that reached the connected dispatcher but could
/// not produce a semantic event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectedRxError {
    PublicHeader,
    Rx(RxError),
    Trigger(TriggerParseError),
    Ndpa(HeNdpaError),
    Beacon(StaBeaconError),
    IndividualTwt(TwtWireError),
    EspNow(EspNowReceiveError),
    EspNowV2(EspNowV2ReceiveError),
    EspNowVersion(EspNowVersionError),
    EspNowV2SinkUnavailable,
    Data(DataDecapError),
    Eapol(EapolParseError),
    SecurityModeMismatch,
    PeerQosMismatch,
    CcmpReplay(StaCcmpRxReplayError),
    Fragment(OpenDataFragmentError),
}

/// Result of consuming one independently owned staged RX frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectedRxDispatch {
    Beacon,
    ProbeResponse,
    Trigger,
    Ndpa,
    BlockAck,
    IndividualTwt,
    EspNow {
        peer: EspNowPeerId,
    },
    EspNowDuplicate {
        peer: EspNowPeerId,
    },
    EspNowV2 {
        peer: EspNowPeerId,
    },
    EspNowV2Duplicate {
        peer: EspNowPeerId,
    },
    PeerDisconnect,
    UnprotectedEapol,
    Data {
        ethernet_frames: u8,
        amsdu: bool,
    },
    FragmentBuffered {
        expired: u8,
        evicted: bool,
    },
    Duplicate,
    Ignored,
    Rejected {
        protection: ConnectedRxProtection,
        error: ConnectedRxError,
    },
}

/// Unique protocol-routing state for one connected station.
pub struct ConnectedRxDispatcher {
    config: ConnectedRxConfig,
    duplicate_filter: RxDuplicateFilter,
    ccmp_replay: Option<StaCcmpRxReplayOwner>,
    esp_now: Option<EspNowRxEpoch>,
    fragments: OpenDataDefragmenter<OPEN_FRAGMENT_CONTEXTS, OPEN_DATA_REASSEMBLY_CAPACITY>,
}

#[expect(
    clippy::large_enum_variant,
    reason = "the no-alloc dispatcher supports both the legacy owned epoch and the production shared endpoint without boxing replay state"
)]
enum StaCcmpRxReplayOwner {
    Owned(StaCcmpRxReplayEpoch),
    Shared(StaCcmpRxReplayRxEndpoint),
}

enum StaCcmpPreparedReplay {
    Owned(StaCcmpRxReplayCandidate),
    Shared(StaCcmpPreparedRxPublication),
}

fn prepare_ccmp_replay(
    replay: &mut Option<StaCcmpRxReplayOwner>,
    protection: ConnectedRxProtection,
    tid: Option<u8>,
    header: CcmpHeader,
) -> Result<StaCcmpPreparedReplay, StaCcmpRxReplayError> {
    match replay
        .as_mut()
        .ok_or(StaCcmpRxReplayError::OwnerUnavailable)?
    {
        StaCcmpRxReplayOwner::Owned(replay) => replay
            .prepare(protection, tid, header)
            .map(StaCcmpPreparedReplay::Owned),
        StaCcmpRxReplayOwner::Shared(replay) => replay
            .prepare_candidate(protection, tid, header)
            .map(StaCcmpPreparedReplay::Shared),
    }
}

fn commit_ccmp_replay(
    replay: &mut Option<StaCcmpRxReplayOwner>,
    prepared: StaCcmpPreparedReplay,
) -> Result<Option<StaCcmpRxPublicationPermit>, StaCcmpRxReplayError> {
    match prepared {
        StaCcmpPreparedReplay::Owned(candidate) => {
            let Some(StaCcmpRxReplayOwner::Owned(replay)) = replay.as_mut() else {
                return Err(StaCcmpRxReplayError::StaleEpoch);
            };
            replay.commit(candidate)?;
            Ok(None)
        }
        StaCcmpPreparedReplay::Shared(prepared) => prepared.commit().map(Some),
    }
}

impl ConnectedRxDispatcher {
    /// Empty identity used only while a caller-owned protocol arena is parked.
    ///
    /// Frames must not be dispatched through this value. Call
    /// [`Self::try_reconfigure`] at the connected-epoch start edge before
    /// installing replay or ESP-NOW owners.
    pub const fn unconfigured() -> Self {
        Self::new(ConnectedRxConfig {
            station_address: [0; 6],
            bssid: [0; 6],
            association_id: 0,
            ingress: RxIngressConfig {
                ring_entry_limit: 1,
                csi_config: 0,
                flags: 0,
            },
            security: WifiSecurityMode::Wpa2Personal,
            peer_qos: true,
        })
    }

    pub const fn new(config: ConnectedRxConfig) -> Self {
        Self {
            config,
            duplicate_filter: RxDuplicateFilter::new(),
            ccmp_replay: None,
            esp_now: None,
            fragments: OpenDataDefragmenter::new(OPEN_DATA_FRAGMENT_TIMEOUT_MICROS),
        }
    }

    /// Revoke the previous association and install a new immutable identity.
    ///
    /// A replay endpoint that cannot stop keeps the old identity installed and
    /// returns an error, so callers cannot publish a new connected epoch over
    /// an in-flight CCMP publication. ESP-NOW duplicate history, ordinary
    /// duplicate history and incomplete fragment contexts are revoked on every
    /// attempt, including that fail-closed replay error path.
    pub fn try_reconfigure(
        &mut self,
        config: ConnectedRxConfig,
    ) -> Result<(), StaCcmpRxReplayError> {
        let replay = self.stop_ccmp_rx_replay();
        self.stop_esp_now_rx_epoch();
        self.clear_fragmentation();
        self.clear_duplicate_history();
        replay?;
        self.config = config;
        Ok(())
    }

    /// Install the unique replay epoch created from this association's M3
    /// pairwise/group RSC values. A WPA2 dispatcher without this owner rejects
    /// protected data instead of silently accepting PN reuse.
    pub fn install_ccmp_rx_replay(&mut self, replay: StaCcmpRxReplayEpoch) {
        assert_eq!(
            self.config.security,
            WifiSecurityMode::Wpa2Personal,
            "CCMP replay state requires a WPA2 connected epoch"
        );
        self.ccmp_replay = Some(StaCcmpRxReplayOwner::Owned(replay));
    }

    /// Install the RX half of an association replay resource shared with the
    /// connected group-key control transaction.
    pub fn install_shared_ccmp_rx_replay(&mut self, replay: StaCcmpRxReplayRxEndpoint) {
        assert_eq!(
            self.config.security,
            WifiSecurityMode::Wpa2Personal,
            "CCMP replay state requires a WPA2 connected epoch"
        );
        self.ccmp_replay = Some(StaCcmpRxReplayOwner::Shared(replay));
    }

    pub const fn ccmp_rx_replay_enabled(&self) -> bool {
        self.ccmp_replay.is_some()
    }

    /// Stop the shared RX endpoint after the protocol task is quiescent.
    pub fn stop_ccmp_rx_replay(&mut self) -> Result<(), StaCcmpRxReplayError> {
        let Some(StaCcmpRxReplayOwner::Shared(replay)) = self.ccmp_replay.as_mut() else {
            self.ccmp_replay = None;
            return Ok(());
        };
        replay.stop()?;
        self.ccmp_replay = None;
        Ok(())
    }

    /// Quarantine a replay endpoint after a proven terminal protocol stop.
    ///
    /// This is deliberately separate from [`Self::stop_ccmp_rx_replay`]: the
    /// normal stop API retains a failed endpoint so its caller may retry. A
    /// terminal task that can no longer carry an in-flight publication must
    /// instead drop that endpoint, whose `Drop` implementation revokes RX and
    /// keeps the shared group lane quarantined.
    pub fn quarantine_ccmp_rx_replay(&mut self) -> bool {
        self.ccmp_replay.take().is_some()
    }

    /// Attach one already station/channel-qualified ESP-NOW receive epoch.
    ///
    /// Hardware receive-policy ownership remains at the connected composition
    /// boundary. This method only installs portable peer and duplicate state.
    pub fn install_esp_now_rx_epoch(&mut self, epoch: EspNowRxEpoch) {
        assert_eq!(
            epoch.config().station().interface.address,
            self.config.station_address,
            "ESP-NOW RX epoch must belong to the connected station"
        );
        self.esp_now = Some(epoch);
    }

    pub const fn esp_now_rx_epoch(&self) -> Option<&EspNowRxEpoch> {
        self.esp_now.as_ref()
    }

    /// Revoke ordinary MPDU retry fingerprints at an association stop edge.
    pub fn clear_duplicate_history(&mut self) {
        self.duplicate_filter = RxDuplicateFilter::new();
    }

    /// Revoke receive authority and clear all duplicate fingerprints before
    /// this connected owner is returned across a stop/restart boundary.
    pub fn stop_esp_now_rx_epoch(&mut self) -> usize {
        let Some(mut epoch) = self.esp_now.take() else {
            return 0;
        };
        epoch.reset_duplicate_history()
    }

    /// Revoke incomplete Open and CCMP MSDUs at the connected-epoch stop edge.
    pub fn clear_fragmentation(&mut self) -> usize {
        self.fragments.clear()
    }

    pub fn clear_open_fragmentation(&mut self) -> usize {
        self.clear_fragmentation()
    }

    pub const fn config(&self) -> ConnectedRxConfig {
        self.config
    }

    /// Return whether this staged unit can publish an Ethernet frame.
    ///
    /// This deliberately performs only immutable public-header
    /// classification. It does not advance duplicate history or authenticate
    /// payload bytes, so an async integration may acquire network capacity
    /// before the finite dispatch without changing protocol state.
    pub fn may_publish_ethernet(&self, segment: RxSegment<'_>) -> bool {
        public_frame_control(segment.buffer).is_some_and(|frame_control| {
            let expected = match self.config.security {
                WifiSecurityMode::Open => DATA_TYPE,
                WifiSecurityMode::Wpa2Personal => DATA_TYPE | PROTECTED,
            };
            frame_control & (DATA_TYPE_MASK | PROTECTED) == expected
                && (self.config.peer_qos || frame_control & QOS_SUBTYPE == 0)
                && !public_fragmented(segment.buffer, frame_control).unwrap_or(false)
        })
    }

    /// Return whether this Open or CCMP fragment may complete an in-progress
    /// MSDU.
    ///
    /// The async adapter uses this immutable hint only to reserve copying
    /// capacity. The reassembly owner still performs the exact sequence and
    /// identity admission after the wait.
    pub fn may_complete_fragment(&self, segment: RxSegment<'_>) -> bool {
        let Some(frame_control) = public_frame_control(segment.buffer) else {
            return false;
        };
        let expected = match self.config.security {
            WifiSecurityMode::Open => DATA_TYPE,
            WifiSecurityMode::Wpa2Personal => DATA_TYPE | PROTECTED,
        };
        if frame_control & (DATA_TYPE_MASK | PROTECTED) != expected
            || (frame_control & QOS_SUBTYPE != 0 && !self.config.peer_qos)
            || frame_control & MORE_FRAGMENTS != 0
        {
            return false;
        }
        segment
            .buffer
            .get(PUBLIC_HEADER_SIZE + 22)
            .is_some_and(|sequence| sequence & 0x0f != 0)
    }

    /// Compatibility hint retained for Open-only adapters.
    pub fn may_complete_open_fragment(&self, segment: RxSegment<'_>) -> bool {
        self.config.security == WifiSecurityMode::Open && self.may_complete_fragment(segment)
    }

    /// Return whether a protected QoS data unit advertises A-MSDU payload.
    ///
    /// This immutable public-header check lets an async adapter select its
    /// deferred multi-frame publication path without parsing/decrypting the
    /// MPDU twice or mutating duplicate history. Malformed input may select
    /// the conservative path and then be rejected by [`Self::dispatch`].
    pub fn may_publish_amsdu(&self, segment: RxSegment<'_>) -> bool {
        if !self.config.peer_qos {
            return false;
        }
        let raw = segment.buffer;
        let Some(frame_control) = public_frame_control(raw) else {
            return false;
        };
        let expected = match self.config.security {
            WifiSecurityMode::Open => DATA_TYPE | QOS_SUBTYPE,
            WifiSecurityMode::Wpa2Personal => DATA_TYPE | PROTECTED | QOS_SUBTYPE,
        };
        if frame_control & (DATA_TYPE_MASK | PROTECTED | QOS_SUBTYPE) != expected {
            return false;
        }
        // Four-address QoS moves the control field by one address slot.
        let qos_control_offset =
            PUBLIC_HEADER_SIZE + 24 + usize::from(frame_control & TO_FROM_DS == TO_FROM_DS) * 6;
        raw.get(qos_control_offset)
            .is_some_and(|control| control & QOS_AMSDU_PRESENT != 0)
    }

    /// Classify a frame that belongs to a receive BlockAck sequence space.
    ///
    /// Open epochs own no BlockAck/reorder state. In WPA2, group, foreign,
    /// unprotected, non-QoS and fragmented frames remain on the direct
    /// dispatch path; agreement state still decides whether the returned TID
    /// is currently reordered.
    pub fn reorder_key(&self, segment: RxSegment<'_>) -> Option<RxBlockAckMpduKey> {
        if self.config.security == WifiSecurityMode::Open {
            return None;
        }
        rx_block_ack_mpdu_key(
            segment.buffer,
            self.config.station_address,
            Some(self.config.bssid),
        )
    }

    /// Consume one staged S31 frame and publish its connected-station effects.
    ///
    /// The supplied segment is already detached from DMA ownership. This call
    /// is finite and allocation-free; it performs no MMIO and never waits.
    pub fn dispatch<S: ConnectedRxSink>(
        &mut self,
        segment: RxSegment<'_>,
        mpdu: &mut [u8],
        _ethernet: &mut [u8],
        sink: &mut S,
    ) -> ConnectedRxDispatch {
        self.dispatch_with_runtime_received_at(segment, mpdu, _ethernet, None, sink)
    }

    /// Dispatch with the executor-clock sample attached by the physical RX
    /// producer at its first completed-frame handoff.
    ///
    /// The timestamp is optional because executor-neutral and synthetic users
    /// cannot manufacture it. Runtime Trigger/NDPA response policy rejects a
    /// missing sample rather than starting a fresh window at mailbox dequeue.
    pub fn dispatch_with_runtime_received_at<S: ConnectedRxSink>(
        &mut self,
        segment: RxSegment<'_>,
        mpdu: &mut [u8],
        _ethernet: &mut [u8],
        runtime_received_at_micros: Option<u64>,
        sink: &mut S,
    ) -> ConnectedRxDispatch {
        let raw = segment.buffer;
        let Some(frame_control) = public_frame_control(raw) else {
            return rejected(ConnectedRxProtection::Other, ConnectedRxError::PublicHeader);
        };
        let protection = public_protection(raw, frame_control, self.config.station_address)
            .unwrap_or(ConnectedRxProtection::Other);

        match frame_control & 0x00fc {
            BEACON_FRAME_CONTROL => {
                let management = match extract_management(
                    core::slice::from_ref(&segment),
                    self.config.ingress,
                    mpdu,
                ) {
                    Ok(management) => management,
                    Err(error) => return rejected(protection, ConnectedRxError::Rx(error)),
                };
                let observation = match parse_sta_beacon(
                    &mpdu[..management.length],
                    self.config.bssid,
                    self.config.association_id,
                ) {
                    Ok(observation) => observation,
                    Err(error) => return rejected(protection, ConnectedRxError::Beacon(error)),
                };
                let Some(metadata) = decode_normalized_rx_metadata(raw) else {
                    return rejected(protection, ConnectedRxError::Rx(RxError::Metadata));
                };
                sink.publish(ConnectedRxEvent::Beacon {
                    observation,
                    metadata,
                });
                ConnectedRxDispatch::Beacon
            }
            PROBE_RESPONSE_FRAME_CONTROL => {
                let management = match extract_management(
                    core::slice::from_ref(&segment),
                    self.config.ingress,
                    mpdu,
                ) {
                    Ok(management) => management,
                    Err(error) => return rejected(protection, ConnectedRxError::Rx(error)),
                };
                if management.length < 24
                    || mpdu[4..10] != self.config.station_address
                    || mpdu[10..16] != self.config.bssid
                    || mpdu[16..22] != self.config.bssid
                {
                    return ConnectedRxDispatch::Ignored;
                }
                sink.publish(ConnectedRxEvent::ProbeResponse);
                ConnectedRxDispatch::ProbeResponse
            }
            TRIGGER_FRAME_CONTROL => {
                let control = match extract_control(
                    core::slice::from_ref(&segment),
                    self.config.ingress,
                    mpdu,
                ) {
                    Ok(control) => control,
                    Err(error) => return rejected(protection, ConnectedRxError::Rx(error)),
                };
                let trigger = match parse_trigger_frame(&mpdu[..control.length]) {
                    Ok(trigger) => trigger,
                    Err(error) => return rejected(protection, ConnectedRxError::Trigger(error)),
                };
                let Some(identity) = self.associated_he_control_identity(
                    trigger.duration,
                    trigger.receiver_address,
                    trigger.transmitter_address,
                ) else {
                    return ConnectedRxDispatch::Ignored;
                };
                let first_user = trigger.user_info_and_padding.get(..5).map(|bytes| {
                    let mut first_user = [0_u8; 5];
                    first_user.copy_from_slice(bytes);
                    first_user
                });
                sink.publish(ConnectedRxEvent::Trigger {
                    identity,
                    common: trigger.common,
                    schedule: HeTriggerScheduledRate::from_trigger_frame(
                        &trigger,
                        self.config.association_id,
                    ),
                    first_user,
                    runtime_received_at_micros,
                });
                ConnectedRxDispatch::Trigger
            }
            NDPA_FRAME_CONTROL => {
                let control = match extract_control(
                    core::slice::from_ref(&segment),
                    self.config.ingress,
                    mpdu,
                ) {
                    Ok(control) => control,
                    Err(error) => return rejected(protection, ConnectedRxError::Rx(error)),
                };
                let ndpa = match HeNdpa::parse(&mpdu[..control.length]) {
                    Ok(ndpa) => ndpa,
                    Err(error) => return rejected(protection, ConnectedRxError::Ndpa(error)),
                };
                let mut receiver_address = [0_u8; 6];
                receiver_address.copy_from_slice(ndpa.receiver_address());
                let mut transmitter_address = [0_u8; 6];
                transmitter_address.copy_from_slice(ndpa.transmitter_address());
                let Some(identity) = self.associated_he_control_identity(
                    ndpa.duration(),
                    receiver_address,
                    transmitter_address,
                ) else {
                    return ConnectedRxDispatch::Ignored;
                };
                sink.publish(ConnectedRxEvent::Ndpa {
                    identity,
                    dialog_token: ndpa.dialog_token(),
                    addressed_to_station: ndpa.contains_association_id(self.config.association_id),
                    runtime_received_at_micros,
                });
                ConnectedRxDispatch::Ndpa
            }
            ACTION_FRAME_CONTROL => {
                let management = match extract_management(
                    core::slice::from_ref(&segment),
                    self.config.ingress,
                    mpdu,
                ) {
                    Ok(management) => management,
                    Err(error) => return rejected(protection, ConnectedRxError::Rx(error)),
                };
                if management.length < 24 {
                    return ConnectedRxDispatch::Ignored;
                }
                let body = &mpdu[24..management.length];
                let is_associated_peer_action = mpdu[4..10] == self.config.station_address
                    && mpdu[10..16] == self.config.bssid
                    && mpdu[16..22] == self.config.bssid;
                if is_associated_peer_action && let Some(action) = parse_block_ack_action(body) {
                    if self.config.security == WifiSecurityMode::Open {
                        return ConnectedRxDispatch::Ignored;
                    }
                    sink.publish(ConnectedRxEvent::BlockAck { action, body });
                    return ConnectedRxDispatch::BlockAck;
                }
                if is_associated_peer_action && is_individual_twt_action_candidate(body) {
                    let action = match parse_individual_twt_action(body) {
                        Ok(action) => action,
                        Err(error) => {
                            return rejected(protection, ConnectedRxError::IndividualTwt(error));
                        }
                    };
                    sink.publish(ConnectedRxEvent::IndividualTwt { action, body });
                    return ConnectedRxDispatch::IndividualTwt;
                }

                // Do not turn every vendor Action frame into an ESP-NOW
                // rejection. Once the category/OUI identify ESP-NOW, however,
                // the strict codec owns all remaining bounds, address, BSSID,
                // type and version failures.
                if self.esp_now.is_none() || !is_esp_now_action_candidate(body) {
                    return ConnectedRxDispatch::Ignored;
                }
                let version = match esp_now_wire_version(body) {
                    Ok(version) => version,
                    Err(error) => {
                        return rejected(protection, ConnectedRxError::EspNowVersion(error));
                    }
                };
                match version {
                    EspNowWireVersion::V1 => {
                        let outcome = match self
                            .esp_now
                            .as_mut()
                            .expect("candidate admission checked ESP-NOW epoch presence")
                            .receive_v1(&mpdu[..management.length])
                        {
                            Ok(outcome) => outcome,
                            Err(error) => {
                                return rejected(protection, ConnectedRxError::EspNow(error));
                            }
                        };
                        match outcome {
                            EspNowRxOutcome::Received(received) => {
                                let metadata = decode_normalized_rx_metadata(raw)
                                    .unwrap_or_else(MacRxMetadata::unavailable);
                                let peer = received.peer();
                                let phy_mode = self
                                    .esp_now
                                    .as_ref()
                                    .and_then(|epoch| epoch.peer_phy_mode(peer))
                                    .expect(
                                        "an admitted ESP-NOW peer retains its configured PHY context",
                                    );
                                let metadata =
                                    normalize_esp_now_rx_metadata(phy_mode, metadata).normalized;
                                sink.publish(ConnectedRxEvent::EspNow { received, metadata });
                                ConnectedRxDispatch::EspNow { peer }
                            }
                            EspNowRxOutcome::Duplicate { peer } => {
                                ConnectedRxDispatch::EspNowDuplicate { peer }
                            }
                        }
                    }
                    EspNowWireVersion::V2 => {
                        if !sink.supports_esp_now_v2() {
                            return rejected(protection, ConnectedRxError::EspNowV2SinkUnavailable);
                        }
                        let outcome = match self
                            .esp_now
                            .as_mut()
                            .expect("candidate admission checked ESP-NOW epoch presence")
                            .receive_v2(&mpdu[..management.length])
                        {
                            Ok(outcome) => outcome,
                            Err(error) => {
                                return rejected(protection, ConnectedRxError::EspNowV2(error));
                            }
                        };
                        match outcome {
                            EspNowV2RxOutcome::Received(received) => {
                                let metadata = decode_normalized_rx_metadata(raw)
                                    .unwrap_or_else(MacRxMetadata::unavailable);
                                let peer = received.peer();
                                let phy_mode = self
                                    .esp_now
                                    .as_ref()
                                    .and_then(|epoch| epoch.peer_phy_mode(peer))
                                    .expect(
                                        "an admitted ESP-NOW peer retains its configured PHY context",
                                    );
                                let metadata =
                                    normalize_esp_now_rx_metadata(phy_mode, metadata).normalized;
                                sink.publish_esp_now_v2(received, metadata);
                                ConnectedRxDispatch::EspNowV2 { peer }
                            }
                            EspNowV2RxOutcome::Duplicate { peer } => {
                                ConnectedRxDispatch::EspNowV2Duplicate { peer }
                            }
                        }
                    }
                }
            }
            DISASSOCIATION_FRAME_CONTROL | DEAUTHENTICATION_FRAME_CONTROL => {
                let management = match extract_management(
                    core::slice::from_ref(&segment),
                    self.config.ingress,
                    mpdu,
                ) {
                    Ok(management) => management,
                    Err(error) => return rejected(protection, ConnectedRxError::Rx(error)),
                };
                let Some(disconnect) = parse_sta_disconnect(
                    &mpdu[..management.length],
                    self.config.station_address,
                    self.config.bssid,
                ) else {
                    return ConnectedRxDispatch::Ignored;
                };
                sink.publish(ConnectedRxEvent::PeerDisconnect(disconnect));
                ConnectedRxDispatch::PeerDisconnect
            }
            _ => self.dispatch_data(
                segment,
                frame_control,
                protection,
                runtime_received_at_micros,
                sink,
            ),
        }
    }

    fn associated_he_control_identity(
        &self,
        duration: u16,
        receiver_address: [u8; 6],
        transmitter_address: [u8; 6],
    ) -> Option<AssociatedHeControlIdentity> {
        if transmitter_address != self.config.bssid
            || (receiver_address != self.config.station_address && receiver_address[0] & 1 == 0)
        {
            return None;
        }
        Some(AssociatedHeControlIdentity {
            duration,
            receiver_address,
            transmitter_address,
        })
    }

    fn dispatch_data<S: ConnectedRxSink>(
        &mut self,
        segment: RxSegment<'_>,
        public_frame_control: u16,
        protection: ConnectedRxProtection,
        runtime_received_at_micros: Option<u64>,
        sink: &mut S,
    ) -> ConnectedRxDispatch {
        if public_frame_control & DATA_TYPE_MASK != DATA_TYPE {
            return ConnectedRxDispatch::Ignored;
        }
        let observed_protected = public_frame_control & PROTECTED != 0;
        let expected_protected = self.config.security == WifiSecurityMode::Wpa2Personal;
        let fragmented = public_fragmented(segment.buffer, public_frame_control).unwrap_or(false);
        if expected_protected && !observed_protected {
            return self.dispatch_unprotected_eapol(segment, protection, sink);
        }
        if observed_protected != expected_protected {
            return rejected(protection, ConnectedRxError::SecurityModeMismatch);
        }
        if fragmented {
            return match self.config.security {
                WifiSecurityMode::Open => self.dispatch_open_fragment(
                    segment,
                    protection,
                    runtime_received_at_micros,
                    sink,
                ),
                WifiSecurityMode::Wpa2Personal => self.dispatch_protected_fragment(
                    segment,
                    protection,
                    runtime_received_at_micros,
                    sink,
                ),
            };
        }
        let (mpdu, retry, sequence_control, tid, ccmp_header, data) = match self.config.security {
            WifiSecurityMode::Open => {
                let view = match view_unprotected_data(segment, self.config.ingress) {
                    Ok(view) => view,
                    Err(error) => return rejected(protection, ConnectedRxError::Rx(error)),
                };
                let identity = match parse_open_data_identity(DataInterfaceRole::Station, view.mpdu)
                {
                    Ok(identity) => identity,
                    Err(error) => {
                        return rejected(protection, ConnectedRxError::Fragment(error));
                    }
                };
                if identity.transmitter_address() != self.config.bssid
                    || identity.receiver_address() != self.config.station_address
                        && identity.receiver_address()[0] & 1 == 0
                    || protection == ConnectedRxProtection::Other
                {
                    return ConnectedRxDispatch::Ignored;
                }
                match self.fragments.admit_unfragmented(
                    identity,
                    view.retry,
                    runtime_received_at_micros,
                ) {
                    Ok(OpenDataUnfragmentedAdmission::Admitted { .. }) => {}
                    Ok(OpenDataUnfragmentedAdmission::Duplicate { .. }) => {
                        return ConnectedRxDispatch::Duplicate;
                    }
                    Err(error) => {
                        return rejected(protection, ConnectedRxError::Fragment(error));
                    }
                }
                let identity = (view.mpdu, view.retry, view.sequence_control, view.tid);
                let data = match view.decapsulate(DataInterfaceRole::Station) {
                    Ok(data) => data,
                    Err(error) => return rejected(protection, ConnectedRxError::Data(error)),
                };
                (identity.0, identity.1, identity.2, identity.3, None, data)
            }
            WifiSecurityMode::Wpa2Personal => {
                let view = match view_protected_data(segment, self.config.ingress) {
                    Ok(view) => view,
                    Err(error) => return rejected(protection, ConnectedRxError::Rx(error)),
                };
                let fragment_identity = match parse_ccmp_data_identity(
                    DataInterfaceRole::Station,
                    view.mpdu,
                    view.ccmp_header,
                ) {
                    Ok(identity) => identity,
                    Err(error) => {
                        return rejected(protection, ConnectedRxError::Fragment(error));
                    }
                };
                if fragment_identity.transmitter_address() != self.config.bssid
                    || fragment_identity.receiver_address() != self.config.station_address
                        && fragment_identity.receiver_address()[0] & 1 == 0
                    || protection == ConnectedRxProtection::Other
                {
                    return ConnectedRxDispatch::Ignored;
                }
                match self.fragments.admit_unfragmented(
                    fragment_identity,
                    view.retry,
                    runtime_received_at_micros,
                ) {
                    Ok(OpenDataUnfragmentedAdmission::Admitted { .. }) => {}
                    Ok(OpenDataUnfragmentedAdmission::Duplicate { .. }) => {
                        return ConnectedRxDispatch::Duplicate;
                    }
                    Err(error) => {
                        return rejected(protection, ConnectedRxError::Fragment(error));
                    }
                }
                let identity = (
                    view.mpdu,
                    view.retry,
                    view.sequence_control,
                    view.tid,
                    view.ccmp_header,
                );
                let data = match view.decapsulate(DataInterfaceRole::Station) {
                    Ok(data) => data,
                    Err(error) => return rejected(protection, ConnectedRxError::Data(error)),
                };
                (
                    identity.0,
                    identity.1,
                    identity.2,
                    identity.3,
                    Some(identity.4),
                    data,
                )
            }
        };
        if mpdu[10..16] != self.config.bssid {
            return ConnectedRxDispatch::Ignored;
        }
        if tid.is_some() && !self.config.peer_qos {
            return rejected(protection, ConnectedRxError::PeerQosMismatch);
        }
        let mut shared_publication = None;
        if let Some(header) = ccmp_header {
            let prepared = match prepare_ccmp_replay(&mut self.ccmp_replay, protection, tid, header)
            {
                Ok(prepared) => prepared,
                Err(error) => return rejected(protection, ConnectedRxError::CcmpReplay(error)),
            };
            // Hardware MIC verification, complete decapsulation and associated
            // peer admission have all completed. Commit before any Ethernet
            // publication; a malformed authenticated MPDU may burn a PN but
            // can never make it reusable.
            shared_publication = match commit_ccmp_replay(&mut self.ccmp_replay, prepared) {
                Ok(permit) => permit,
                Err(error) => return rejected(protection, ConnectedRxError::CcmpReplay(error)),
            };
        }
        if self
            .duplicate_filter
            .is_duplicate(retry, sequence_control, tid)
        {
            return ConnectedRxDispatch::Duplicate;
        }
        if protection == ConnectedRxProtection::Pairwise {
            sink.publish(ConnectedRxEvent::PowerSaveDelivery(StaPsPollDelivery {
                more_data: public_frame_control & MORE_DATA != 0,
            }));
        }
        let mut frames = data.frames;
        let amsdu = data.amsdu;
        let mut count = 0_u8;
        for frame in &mut frames {
            let frame = match frame {
                Ok(frame) => frame,
                Err(error) => return rejected(protection, ConnectedRxError::Data(error)),
            };
            sink.publish(ConnectedRxEvent::Ethernet {
                frame,
                raw: data.raw,
                amsdu,
                metadata: data.metadata,
            });
            count = count.saturating_add(1);
        }
        let result = ConnectedRxDispatch::Data {
            ethernet_frames: count,
            amsdu,
        };
        drop(shared_publication);
        result
    }

    fn dispatch_open_fragment<S: ConnectedRxSink>(
        &mut self,
        segment: RxSegment<'_>,
        protection: ConnectedRxProtection,
        runtime_received_at_micros: Option<u64>,
        sink: &mut S,
    ) -> ConnectedRxDispatch {
        let Some(now_micros) = runtime_received_at_micros else {
            return rejected(
                protection,
                ConnectedRxError::Fragment(OpenDataFragmentError::ClockUnavailable),
            );
        };
        let view = match view_unprotected_data_fragment(
            segment,
            self.config.ingress,
            DataInterfaceRole::Station,
        ) {
            Ok(view) => view,
            Err(UnprotectedDataFragmentRxError::Radio(error)) => {
                return rejected(protection, ConnectedRxError::Rx(error));
            }
            Err(UnprotectedDataFragmentRxError::Fragment(error)) => {
                return rejected(protection, ConnectedRxError::Fragment(error));
            }
        };
        let identity = view.fragment.identity();
        if identity.transmitter_address() != self.config.bssid
            || identity.receiver_address() != self.config.station_address
                && identity.receiver_address()[0] & 1 == 0
            || protection == ConnectedRxProtection::Other
        {
            return ConnectedRxDispatch::Ignored;
        }
        if identity.tid().is_some() && !self.config.peer_qos {
            return rejected(protection, ConnectedRxError::PeerQosMismatch);
        }
        if view.fragment.fragment_number() == 0
            && self.duplicate_filter.is_known_duplicate(
                view.fragment.retry(),
                view.fragment.sequence_control(),
                identity.tid(),
            )
        {
            // Fragment zero shares the ordinary MPDU's Sequence Control
            // value. Consult the association-wide duplicate owner exactly at
            // this edge so a retry cannot turn an already accepted ordinary
            // MPDU into a new fragment train merely by setting More
            // Fragments. Later fragments remain owned by the reassembler's
            // per-fragment history, and no fragment mutates ordinary history.
            return ConnectedRxDispatch::Duplicate;
        }
        let power_save_delivery =
            (protection == ConnectedRxProtection::Pairwise).then_some(StaPsPollDelivery {
                more_data: u16::from_le_bytes([view.mpdu[0], view.mpdu[1]]) & MORE_DATA != 0,
            });
        let raw = view.raw;
        let metadata = view.metadata;
        match self.fragments.ingest(view.fragment, now_micros, |data| {
            if let Some(delivery) = power_save_delivery {
                sink.publish(ConnectedRxEvent::PowerSaveDelivery(delivery));
            }
            sink.publish(ConnectedRxEvent::Ethernet {
                frame: data.ethernet_frame(),
                raw,
                amsdu: false,
                metadata,
            });
        }) {
            Ok(OpenDataDefragmentation::Buffered { expired, evicted }) => {
                ConnectedRxDispatch::FragmentBuffered {
                    expired,
                    evicted: evicted.is_some(),
                }
            }
            Ok(OpenDataDefragmentation::Duplicate { .. }) => ConnectedRxDispatch::Duplicate,
            Ok(OpenDataDefragmentation::Complete { .. }) => ConnectedRxDispatch::Data {
                ethernet_frames: 1,
                amsdu: false,
            },
            Err(error) => rejected(protection, ConnectedRxError::Fragment(error)),
        }
    }

    fn dispatch_protected_fragment<S: ConnectedRxSink>(
        &mut self,
        segment: RxSegment<'_>,
        protection: ConnectedRxProtection,
        runtime_received_at_micros: Option<u64>,
        sink: &mut S,
    ) -> ConnectedRxDispatch {
        let Some(now_micros) = runtime_received_at_micros else {
            return rejected(
                protection,
                ConnectedRxError::Fragment(OpenDataFragmentError::ClockUnavailable),
            );
        };
        let view = match view_protected_data_fragment(
            segment,
            self.config.ingress,
            DataInterfaceRole::Station,
        ) {
            Ok(view) => view,
            Err(ProtectedDataFragmentRxError::Radio(error)) => {
                return rejected(protection, ConnectedRxError::Rx(error));
            }
            Err(ProtectedDataFragmentRxError::Fragment(error)) => {
                return rejected(protection, ConnectedRxError::Fragment(error));
            }
        };
        let identity = view.fragment.identity();
        if identity.transmitter_address() != self.config.bssid
            || identity.receiver_address() != self.config.station_address
            || protection != ConnectedRxProtection::Pairwise
        {
            return ConnectedRxDispatch::Ignored;
        }
        let tid = identity.tid();
        if tid.is_some() && !self.config.peer_qos {
            return rejected(protection, ConnectedRxError::PeerQosMismatch);
        }
        if view.fragment.fragment_number() == 0
            && self.duplicate_filter.is_known_duplicate(
                view.fragment.retry(),
                view.fragment.sequence_control(),
                tid,
            )
        {
            // Protected and Open fragment zero share the ordinary MPDU's
            // Sequence Control space. Fence an authenticated Retry against
            // the association-wide ordinary history before replay prepare or
            // fragment preflight can advance or mutate either owner.
            return ConnectedRxDispatch::Duplicate;
        }

        let raw = view.raw;
        let metadata = view.metadata;
        let more_data = u16::from_le_bytes([view.mpdu[0], view.mpdu[1]]) & MORE_DATA != 0;
        let fragment = view.fragment;
        let more_fragments = fragment.more_fragments();
        let ccmp_header = view.ccmp_header;
        let (fragments, replay) = (&mut self.fragments, &mut self.ccmp_replay);
        let admission = match fragments.preflight_in_epoch(fragment, 0, now_micros) {
            Ok(OpenDataFragmentPreflight::Duplicate { .. }) => {
                return ConnectedRxDispatch::Duplicate;
            }
            Ok(OpenDataFragmentPreflight::Admitted(admission)) => admission,
            Err(error) => return rejected(protection, ConnectedRxError::Fragment(error)),
        };
        let prepared = match prepare_ccmp_replay(replay, protection, tid, ccmp_header) {
            Ok(prepared) => prepared,
            Err(error) => return rejected(protection, ConnectedRxError::CcmpReplay(error)),
        };

        if more_fragments {
            let outcome =
                admission.ingest(|_| unreachable!("More Fragments cannot complete one MSDU"));
            let outcome = match outcome {
                Ok(outcome) => outcome,
                Err(error) => return rejected(protection, ConnectedRxError::Fragment(error)),
            };
            if let Err(error) = commit_ccmp_replay(replay, prepared) {
                fragments.discard(identity, 0);
                return rejected(protection, ConnectedRxError::CcmpReplay(error));
            }
            return match outcome {
                OpenDataDefragmentation::Buffered { expired, evicted } => {
                    ConnectedRxDispatch::FragmentBuffered {
                        expired,
                        evicted: evicted.is_some(),
                    }
                }
                _ => unreachable!("More Fragments produces only a buffered admission"),
            };
        }

        let outcome = admission.ingest(|data| {
            let publication = commit_ccmp_replay(replay, prepared)?;
            sink.publish(ConnectedRxEvent::PowerSaveDelivery(StaPsPollDelivery {
                more_data,
            }));
            sink.publish(ConnectedRxEvent::Ethernet {
                frame: data.ethernet_frame(),
                raw,
                amsdu: false,
                metadata,
            });
            drop(publication);
            Ok::<(), StaCcmpRxReplayError>(())
        });
        match outcome {
            Ok(OpenDataDefragmentation::Complete { value: Ok(()), .. }) => {
                ConnectedRxDispatch::Data {
                    ethernet_frames: 1,
                    amsdu: false,
                }
            }
            Ok(OpenDataDefragmentation::Complete {
                value: Err(error), ..
            }) => {
                fragments.discard(identity, 0);
                rejected(protection, ConnectedRxError::CcmpReplay(error))
            }
            Ok(_) => unreachable!("final fragment produces one completion"),
            Err(error) => rejected(protection, ConnectedRxError::Fragment(error)),
        }
    }

    fn dispatch_unprotected_eapol<S: ConnectedRxSink>(
        &mut self,
        segment: RxSegment<'_>,
        protection: ConnectedRxProtection,
        sink: &mut S,
    ) -> ConnectedRxDispatch {
        if protection != ConnectedRxProtection::Pairwise {
            return rejected(protection, ConnectedRxError::SecurityModeMismatch);
        }
        let view = match view_unprotected_data(segment, self.config.ingress) {
            Ok(view) => view,
            Err(error) => return rejected(protection, ConnectedRxError::Rx(error)),
        };
        if view.mpdu.get(4..10) != Some(&self.config.station_address)
            || view.mpdu.get(10..16) != Some(&self.config.bssid)
            || (view.tid.is_some() && !self.config.peer_qos)
        {
            return rejected(protection, ConnectedRxError::SecurityModeMismatch);
        }
        let data = match view.decapsulate(DataInterfaceRole::Station) {
            Ok(data) => data,
            Err(error) => return rejected(protection, ConnectedRxError::Data(error)),
        };
        if data.amsdu {
            return rejected(protection, ConnectedRxError::SecurityModeMismatch);
        }
        let mut frames = data.frames;
        let frame = match frames.next() {
            Some(Ok(frame)) => frame,
            Some(Err(error)) => return rejected(protection, ConnectedRxError::Data(error)),
            None => return rejected(protection, ConnectedRxError::SecurityModeMismatch),
        };
        if frames.next().is_some()
            || frame.destination != self.config.station_address
            || frame.source != self.config.bssid
            || frame.ether_type != 0x888e
        {
            return rejected(protection, ConnectedRxError::SecurityModeMismatch);
        }
        if let Err(error) = EapolKeyFrame::parse(frame.payload) {
            return rejected(protection, ConnectedRxError::Eapol(error));
        }
        sink.publish(ConnectedRxEvent::UnprotectedEapol {
            source: frame.source,
            payload: frame.payload,
        });
        ConnectedRxDispatch::UnprotectedEapol
    }
}

impl Default for ConnectedRxDispatcher {
    fn default() -> Self {
        Self::unconfigured()
    }
}

fn rejected(protection: ConnectedRxProtection, error: ConnectedRxError) -> ConnectedRxDispatch {
    ConnectedRxDispatch::Rejected { protection, error }
}

fn is_esp_now_action_candidate(body: &[u8]) -> bool {
    if body.first() != Some(&ESP_NOW_ACTION_CATEGORY) {
        return false;
    }
    body.len() < 4 || body[1..4] == ESP_NOW_ORGANIZATION_IDENTIFIER
}

fn public_frame_control(raw: &[u8]) -> Option<u16> {
    Some(u16::from_le_bytes([
        *raw.get(PUBLIC_HEADER_SIZE)?,
        *raw.get(PUBLIC_HEADER_SIZE + 1)?,
    ]))
}

fn public_protection(
    raw: &[u8],
    frame_control: u16,
    station_address: [u8; 6],
) -> Option<ConnectedRxProtection> {
    if frame_control & DATA_TYPE_MASK != DATA_TYPE {
        return Some(ConnectedRxProtection::Other);
    }
    let destination = raw.get(PUBLIC_HEADER_SIZE + 4..PUBLIC_HEADER_SIZE + 10)?;
    if destination.first().is_some_and(|byte| byte & 1 != 0) {
        Some(ConnectedRxProtection::Group)
    } else if destination == station_address {
        Some(ConnectedRxProtection::Pairwise)
    } else {
        Some(ConnectedRxProtection::Other)
    }
}

fn public_fragmented(raw: &[u8], frame_control: u16) -> Option<bool> {
    if frame_control & MORE_FRAGMENTS != 0 {
        return Some(true);
    }
    Some(*raw.get(PUBLIC_HEADER_SIZE + 22)? & 0x0f != 0)
}

#[cfg(test)]
mod tests {
    use std::vec::Vec;

    use open_esp_radio_esp32s31_wifi_dma::descriptor::{BIT_30, BIT_31, LENGTH_SHIFT};

    use super::*;

    const STATION: [u8; 6] = [0x10, 0x11, 0x12, 0x13, 0x14, 0x15];
    const BSSID: [u8; 6] = [0x20, 0x21, 0x22, 0x23, 0x24, 0x25];
    const SOURCE: [u8; 6] = [0x30, 0x31, 0x32, 0x33, 0x34, 0x35];
    const TAIL_OFFSET: usize = 0x38;
    const FRAME_OFFSET: usize = 0x40;

    fn replay_resource() -> (StaCcmpRxReplayRxEndpoint, StaCcmpRxReplayControlEndpoint) {
        let resource = std::boxed::Box::leak(std::boxed::Box::new(StaCcmpRxReplayResource::new()));
        resource
            .start(StaCcmpRxReplayEpoch::new([0; 8], 1, [0; 8]).unwrap())
            .unwrap()
    }

    fn ccmp_header(packet_number: u64, key_id: u8) -> CcmpHeader {
        CcmpHeader::new(
            open_esp_radio_ieee80211::ccmp::CcmpPacketNumber::new(packet_number).unwrap(),
            CcmpKeyId::new(key_id).unwrap(),
        )
    }

    #[test]
    fn shared_group_rotation_changes_key_id_and_rsc_without_resetting_pairwise() {
        let (mut rx, mut control) = replay_resource();
        drop(
            rx.prepare_publication(ConnectedRxProtection::Pairwise, Some(3), ccmp_header(9, 0))
                .unwrap(),
        );
        drop(
            rx.prepare_publication(ConnectedRxProtection::Group, Some(3), ccmp_header(7, 1))
                .unwrap(),
        );

        let prepared = control
            .prepare_group_rotation(2, [20, 0, 0, 0, 0, 0, 0, 0])
            .unwrap();
        let installing = control.begin_group_rotation(prepared).unwrap();
        assert_eq!(
            rx.prepare_publication(ConnectedRxProtection::Group, Some(3), ccmp_header(21, 2),)
                .err(),
            Some(StaCcmpRxReplayError::GroupRotationInProgress)
        );
        // The group gate never resets or suspends PTK replay ownership.
        drop(
            rx.prepare_publication(ConnectedRxProtection::Pairwise, Some(3), ccmp_header(10, 0))
                .unwrap(),
        );
        control.commit_group_rotation(installing).unwrap();

        assert!(matches!(
            rx.prepare_publication(ConnectedRxProtection::Group, Some(3), ccmp_header(22, 1),),
            Err(StaCcmpRxReplayError::UnexpectedKeyId { .. })
        ));
        assert!(matches!(
            rx.prepare_publication(ConnectedRxProtection::Group, Some(3), ccmp_header(20, 2),),
            Err(StaCcmpRxReplayError::Replay(
                CcmpReplayError::Replayed { .. }
            ))
        ));
        drop(
            rx.prepare_publication(ConnectedRxProtection::Group, Some(3), ccmp_header(21, 2))
                .unwrap(),
        );
        assert!(matches!(
            rx.prepare_publication(ConnectedRxProtection::Pairwise, Some(3), ccmp_header(10, 0),),
            Err(StaCcmpRxReplayError::Replay(
                CcmpReplayError::Replayed { .. }
            ))
        ));
        drop(
            rx.prepare_publication(ConnectedRxProtection::Pairwise, Some(3), ccmp_header(11, 0))
                .unwrap(),
        );
        rx.stop().unwrap();
        control.stop().unwrap();
    }

    #[test]
    fn shared_group_rotation_applies_same_key_id_rsc_and_rejects_stale_candidate() {
        let (mut rx, mut control) = replay_resource();
        let stale = control
            .prepare_group_rotation(1, [4, 0, 0, 0, 0, 0, 0, 0])
            .unwrap();
        let current = control
            .prepare_group_rotation(1, [8, 0, 0, 0, 0, 0, 0, 0])
            .unwrap();
        let installing = control.begin_group_rotation(current).unwrap();
        control.commit_group_rotation(installing).unwrap();
        assert_eq!(
            control.begin_group_rotation(stale).err(),
            Some(StaCcmpRxReplayError::StaleGroupRotation)
        );
        assert!(matches!(
            rx.prepare_publication(ConnectedRxProtection::Group, None, ccmp_header(8, 1),),
            Err(StaCcmpRxReplayError::Replay(
                CcmpReplayError::Replayed { .. }
            ))
        ));
        drop(
            rx.prepare_publication(ConnectedRxProtection::Group, None, ccmp_header(9, 1))
                .unwrap(),
        );
        rx.stop().unwrap();
        control.stop().unwrap();
    }

    #[test]
    fn same_key_id_rotation_merges_each_lane_monotonically_but_new_key_id_resets() {
        let (mut rx, mut control) = replay_resource();
        drop(
            rx.prepare_publication(ConnectedRxProtection::Group, Some(0), ccmp_header(10, 1))
                .unwrap(),
        );
        drop(
            rx.prepare_publication(ConnectedRxProtection::Group, Some(7), ccmp_header(20, 1))
                .unwrap(),
        );
        drop(
            rx.prepare_publication(ConnectedRxProtection::Group, None, ccmp_header(5, 1))
                .unwrap(),
        );

        let lower = control
            .prepare_group_rotation(1, [8, 0, 0, 0, 0, 0, 0, 0])
            .unwrap();
        let installing = control.begin_group_rotation(lower).unwrap();
        control.commit_group_rotation(installing).unwrap();
        for (tid, highest) in [(Some(0), 10), (Some(7), 20), (None, 8)] {
            assert!(matches!(
                rx.prepare_publication(ConnectedRxProtection::Group, tid, ccmp_header(highest, 1),),
                Err(StaCcmpRxReplayError::Replay(
                    CcmpReplayError::Replayed { .. }
                ))
            ));
        }

        let higher = control
            .prepare_group_rotation(1, [25, 0, 0, 0, 0, 0, 0, 0])
            .unwrap();
        let installing = control.begin_group_rotation(higher).unwrap();
        control.commit_group_rotation(installing).unwrap();
        let equal = control
            .prepare_group_rotation(1, [25, 0, 0, 0, 0, 0, 0, 0])
            .unwrap();
        let installing = control.begin_group_rotation(equal).unwrap();
        control.commit_group_rotation(installing).unwrap();
        for tid in [None, Some(0), Some(7), Some(15)] {
            assert!(matches!(
                rx.prepare_publication(ConnectedRxProtection::Group, tid, ccmp_header(25, 1),),
                Err(StaCcmpRxReplayError::Replay(
                    CcmpReplayError::Replayed { .. }
                ))
            ));
        }

        let new_key = control
            .prepare_group_rotation(2, [3, 0, 0, 0, 0, 0, 0, 0])
            .unwrap();
        let installing = control.begin_group_rotation(new_key).unwrap();
        control.commit_group_rotation(installing).unwrap();
        drop(
            rx.prepare_publication(ConnectedRxProtection::Group, Some(7), ccmp_header(4, 2))
                .unwrap(),
        );
        rx.stop().unwrap();
        control.stop().unwrap();
    }

    #[test]
    fn same_key_id_begin_refreshes_all_lanes_advanced_after_prepare() {
        let (mut rx, mut control) = replay_resource();
        let prepared = control
            .prepare_group_rotation(1, [7, 0, 0, 0, 0, 0, 0, 0])
            .unwrap();

        for tid in core::iter::once(None).chain((0_u8..16).map(Some)) {
            let packet_number = 40 + u64::from(tid.unwrap_or(16));
            drop(
                rx.prepare_publication(
                    ConnectedRxProtection::Group,
                    tid,
                    ccmp_header(packet_number, 1),
                )
                .unwrap(),
            );
        }

        let installing = control.begin_group_rotation(prepared).unwrap();
        control.commit_group_rotation(installing).unwrap();
        for tid in core::iter::once(None).chain((0_u8..16).map(Some)) {
            let packet_number = 40 + u64::from(tid.unwrap_or(16));
            assert!(matches!(
                rx.prepare_publication(
                    ConnectedRxProtection::Group,
                    tid,
                    ccmp_header(packet_number, 1),
                ),
                Err(StaCcmpRxReplayError::Replay(
                    CcmpReplayError::Replayed { .. }
                ))
            ));
        }
        rx.stop().unwrap();
        control.stop().unwrap();
    }

    #[test]
    fn group_publication_and_stop_races_fail_without_mixing_epochs() {
        let (mut rx, mut control) = replay_resource();
        let permit = rx
            .prepare_publication(ConnectedRxProtection::Group, None, ccmp_header(1, 1))
            .unwrap();
        let prepared = control
            .prepare_group_rotation(2, [3, 0, 0, 0, 0, 0, 0, 0])
            .unwrap();
        assert_eq!(
            control.begin_group_rotation(prepared).err(),
            Some(StaCcmpRxReplayError::PublicationInFlight)
        );
        drop(permit);

        let prepared = control
            .prepare_group_rotation(2, [3, 0, 0, 0, 0, 0, 0, 0])
            .unwrap();
        let installing = control.begin_group_rotation(prepared).unwrap();
        control.abort_group_rotation(installing).unwrap();
        rx.stop().unwrap();
        control.stop().unwrap();

        let (mut rx, mut control) = replay_resource();
        let prepared = control
            .prepare_group_rotation(2, [3, 0, 0, 0, 0, 0, 0, 0])
            .unwrap();
        let installing = control.begin_group_rotation(prepared).unwrap();
        rx.stop().unwrap();
        assert_eq!(
            control.commit_group_rotation(installing),
            Err(StaCcmpRxReplayError::RxStopped)
        );
        control.stop().unwrap();
    }

    #[test]
    fn endpoint_drop_defers_epoch_release_until_group_publication_returns() {
        let resource = std::boxed::Box::leak(std::boxed::Box::new(StaCcmpRxReplayResource::new()));
        let (rx, control) = resource
            .start(StaCcmpRxReplayEpoch::new([0; 8], 1, [0; 8]).unwrap())
            .unwrap();
        let permit = rx
            .prepare_publication(ConnectedRxProtection::Group, None, ccmp_header(1, 1))
            .unwrap();
        drop(rx);
        drop(control);
        let busy = match resource.start(StaCcmpRxReplayEpoch::new([0; 8], 1, [0; 8]).unwrap()) {
            Ok(_) => panic!("a live publication must retain the old replay epoch"),
            Err(failure) => failure,
        };
        let (error, recovered) = busy.into_parts();
        assert_eq!(error, StaCcmpRxReplayStartError::Busy);

        drop(permit);
        let (mut rx, mut control) = resource.start(recovered).unwrap();
        rx.stop().unwrap();
        control.stop().unwrap();
    }

    #[test]
    fn stale_generation_commit_cannot_quarantine_new_epoch() {
        let resource = std::boxed::Box::leak(std::boxed::Box::new(StaCcmpRxReplayResource::new()));
        let (mut old_rx, mut old_control) = resource
            .start(StaCcmpRxReplayEpoch::new([0; 8], 1, [0; 8]).unwrap())
            .unwrap();
        let prepared = old_control
            .prepare_group_rotation(2, [3, 0, 0, 0, 0, 0, 0, 0])
            .unwrap();
        let installing = old_control.begin_group_rotation(prepared).unwrap();
        old_rx.stop().unwrap();
        old_control.stop().unwrap();

        let (mut rx, mut control) = resource
            .start(StaCcmpRxReplayEpoch::new([0; 8], 2, [0; 8]).unwrap())
            .unwrap();
        assert_eq!(
            old_control.commit_group_rotation(installing),
            Err(StaCcmpRxReplayError::StaleGroupRotation)
        );
        drop(
            rx.prepare_publication(ConnectedRxProtection::Group, None, ccmp_header(1, 2))
                .unwrap(),
        );
        rx.stop().unwrap();
        control.stop().unwrap();
    }

    #[test]
    fn stale_generation_abort_cannot_quarantine_new_epoch() {
        let resource = std::boxed::Box::leak(std::boxed::Box::new(StaCcmpRxReplayResource::new()));
        let (mut old_rx, mut old_control) = resource
            .start(StaCcmpRxReplayEpoch::new([0; 8], 1, [0; 8]).unwrap())
            .unwrap();
        let prepared = old_control
            .prepare_group_rotation(2, [3, 0, 0, 0, 0, 0, 0, 0])
            .unwrap();
        let installing = old_control.begin_group_rotation(prepared).unwrap();
        old_rx.stop().unwrap();
        old_control.stop().unwrap();

        let (mut rx, mut control) = resource
            .start(StaCcmpRxReplayEpoch::new([0; 8], 2, [0; 8]).unwrap())
            .unwrap();
        assert_eq!(
            old_control.abort_group_rotation(installing),
            Err(StaCcmpRxReplayError::StaleGroupRotation)
        );
        drop(
            rx.prepare_publication(ConnectedRxProtection::Group, None, ccmp_header(1, 2))
                .unwrap(),
        );
        rx.stop().unwrap();
        control.stop().unwrap();
    }

    #[test]
    fn generation_and_rotation_ticket_exhaustion_fail_closed_without_wrap() {
        let exhausted_generation =
            std::boxed::Box::leak(std::boxed::Box::new(StaCcmpRxReplayResource::new()));
        critical_section::with(|cs| {
            exhausted_generation
                .state
                .borrow(cs)
                .borrow_mut()
                .generation = u32::MAX;
        });
        let exhausted = match exhausted_generation
            .start(StaCcmpRxReplayEpoch::new([0; 8], 1, [0; 8]).unwrap())
        {
            Ok(_) => panic!("an exhausted generation must not wrap"),
            Err(failure) => failure,
        };
        let (error, recovered) = exhausted.into_parts();
        assert_eq!(error, StaCcmpRxReplayStartError::GenerationExhausted);
        let recovery_resource =
            std::boxed::Box::leak(std::boxed::Box::new(StaCcmpRxReplayResource::new()));
        let (mut recovered_rx, mut recovered_control) = recovery_resource.start(recovered).unwrap();
        recovered_rx.stop().unwrap();
        recovered_control.stop().unwrap();

        let resource = std::boxed::Box::leak(std::boxed::Box::new(StaCcmpRxReplayResource::new()));
        let (mut rx, mut control) = resource
            .start(StaCcmpRxReplayEpoch::new([0; 8], 1, [0; 8]).unwrap())
            .unwrap();
        critical_section::with(|cs| {
            resource.state.borrow(cs).borrow_mut().next_rotation_ticket = u32::MAX;
        });
        assert_eq!(
            control
                .prepare_group_rotation(2, [3, 0, 0, 0, 0, 0, 0, 0])
                .err(),
            Some(StaCcmpRxReplayError::GroupRotationTicketExhausted)
        );
        assert_eq!(
            rx.prepare_publication(ConnectedRxProtection::Group, None, ccmp_header(1, 1))
                .err(),
            Some(StaCcmpRxReplayError::GroupRotationInProgress)
        );
        drop(
            rx.prepare_publication(ConnectedRxProtection::Pairwise, None, ccmp_header(1, 0))
                .unwrap(),
        );
        rx.stop().unwrap();
        control.stop().unwrap();
    }

    #[derive(Default)]
    struct RecordingSink {
        beacons: Vec<StaBeaconObservation>,
        beacon_metadata: Vec<MacRxMetadata<RxPhyInfo>>,
        probe_responses: u32,
        ethernet: Vec<Vec<u8>>,
        ethernet_metadata: Vec<MacRxMetadata<RxPhyInfo>>,
        block_ack: Vec<BlockAckAction>,
        peer_disconnects: Vec<StaDisconnect>,
        power_save_deliveries: Vec<StaPsPollDelivery>,
        unprotected_eapol: Vec<Vec<u8>>,
    }

    impl ConnectedRxSink for RecordingSink {
        fn publish(&mut self, event: ConnectedRxEvent<'_>) {
            match event {
                ConnectedRxEvent::Beacon {
                    observation,
                    metadata,
                } => {
                    self.beacons.push(observation);
                    self.beacon_metadata.push(metadata);
                }
                ConnectedRxEvent::ProbeResponse => {
                    self.probe_responses = self.probe_responses.saturating_add(1);
                }
                ConnectedRxEvent::Ethernet {
                    frame, metadata, ..
                } => {
                    let mut bytes = std::vec![0; frame.length()];
                    frame.copy_to(&mut bytes).unwrap();
                    self.ethernet.push(bytes);
                    self.ethernet_metadata.push(metadata);
                }
                ConnectedRxEvent::BlockAck { action, .. } => self.block_ack.push(action),
                ConnectedRxEvent::PeerDisconnect(disconnect) => {
                    self.peer_disconnects.push(disconnect);
                }
                ConnectedRxEvent::PowerSaveDelivery(delivery) => {
                    self.power_save_deliveries.push(delivery);
                }
                ConnectedRxEvent::UnprotectedEapol { payload, .. } => {
                    self.unprotected_eapol.push(payload.to_vec());
                }
                ConnectedRxEvent::Trigger { .. }
                | ConnectedRxEvent::Ndpa { .. }
                | ConnectedRxEvent::IndividualTwt { .. }
                | ConnectedRxEvent::EspNow { .. } => {}
            }
        }
    }

    #[test]
    fn routes_associated_beacon_and_local_tim_as_owned_control_state() {
        const FIXED: usize = 36;
        const TIM: [u8; 6] = [5, 4, 0, 3, 1, 0x80];
        const MPDU: usize = FIXED + TIM.len();
        const SIGNAL: usize = MPDU + 4;
        let mut storage = [0_u8; 192];
        set_tail(&mut storage, SIGNAL);
        let frame = &mut storage[FRAME_OFFSET..FRAME_OFFSET + MPDU];
        frame[..2].copy_from_slice(&BEACON_FRAME_CONTROL.to_le_bytes());
        frame[4..10].fill(0xff);
        frame[10..16].copy_from_slice(&BSSID);
        frame[16..22].copy_from_slice(&BSSID);
        frame[24..32].copy_from_slice(&123_u64.to_le_bytes());
        frame[32..34].copy_from_slice(&100_u16.to_le_bytes());
        frame[34..36].copy_from_slice(&0x0431_u16.to_le_bytes());
        frame[FIXED..].copy_from_slice(&TIM);

        let mut dispatcher = ConnectedRxDispatcher::new(config());
        let mut sink = RecordingSink::default();
        let mut mpdu = [0_u8; 128];
        let mut ethernet = [0_u8; 128];
        assert!(!dispatcher.may_publish_amsdu(segment(&storage, SIGNAL)));
        assert_eq!(
            dispatcher.dispatch(
                segment(&storage, SIGNAL),
                &mut mpdu,
                &mut ethernet,
                &mut sink,
            ),
            ConnectedRxDispatch::Beacon
        );
        assert_eq!(sink.beacons.len(), 1);
        assert_eq!(sink.beacons[0].timestamp_tsf, 123);
        assert_eq!(sink.beacons[0].interval_tu, 100);
        assert!(sink.beacons[0].tim.unwrap().unicast_buffered);
        assert!(sink.beacons[0].tim.unwrap().group_buffered);
        assert_eq!(
            sink.beacon_metadata[0].s_mpdu,
            MacRxEvidence::HardwareObserved(true)
        );
        assert_eq!(
            sink.beacon_metadata[0].ampdu,
            MacRxEvidence::ProtocolValidated(false)
        );
    }

    #[test]
    fn routes_only_probe_responses_from_the_associated_bssid() {
        const MPDU: usize = 36;
        const SIGNAL: usize = MPDU + 4;
        let mut storage = [0_u8; 192];
        set_tail(&mut storage, SIGNAL);
        let frame = &mut storage[FRAME_OFFSET..FRAME_OFFSET + MPDU];
        frame[..2].copy_from_slice(&PROBE_RESPONSE_FRAME_CONTROL.to_le_bytes());
        frame[4..10].copy_from_slice(&STATION);
        frame[10..16].copy_from_slice(&BSSID);
        frame[16..22].copy_from_slice(&BSSID);

        let mut dispatcher = ConnectedRxDispatcher::new(config());
        let mut sink = RecordingSink::default();
        let mut mpdu = [0_u8; 128];
        let mut ethernet = [0_u8; 128];
        assert_eq!(
            dispatcher.dispatch(
                segment(&storage, SIGNAL),
                &mut mpdu,
                &mut ethernet,
                &mut sink,
            ),
            ConnectedRxDispatch::ProbeResponse
        );
        assert_eq!(sink.probe_responses, 1);

        storage[FRAME_OFFSET + 10..FRAME_OFFSET + 16].copy_from_slice(&SOURCE);
        assert_eq!(
            dispatcher.dispatch(
                segment(&storage, SIGNAL),
                &mut mpdu,
                &mut ethernet,
                &mut sink,
            ),
            ConnectedRxDispatch::Ignored
        );
        assert_eq!(sink.probe_responses, 1);
    }

    fn config() -> ConnectedRxConfig {
        ConnectedRxConfig {
            station_address: STATION,
            bssid: BSSID,
            association_id: 7,
            ingress: RxIngressConfig {
                ring_entry_limit: 1,
                csi_config: 0,
                flags: 0,
            },
            security: WifiSecurityMode::Wpa2Personal,
            peer_qos: true,
        }
    }

    fn dispatcher() -> ConnectedRxDispatcher {
        let mut dispatcher = ConnectedRxDispatcher::new(config());
        dispatcher.install_ccmp_rx_replay(StaCcmpRxReplayEpoch::new([0; 8], 1, [0; 8]).unwrap());
        dispatcher
    }

    fn segment(storage: &[u8; 192], signal_length: usize) -> RxSegment<'_> {
        let received = FRAME_OFFSET + signal_length;
        RxSegment {
            descriptor_address: 0x2f00_4000,
            descriptor_word0: 192 | ((received as u32) << LENGTH_SHIFT) | BIT_30 | BIT_31,
            buffer: storage,
            next_descriptor_address: 0,
        }
    }

    fn large_segment(storage: &[u8; 256], signal_length: usize) -> RxSegment<'_> {
        RxSegment {
            descriptor_address: 0x2f00_4000,
            descriptor_word0: 256
                | (((FRAME_OFFSET + signal_length) as u32) << LENGTH_SHIFT)
                | BIT_30
                | BIT_31,
            buffer: storage,
            next_descriptor_address: 0,
        }
    }

    fn set_tail(storage: &mut [u8; 192], signal_length: usize) {
        // Synthetic connected frames are standalone MPDUs unless a test
        // explicitly overrides the hardware `cur_single_mpdu` bit.
        storage[0x1f] = 1;
        storage[TAIL_OFFSET..TAIL_OFFSET + 4].copy_from_slice(
            &(((signal_length + 4) as u32) << 16 | signal_length as u32).to_le_bytes(),
        );
    }

    fn open_fragment(
        storage: &mut [u8; 192],
        sequence: u16,
        fragment: u8,
        more_fragments: bool,
        retry: bool,
        source: [u8; 6],
        payload: &[u8],
    ) -> usize {
        let mpdu_length = 24 + payload.len();
        let signal_length = mpdu_length + 4;
        storage.fill(0);
        set_tail(storage, signal_length);
        let frame = &mut storage[FRAME_OFFSET..FRAME_OFFSET + mpdu_length];
        let mut frame_control = 0x0208_u16;
        if more_fragments {
            frame_control |= MORE_FRAGMENTS;
        }
        if retry {
            frame_control |= 0x0800;
        }
        frame[..2].copy_from_slice(&frame_control.to_le_bytes());
        frame[4..10].copy_from_slice(&STATION);
        frame[10..16].copy_from_slice(&BSSID);
        frame[16..22].copy_from_slice(&source);
        frame[22..24].copy_from_slice(&((sequence << 4) | u16::from(fragment)).to_le_bytes());
        frame[24..].copy_from_slice(payload);
        signal_length
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the test vector keeps every independently mutated 802.11/CCMP field explicit"
    )]
    fn protected_fragment(
        storage: &mut [u8; 192],
        sequence: u16,
        fragment: u8,
        more_fragments: bool,
        retry: bool,
        packet_number: u64,
        source: [u8; 6],
        payload: &[u8],
    ) -> usize {
        let mpdu_length = 24 + 8 + payload.len() + 8;
        let signal_length = mpdu_length + 4;
        storage.fill(0);
        set_tail(storage, signal_length);
        let frame = &mut storage[FRAME_OFFSET..FRAME_OFFSET + mpdu_length];
        let mut frame_control = 0x4208_u16;
        if more_fragments {
            frame_control |= MORE_FRAGMENTS;
        }
        if retry {
            frame_control |= 0x0800;
        }
        frame[..2].copy_from_slice(&frame_control.to_le_bytes());
        frame[4..10].copy_from_slice(&STATION);
        frame[10..16].copy_from_slice(&BSSID);
        frame[16..22].copy_from_slice(&source);
        frame[22..24].copy_from_slice(&((sequence << 4) | u16::from(fragment)).to_le_bytes());
        frame[24..32].copy_from_slice(
            &CcmpHeader::new(
                open_esp_radio_ieee80211::ccmp::CcmpPacketNumber::new(packet_number).unwrap(),
                CcmpKeyId::PAIRWISE,
            )
            .encode(),
        );
        frame[32..32 + payload.len()].copy_from_slice(payload);
        signal_length
    }

    #[test]
    fn open_station_reassembles_only_the_exact_fragment_identity() {
        let first_payload = [0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 1, 2];
        let final_payload = [3, 4, 5];
        let mut first_storage = [0_u8; 192];
        let first_signal = open_fragment(
            &mut first_storage,
            0x123,
            0,
            true,
            false,
            SOURCE,
            &first_payload,
        );
        let mut final_storage = [0_u8; 192];
        let final_signal = open_fragment(
            &mut final_storage,
            0x123,
            1,
            false,
            false,
            SOURCE,
            &final_payload,
        );
        let mut open = config();
        open.security = WifiSecurityMode::Open;
        open.peer_qos = false;
        let mut dispatcher = ConnectedRxDispatcher::new(open);
        let mut sink = RecordingSink::default();
        let mut mpdu = [0_u8; 128];
        let mut ethernet = [0_u8; 128];

        assert!(!dispatcher.may_publish_ethernet(segment(&first_storage, first_signal)));
        assert!(!dispatcher.may_complete_open_fragment(segment(&first_storage, first_signal)));
        assert_eq!(
            dispatcher.dispatch_with_runtime_received_at(
                segment(&first_storage, first_signal),
                &mut mpdu,
                &mut ethernet,
                Some(10),
                &mut sink,
            ),
            ConnectedRxDispatch::FragmentBuffered {
                expired: 0,
                evicted: false,
            }
        );
        assert!(sink.power_save_deliveries.is_empty());

        // A retried first fragment is a defragmenter duplicate and cannot
        // complete the one-shot PS-Poll delivery lane.
        first_storage[FRAME_OFFSET + 1] |= 0x08;
        assert_eq!(
            dispatcher.dispatch_with_runtime_received_at(
                segment(&first_storage, first_signal),
                &mut mpdu,
                &mut ethernet,
                Some(11),
                &mut sink,
            ),
            ConnectedRxDispatch::Duplicate
        );
        assert!(sink.power_save_deliveries.is_empty());

        // A retry cannot clear More Fragments and route its partial first
        // body through ordinary decapsulation while the exact sequence is
        // still retained.
        first_storage[FRAME_OFFSET + 1] &= !0x04;
        assert_eq!(
            dispatcher.dispatch_with_runtime_received_at(
                segment(&first_storage, first_signal),
                &mut mpdu,
                &mut ethernet,
                Some(11),
                &mut sink,
            ),
            ConnectedRxDispatch::Rejected {
                protection: ConnectedRxProtection::Pairwise,
                error: ConnectedRxError::Fragment(OpenDataFragmentError::MoreFragmentsMismatch),
            }
        );
        first_storage[FRAME_OFFSET + 1] &= !0x08;
        first_storage[FRAME_OFFSET + 1] |= 0x04;

        final_storage[FRAME_OFFSET + 16] ^= 1;
        assert_eq!(
            dispatcher.dispatch_with_runtime_received_at(
                segment(&final_storage, final_signal),
                &mut mpdu,
                &mut ethernet,
                Some(11),
                &mut sink,
            ),
            ConnectedRxDispatch::Rejected {
                protection: ConnectedRxProtection::Pairwise,
                error: ConnectedRxError::Fragment(OpenDataFragmentError::IdentityMismatch),
            }
        );
        assert!(sink.ethernet.is_empty());

        final_storage[FRAME_OFFSET + 16] ^= 1;
        assert!(!dispatcher.may_publish_ethernet(segment(&final_storage, final_signal)));
        assert!(dispatcher.may_complete_open_fragment(segment(&final_storage, final_signal)));
        assert_eq!(
            dispatcher.dispatch_with_runtime_received_at(
                segment(&final_storage, final_signal),
                &mut mpdu,
                &mut ethernet,
                Some(12),
                &mut sink,
            ),
            ConnectedRxDispatch::Data {
                ethernet_frames: 1,
                amsdu: false,
            }
        );
        assert_eq!(sink.ethernet.len(), 1);
        assert_eq!(&sink.ethernet[0][..6], &STATION);
        assert_eq!(&sink.ethernet[0][6..12], &SOURCE);
        assert_eq!(&sink.ethernet[0][12..14], &0x0800_u16.to_be_bytes());
        assert_eq!(&sink.ethernet[0][14..], &[1, 2, 3, 4, 5]);
        assert_eq!(
            sink.power_save_deliveries,
            [StaPsPollDelivery { more_data: false }]
        );

        let _ = dispatcher.dispatch_with_runtime_received_at(
            segment(&first_storage, first_signal),
            &mut mpdu,
            &mut ethernet,
            Some(20),
            &mut sink,
        );
        assert_eq!(dispatcher.clear_open_fragmentation(), 1);
    }

    #[test]
    fn station_ccmp_fragments_commit_each_pn_before_one_final_publication() {
        let first_payload = [0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 1, 2];
        let final_payload = [3, 4, 5];
        let mut first_storage = [0_u8; 192];
        let first_signal = protected_fragment(
            &mut first_storage,
            7,
            0,
            true,
            false,
            3,
            SOURCE,
            &first_payload,
        );
        let mut final_storage = [0_u8; 192];
        let final_signal = protected_fragment(
            &mut final_storage,
            7,
            1,
            false,
            false,
            4,
            SOURCE,
            &final_payload,
        );
        let mut dispatcher = dispatcher();
        let mut sink = RecordingSink::default();
        let mut mpdu = [0_u8; 128];
        let mut ethernet = [0_u8; 128];
        assert!(!dispatcher.may_publish_ethernet(segment(&first_storage, first_signal)));
        assert_eq!(
            dispatcher.dispatch_with_runtime_received_at(
                segment(&first_storage, first_signal),
                &mut mpdu,
                &mut ethernet,
                Some(1),
                &mut sink,
            ),
            ConnectedRxDispatch::FragmentBuffered {
                expired: 0,
                evicted: false,
            }
        );
        assert!(sink.ethernet.is_empty());

        first_storage[FRAME_OFFSET + 1] |= 0x08;
        assert_eq!(
            dispatcher.dispatch_with_runtime_received_at(
                segment(&first_storage, first_signal),
                &mut mpdu,
                &mut ethernet,
                Some(2),
                &mut sink,
            ),
            ConnectedRxDispatch::Duplicate
        );

        first_storage[FRAME_OFFSET + 24..FRAME_OFFSET + 32].copy_from_slice(
            &CcmpHeader::new(
                open_esp_radio_ieee80211::ccmp::CcmpPacketNumber::new(4).unwrap(),
                CcmpKeyId::PAIRWISE,
            )
            .encode(),
        );
        assert_eq!(
            dispatcher.dispatch_with_runtime_received_at(
                segment(&first_storage, first_signal),
                &mut mpdu,
                &mut ethernet,
                Some(3),
                &mut sink,
            ),
            ConnectedRxDispatch::Rejected {
                protection: ConnectedRxProtection::Pairwise,
                error: ConnectedRxError::Fragment(
                    OpenDataFragmentError::RetryPacketNumberMismatch {
                        fragment_number: 0,
                        expected: open_esp_radio_ieee80211::ccmp::CcmpPacketNumber::new(3).unwrap(),
                        observed: open_esp_radio_ieee80211::ccmp::CcmpPacketNumber::new(4).unwrap(),
                    }
                ),
            }
        );
        first_storage[FRAME_OFFSET + 24..FRAME_OFFSET + 32].copy_from_slice(
            &CcmpHeader::new(
                open_esp_radio_ieee80211::ccmp::CcmpPacketNumber::new(3).unwrap(),
                CcmpKeyId::PAIRWISE,
            )
            .encode(),
        );
        first_storage[FRAME_OFFSET + 32] ^= 1;
        assert_eq!(
            dispatcher.dispatch_with_runtime_received_at(
                segment(&first_storage, first_signal),
                &mut mpdu,
                &mut ethernet,
                Some(4),
                &mut sink,
            ),
            ConnectedRxDispatch::Rejected {
                protection: ConnectedRxProtection::Pairwise,
                error: ConnectedRxError::Fragment(OpenDataFragmentError::RetryPayloadMismatch {
                    fragment_number: 0
                }),
            }
        );
        assert!(dispatcher.may_complete_fragment(segment(&final_storage, final_signal)));
        assert_eq!(
            dispatcher.dispatch_with_runtime_received_at(
                segment(&final_storage, final_signal),
                &mut mpdu,
                &mut ethernet,
                Some(5),
                &mut sink,
            ),
            ConnectedRxDispatch::Data {
                ethernet_frames: 1,
                amsdu: false,
            }
        );
        assert_eq!(sink.ethernet.len(), 1);
        assert_eq!(&sink.ethernet[0][14..], &[1, 2, 3, 4, 5]);
        assert_eq!(
            sink.power_save_deliveries,
            [StaPsPollDelivery { more_data: false }]
        );

        final_storage[FRAME_OFFSET + 1] |= 0x08;
        assert_eq!(
            dispatcher.dispatch_with_runtime_received_at(
                segment(&final_storage, final_signal),
                &mut mpdu,
                &mut ethernet,
                Some(6),
                &mut sink,
            ),
            ConnectedRxDispatch::Duplicate
        );
        assert_eq!(sink.ethernet.len(), 1);
        let Some(StaCcmpRxReplayOwner::Owned(replay)) = dispatcher.ccmp_replay.as_ref() else {
            panic!("test dispatcher owns one replay epoch")
        };
        assert_eq!(
            replay.pairwise.highest(CcmpReplayLane::NonQos),
            open_esp_radio_ieee80211::ccmp::CcmpPacketNumber::new(4)
        );
    }

    #[test]
    fn protected_retry_cannot_turn_an_ordinary_mpdu_into_a_fragment_train() {
        let payload = [0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 1];
        let mut ordinary_storage = [0_u8; 192];
        let ordinary_signal = protected_fragment(
            &mut ordinary_storage,
            7,
            0,
            false,
            false,
            3,
            SOURCE,
            &payload,
        );
        let mut retry_first_storage = [0_u8; 192];
        let retry_first_signal = protected_fragment(
            &mut retry_first_storage,
            7,
            0,
            true,
            true,
            4,
            SOURCE,
            &payload,
        );
        let mut colliding_final_storage = [0_u8; 192];
        let colliding_final_signal = protected_fragment(
            &mut colliding_final_storage,
            7,
            1,
            false,
            false,
            5,
            SOURCE,
            &[2],
        );
        let mut new_first_storage = [0_u8; 192];
        let new_first_signal = protected_fragment(
            &mut new_first_storage,
            8,
            0,
            true,
            true,
            4,
            SOURCE,
            &payload,
        );
        let mut new_final_storage = [0_u8; 192];
        let new_final_signal =
            protected_fragment(&mut new_final_storage, 8, 1, false, false, 5, SOURCE, &[2]);
        let mut dispatcher = dispatcher();
        let mut sink = RecordingSink::default();
        let mut mpdu = [0_u8; 128];
        let mut ethernet = [0_u8; 128];

        assert_eq!(
            dispatcher.dispatch_with_runtime_received_at(
                segment(&ordinary_storage, ordinary_signal),
                &mut mpdu,
                &mut ethernet,
                Some(1),
                &mut sink,
            ),
            ConnectedRxDispatch::Data {
                ethernet_frames: 1,
                amsdu: false,
            }
        );
        assert_eq!(sink.ethernet.len(), 1);

        assert_eq!(
            dispatcher.dispatch_with_runtime_received_at(
                segment(&retry_first_storage, retry_first_signal),
                &mut mpdu,
                &mut ethernet,
                Some(2),
                &mut sink,
            ),
            ConnectedRxDispatch::Duplicate
        );
        assert_eq!(dispatcher.fragments.active_contexts(), 0);
        let Some(StaCcmpRxReplayOwner::Owned(replay)) = dispatcher.ccmp_replay.as_ref() else {
            panic!("test dispatcher owns one replay epoch")
        };
        assert_eq!(
            replay.pairwise.highest(CcmpReplayLane::NonQos),
            open_esp_radio_ieee80211::ccmp::CcmpPacketNumber::new(3),
            "duplicate admission must precede replay commit"
        );

        assert_eq!(
            dispatcher.dispatch_with_runtime_received_at(
                segment(&colliding_final_storage, colliding_final_signal),
                &mut mpdu,
                &mut ethernet,
                Some(3),
                &mut sink,
            ),
            ConnectedRxDispatch::Rejected {
                protection: ConnectedRxProtection::Pairwise,
                error: ConnectedRxError::Fragment(OpenDataFragmentError::Orphan {
                    fragment_number: 1,
                }),
            }
        );
        assert_eq!(dispatcher.fragments.active_contexts(), 0);
        assert_eq!(sink.ethernet.len(), 1);

        // Retry is not itself a rejection: a fragment-zero sequence absent
        // from ordinary history still starts and completes a normal train.
        assert_eq!(
            dispatcher.dispatch_with_runtime_received_at(
                segment(&new_first_storage, new_first_signal),
                &mut mpdu,
                &mut ethernet,
                Some(4),
                &mut sink,
            ),
            ConnectedRxDispatch::FragmentBuffered {
                expired: 0,
                evicted: false,
            }
        );
        assert_eq!(
            dispatcher.dispatch_with_runtime_received_at(
                segment(&new_final_storage, new_final_signal),
                &mut mpdu,
                &mut ethernet,
                Some(5),
                &mut sink,
            ),
            ConnectedRxDispatch::Data {
                ethernet_frames: 1,
                amsdu: false,
            }
        );
        assert_eq!(sink.ethernet.len(), 2);
        let Some(StaCcmpRxReplayOwner::Owned(replay)) = dispatcher.ccmp_replay.as_ref() else {
            panic!("test dispatcher owns one replay epoch")
        };
        assert_eq!(
            replay.pairwise.highest(CcmpReplayLane::NonQos),
            open_esp_radio_ieee80211::ccmp::CcmpPacketNumber::new(5)
        );
    }

    #[test]
    fn replay_rejection_cannot_evict_two_durable_ccmp_fragment_trains() {
        let first_payload = [0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 1];
        let mut first_a = [0_u8; 192];
        let signal_a =
            protected_fragment(&mut first_a, 10, 0, true, false, 3, SOURCE, &first_payload);
        let mut first_b = [0_u8; 192];
        let signal_b =
            protected_fragment(&mut first_b, 11, 0, true, false, 4, SOURCE, &first_payload);
        let mut replayed = [0_u8; 192];
        let replayed_signal =
            protected_fragment(&mut replayed, 12, 0, true, false, 4, SOURCE, &first_payload);
        let mut final_a = [0_u8; 192];
        let final_signal = protected_fragment(&mut final_a, 10, 1, false, false, 5, SOURCE, &[2]);
        let mut dispatcher = dispatcher();
        let mut sink = RecordingSink::default();
        let mut mpdu = [0_u8; 128];
        let mut ethernet = [0_u8; 128];

        for (storage, signal, now) in [(&first_a, signal_a, 1), (&first_b, signal_b, 2)] {
            assert!(matches!(
                dispatcher.dispatch_with_runtime_received_at(
                    segment(storage, signal),
                    &mut mpdu,
                    &mut ethernet,
                    Some(now),
                    &mut sink,
                ),
                ConnectedRxDispatch::FragmentBuffered { .. }
            ));
        }
        assert_eq!(dispatcher.fragments.active_contexts(), 2);
        let pn4 = open_esp_radio_ieee80211::ccmp::CcmpPacketNumber::new(4).unwrap();
        assert_eq!(
            dispatcher.dispatch_with_runtime_received_at(
                segment(&replayed, replayed_signal),
                &mut mpdu,
                &mut ethernet,
                Some(3),
                &mut sink,
            ),
            ConnectedRxDispatch::Rejected {
                protection: ConnectedRxProtection::Pairwise,
                error: ConnectedRxError::CcmpReplay(StaCcmpRxReplayError::Replay(
                    CcmpReplayError::Replayed {
                        packet_number: pn4,
                        highest: pn4,
                    }
                )),
            }
        );
        assert_eq!(dispatcher.fragments.active_contexts(), 2);
        assert_eq!(
            dispatcher.dispatch_with_runtime_received_at(
                segment(&final_a, final_signal),
                &mut mpdu,
                &mut ethernet,
                Some(4),
                &mut sink,
            ),
            ConnectedRxDispatch::Data {
                ethernet_frames: 1,
                amsdu: false,
            }
        );
        assert_eq!(dispatcher.fragments.active_contexts(), 1);
        assert_eq!(sink.ethernet.len(), 1);
    }

    #[test]
    fn open_retry_cannot_turn_an_ordinary_mpdu_into_a_fragment_train() {
        let payload = [0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 1];
        let mut ordinary_storage = [0_u8; 192];
        let ordinary_signal =
            open_fragment(&mut ordinary_storage, 7, 0, false, false, SOURCE, &payload);
        let mut open = config();
        open.security = WifiSecurityMode::Open;
        open.peer_qos = false;
        let mut dispatcher = ConnectedRxDispatcher::new(open);
        let mut sink = RecordingSink::default();
        let mut mpdu = [0_u8; 128];
        let mut ethernet = [0_u8; 128];

        assert_eq!(
            dispatcher.dispatch_with_runtime_received_at(
                segment(&ordinary_storage, ordinary_signal),
                &mut mpdu,
                &mut ethernet,
                Some(1),
                &mut sink,
            ),
            ConnectedRxDispatch::Data {
                ethernet_frames: 1,
                amsdu: false,
            }
        );

        ordinary_storage[FRAME_OFFSET + 1] |= 0x04 | 0x08;
        assert_eq!(
            dispatcher.dispatch_with_runtime_received_at(
                segment(&ordinary_storage, ordinary_signal),
                &mut mpdu,
                &mut ethernet,
                Some(2),
                &mut sink,
            ),
            ConnectedRxDispatch::Duplicate
        );
        assert_eq!(dispatcher.clear_open_fragmentation(), 0);

        let mut final_storage = [0_u8; 192];
        let final_signal = open_fragment(&mut final_storage, 7, 1, false, false, SOURCE, &[2]);
        assert_eq!(
            dispatcher.dispatch_with_runtime_received_at(
                segment(&final_storage, final_signal),
                &mut mpdu,
                &mut ethernet,
                Some(3),
                &mut sink,
            ),
            ConnectedRxDispatch::Rejected {
                protection: ConnectedRxProtection::Pairwise,
                error: ConnectedRxError::Fragment(OpenDataFragmentError::Orphan {
                    fragment_number: 1,
                }),
            }
        );

        let mut invalid_first_storage = [0_u8; 192];
        let invalid_first_signal = open_fragment(
            &mut invalid_first_storage,
            8,
            0,
            true,
            false,
            SOURCE,
            &[0; 9],
        );
        assert_eq!(
            dispatcher.dispatch_with_runtime_received_at(
                segment(&invalid_first_storage, invalid_first_signal),
                &mut mpdu,
                &mut ethernet,
                Some(4),
                &mut sink,
            ),
            ConnectedRxDispatch::FragmentBuffered {
                expired: 0,
                evicted: false,
            }
        );
        let mut invalid_final_storage = [0_u8; 192];
        let invalid_final_signal =
            open_fragment(&mut invalid_final_storage, 8, 1, false, false, SOURCE, &[2]);
        assert_eq!(
            dispatcher.dispatch_with_runtime_received_at(
                segment(&invalid_final_storage, invalid_final_signal),
                &mut mpdu,
                &mut ethernet,
                Some(5),
                &mut sink,
            ),
            ConnectedRxDispatch::Rejected {
                protection: ConnectedRxProtection::Pairwise,
                error: ConnectedRxError::Fragment(OpenDataFragmentError::InvalidLlcSnap),
            }
        );

        invalid_first_storage[FRAME_OFFSET + 1] |= 0x08;
        assert_eq!(
            dispatcher.dispatch_with_runtime_received_at(
                segment(&invalid_first_storage, invalid_first_signal),
                &mut mpdu,
                &mut ethernet,
                Some(6),
                &mut sink,
            ),
            ConnectedRxDispatch::FragmentBuffered {
                expired: 0,
                evicted: false,
            },
            "failed fragment trains do not poison ordinary duplicate history"
        );
        assert_eq!(dispatcher.clear_open_fragmentation(), 1);
        assert_eq!(sink.ethernet.len(), 1);
    }

    #[test]
    fn reconfigure_revokes_duplicate_and_fragment_history() {
        let payload = [0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 1];
        let mut ordinary = [0_u8; 192];
        let ordinary_signal = open_fragment(&mut ordinary, 7, 0, false, false, SOURCE, &payload);
        let mut fragment = [0_u8; 192];
        let fragment_signal = open_fragment(&mut fragment, 8, 0, true, false, SOURCE, &payload);
        let mut open = config();
        open.security = WifiSecurityMode::Open;
        open.peer_qos = false;
        let mut dispatcher = ConnectedRxDispatcher::new(open);
        let mut sink = RecordingSink::default();
        let mut mpdu = [0_u8; 128];
        let mut ethernet = [0_u8; 128];

        assert!(matches!(
            dispatcher.dispatch(
                segment(&ordinary, ordinary_signal),
                &mut mpdu,
                &mut ethernet,
                &mut sink,
            ),
            ConnectedRxDispatch::Data { .. }
        ));
        ordinary[FRAME_OFFSET + 1] |= 0x08;
        assert_eq!(
            dispatcher.dispatch(
                segment(&ordinary, ordinary_signal),
                &mut mpdu,
                &mut ethernet,
                &mut sink,
            ),
            ConnectedRxDispatch::Duplicate
        );
        assert!(matches!(
            dispatcher.dispatch_with_runtime_received_at(
                segment(&fragment, fragment_signal),
                &mut mpdu,
                &mut ethernet,
                Some(1),
                &mut sink,
            ),
            ConnectedRxDispatch::FragmentBuffered { .. }
        ));
        assert_eq!(dispatcher.fragments.active_contexts(), 1);

        dispatcher.try_reconfigure(open).unwrap();
        assert_eq!(dispatcher.fragments.active_contexts(), 0);
        assert!(matches!(
            dispatcher.dispatch(
                segment(&ordinary, ordinary_signal),
                &mut mpdu,
                &mut ethernet,
                &mut sink,
            ),
            ConnectedRxDispatch::Data { .. }
        ));
    }

    #[test]
    fn reconfigure_refuses_an_in_flight_shared_replay_publication() {
        let (rx, mut control) = replay_resource();
        let original = config();
        let mut replacement = original;
        replacement.bssid = SOURCE;
        let mut dispatcher = ConnectedRxDispatcher::new(original);
        dispatcher.install_shared_ccmp_rx_replay(rx);
        let prepared = prepare_ccmp_replay(
            &mut dispatcher.ccmp_replay,
            ConnectedRxProtection::Group,
            None,
            ccmp_header(1, 1),
        )
        .unwrap();
        let publication = commit_ccmp_replay(&mut dispatcher.ccmp_replay, prepared)
            .unwrap()
            .expect("shared replay prepares one publication permit");

        assert_eq!(
            dispatcher.try_reconfigure(replacement),
            Err(StaCcmpRxReplayError::PublicationInFlight)
        );
        assert_eq!(dispatcher.config(), original);
        assert!(dispatcher.ccmp_rx_replay_enabled());

        drop(publication);
        dispatcher.try_reconfigure(replacement).unwrap();
        assert_eq!(dispatcher.config(), replacement);
        assert!(!dispatcher.ccmp_rx_replay_enabled());
        control.stop().unwrap();
    }

    #[test]
    fn dispatches_protected_ethernet_and_owns_duplicate_history() {
        const HEADER: usize = 24;
        const PAYLOAD: [u8; 4] = [1, 2, 3, 4];
        const MPDU: usize = HEADER + 8 + 8 + PAYLOAD.len() + 8;
        const SIGNAL: usize = MPDU + 4;
        let mut storage = [0_u8; 192];
        set_tail(&mut storage, SIGNAL);
        let frame = &mut storage[FRAME_OFFSET..FRAME_OFFSET + MPDU];
        frame[0] = 0x08;
        frame[1] = 0x42;
        frame[4..10].copy_from_slice(&STATION);
        frame[10..16].copy_from_slice(&BSSID);
        frame[16..22].copy_from_slice(&SOURCE);
        frame[22..24].copy_from_slice(&0x1230_u16.to_le_bytes());
        frame[HEADER..HEADER + 8].copy_from_slice(&[3, 0, 0, 0x20, 0, 0, 0, 0]);
        frame[HEADER + 8..HEADER + 16].copy_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x00]);
        frame[HEADER + 16..HEADER + 20].copy_from_slice(&PAYLOAD);

        let mut sink = RecordingSink::default();
        let mut mpdu = [0_u8; 128];
        let mut ethernet = [0_u8; 128];
        let mut missing_replay = ConnectedRxDispatcher::new(config());
        assert_eq!(
            missing_replay.dispatch(
                segment(&storage, SIGNAL),
                &mut mpdu,
                &mut ethernet,
                &mut sink,
            ),
            ConnectedRxDispatch::Rejected {
                protection: ConnectedRxProtection::Pairwise,
                error: ConnectedRxError::CcmpReplay(StaCcmpRxReplayError::OwnerUnavailable),
            }
        );
        assert!(sink.ethernet.is_empty());

        let mut dispatcher = dispatcher();
        assert!(!dispatcher.may_publish_amsdu(segment(&storage, SIGNAL)));
        assert_eq!(
            dispatcher.dispatch(
                segment(&storage, SIGNAL),
                &mut mpdu,
                &mut ethernet,
                &mut sink,
            ),
            ConnectedRxDispatch::Data {
                ethernet_frames: 1,
                amsdu: false,
            }
        );
        assert_eq!(sink.ethernet.len(), 1);
        assert_eq!(&sink.ethernet[0][..6], &STATION);
        assert_eq!(&sink.ethernet[0][6..12], &SOURCE);
        assert_eq!(&sink.ethernet[0][12..14], &0x0800_u16.to_be_bytes());
        assert_eq!(&sink.ethernet[0][14..], &PAYLOAD);
        assert_eq!(
            sink.ethernet_metadata[0].crypto,
            MacRxEvidence::HardwareObserved(MacRxCryptoStatus::DecryptedAndIntegrityVerified)
        );
        assert_eq!(
            sink.ethernet_metadata[0].amsdu,
            MacRxEvidence::ProtocolValidated(false)
        );
        assert_eq!(
            sink.ethernet_metadata[0].s_mpdu,
            MacRxEvidence::HardwareObserved(true)
        );
        assert_eq!(
            sink.ethernet_metadata[0].ampdu,
            MacRxEvidence::ProtocolValidated(false)
        );

        storage[FRAME_OFFSET + 1] |= 0x08;
        let replay = dispatcher.dispatch(
            segment(&storage, SIGNAL),
            &mut mpdu,
            &mut ethernet,
            &mut sink,
        );
        assert!(matches!(
            replay,
            ConnectedRxDispatch::Rejected {
                protection: ConnectedRxProtection::Pairwise,
                error: ConnectedRxError::CcmpReplay(StaCcmpRxReplayError::Replay(
                    CcmpReplayError::Replayed {
                        packet_number,
                        highest,
                    }
                )),
            } if packet_number.value() == 3 && highest.value() == 3
        ));
    }

    #[test]
    fn wpa2_admits_only_plaintext_eapol_from_the_exact_associated_link() {
        const HEADER: usize = 24;
        let message3 = open_esp_radio_wpa2::frames::Wpa2TxFrame::<512>::message3(
            STATION, 2, [4; 32], [0; 8], &[0x55; 8],
        )
        .unwrap();
        let mpdu_length = HEADER + 8 + message3.as_bytes().len();
        let signal_length = mpdu_length + 4;
        let mut storage = [0_u8; 256];
        storage[0x1f] = 1;
        storage[TAIL_OFFSET..TAIL_OFFSET + 4].copy_from_slice(
            &(((signal_length + 4) as u32) << 16 | signal_length as u32).to_le_bytes(),
        );
        let frame = &mut storage[FRAME_OFFSET..FRAME_OFFSET + mpdu_length];
        frame[..2].copy_from_slice(&0x0208_u16.to_le_bytes());
        frame[4..10].copy_from_slice(&STATION);
        frame[10..16].copy_from_slice(&BSSID);
        frame[16..22].copy_from_slice(&BSSID);
        frame[HEADER..HEADER + 8].copy_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x88, 0x8e]);
        frame[HEADER + 8..].copy_from_slice(message3.as_bytes());
        let mut dispatcher = dispatcher();
        let mut sink = RecordingSink::default();
        let mut mpdu = [0_u8; 192];
        let mut ethernet = [0_u8; 192];
        assert!(!dispatcher.may_publish_ethernet(large_segment(&storage, signal_length)));
        assert_eq!(
            dispatcher.dispatch(
                large_segment(&storage, signal_length),
                &mut mpdu,
                &mut ethernet,
                &mut sink,
            ),
            ConnectedRxDispatch::UnprotectedEapol
        );
        assert_eq!(sink.unprotected_eapol, [message3.as_bytes()]);
        assert!(sink.ethernet.is_empty());

        storage[FRAME_OFFSET + 16] ^= 1;
        assert_eq!(
            dispatcher.dispatch(
                large_segment(&storage, signal_length),
                &mut mpdu,
                &mut ethernet,
                &mut sink,
            ),
            ConnectedRxDispatch::Rejected {
                protection: ConnectedRxProtection::Pairwise,
                error: ConnectedRxError::SecurityModeMismatch,
            }
        );
        assert_eq!(sink.unprotected_eapol.len(), 1);

        storage[FRAME_OFFSET + 16] ^= 1;
        storage[FRAME_OFFSET + HEADER + 6..FRAME_OFFSET + HEADER + 8]
            .copy_from_slice(&0x0800_u16.to_be_bytes());
        assert_eq!(
            dispatcher.dispatch(
                large_segment(&storage, signal_length),
                &mut mpdu,
                &mut ethernet,
                &mut sink,
            ),
            ConnectedRxDispatch::Rejected {
                protection: ConnectedRxProtection::Pairwise,
                error: ConnectedRxError::SecurityModeMismatch,
            }
        );
        assert_eq!(sink.unprotected_eapol.len(), 1);

        // Open associations keep their ordinary plaintext data semantics;
        // the special EAPOL lane exists only for an installed WPA2 epoch.
        storage[FRAME_OFFSET + HEADER + 6..FRAME_OFFSET + HEADER + 8]
            .copy_from_slice(&0x888e_u16.to_be_bytes());
        let mut open = config();
        open.security = WifiSecurityMode::Open;
        open.peer_qos = false;
        let mut open_dispatcher = ConnectedRxDispatcher::new(open);
        let mut open_sink = RecordingSink::default();
        assert_eq!(
            open_dispatcher.dispatch(
                large_segment(&storage, signal_length),
                &mut mpdu,
                &mut ethernet,
                &mut open_sink,
            ),
            ConnectedRxDispatch::Data {
                ethernet_frames: 1,
                amsdu: false,
            }
        );
        assert_eq!(open_sink.ethernet.len(), 1);
        assert!(open_sink.unprotected_eapol.is_empty());
    }

    #[test]
    fn preflight_detects_amsdu_without_mutating_dispatch_state() {
        const HEADER: usize = 26;
        const FIRST_SUBFRAME: usize = 24;
        const SECOND_SUBFRAME: usize = 25;
        const MPDU: usize = HEADER + 8 + FIRST_SUBFRAME + SECOND_SUBFRAME + 8;
        const SIGNAL: usize = MPDU + 4;
        let mut storage = [0_u8; 192];
        set_tail(&mut storage, SIGNAL);
        let frame = &mut storage[FRAME_OFFSET..FRAME_OFFSET + MPDU];
        frame[0] = 0x88;
        frame[1] = 0x42;
        frame[4..10].copy_from_slice(&STATION);
        frame[10..16].copy_from_slice(&BSSID);
        frame[16..22].copy_from_slice(&SOURCE);
        frame[22..24].copy_from_slice(&0x1230_u16.to_le_bytes());
        frame[24] = 0x80;
        frame[HEADER..HEADER + 8].copy_from_slice(&[3, 0, 0, 0x20, 0, 0, 0, 0]);
        let mut offset = HEADER + 8;
        frame[offset..offset + 6].copy_from_slice(&STATION);
        frame[offset + 6..offset + 12].copy_from_slice(&SOURCE);
        frame[offset + 12..offset + 14].copy_from_slice(&10_u16.to_be_bytes());
        frame[offset + 14..offset + 22].copy_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x00]);
        frame[offset + 22..offset + 24].copy_from_slice(&[1, 2]);
        offset += FIRST_SUBFRAME;
        frame[offset..offset + 6].copy_from_slice(&[0xff; 6]);
        frame[offset + 6..offset + 12].copy_from_slice(&SOURCE);
        frame[offset + 12..offset + 14].copy_from_slice(&11_u16.to_be_bytes());
        frame[offset + 14..offset + 22].copy_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x06]);
        frame[offset + 22..offset + 25].copy_from_slice(&[3, 4, 5]);

        let mut dispatcher = dispatcher();
        let segment = segment(&storage, SIGNAL);
        assert_eq!(
            dispatcher.reorder_key(segment),
            Some(RxBlockAckMpduKey {
                peer: BSSID,
                tid: 0,
                sequence: 0x123,
                retry: false,
            })
        );
        assert!(dispatcher.may_publish_amsdu(segment));
        // Preflight is repeatable and does not claim duplicate history.
        assert!(dispatcher.may_publish_amsdu(segment));

        let mut sink = RecordingSink::default();
        let mut mpdu = [0_u8; 128];
        let mut ethernet = [0_u8; 128];
        assert_eq!(
            dispatcher.dispatch(segment, &mut mpdu, &mut ethernet, &mut sink),
            ConnectedRxDispatch::Data {
                ethernet_frames: 2,
                amsdu: true,
            }
        );
        assert_eq!(sink.ethernet.len(), 2);
        assert!(sink.ethernet_metadata.iter().all(|metadata| {
            metadata.amsdu == MacRxEvidence::ProtocolValidated(true)
                && metadata.s_mpdu == MacRxEvidence::HardwareObserved(true)
                && metadata.ampdu == MacRxEvidence::ProtocolValidated(false)
                && metadata.crypto
                    == MacRxEvidence::HardwareObserved(
                        MacRxCryptoStatus::DecryptedAndIntegrityVerified,
                    )
        }));
    }

    #[test]
    fn routes_an_addressed_block_ack_action_without_platform_effects() {
        const BODY: [u8; 9] = [3, 0, 9, 0x02, 0x08, 0, 0, 0x30, 0x12];
        const MPDU: usize = 24 + BODY.len();
        const SIGNAL: usize = MPDU + 4;
        let mut storage = [0_u8; 192];
        set_tail(&mut storage, SIGNAL);
        let frame = &mut storage[FRAME_OFFSET..FRAME_OFFSET + MPDU];
        frame[0] = 0xd0;
        frame[4..10].copy_from_slice(&STATION);
        frame[10..16].copy_from_slice(&BSSID);
        frame[16..22].copy_from_slice(&BSSID);
        frame[24..].copy_from_slice(&BODY);

        let mut dispatcher = ConnectedRxDispatcher::new(config());
        let mut sink = RecordingSink::default();
        let mut mpdu = [0_u8; 128];
        let mut ethernet = [0_u8; 128];
        assert_eq!(
            dispatcher.dispatch(
                segment(&storage, SIGNAL),
                &mut mpdu,
                &mut ethernet,
                &mut sink,
            ),
            ConnectedRxDispatch::BlockAck
        );
        assert_eq!(sink.block_ack.len(), 1);
        assert!(matches!(
            sink.block_ack[0],
            BlockAckAction::AddbaRequest {
                dialog_token: 9,
                tid: 0,
                immediate: true,
                window: 32,
                starting_sequence: 0x123,
                ..
            }
        ));
    }

    #[test]
    fn routes_only_peer_disconnects_addressed_to_this_station() {
        const MPDU: usize = 26;
        const SIGNAL: usize = MPDU + 4;
        let mut storage = [0_u8; 192];
        set_tail(&mut storage, SIGNAL);
        let frame = &mut storage[FRAME_OFFSET..FRAME_OFFSET + MPDU];
        frame[..2].copy_from_slice(&DEAUTHENTICATION_FRAME_CONTROL.to_le_bytes());
        frame[4..10].copy_from_slice(&STATION);
        frame[10..16].copy_from_slice(&BSSID);
        frame[16..22].copy_from_slice(&BSSID);
        frame[24..26].copy_from_slice(&7_u16.to_le_bytes());

        let mut dispatcher = ConnectedRxDispatcher::new(config());
        let mut sink = RecordingSink::default();
        let mut mpdu = [0_u8; 128];
        let mut ethernet = [0_u8; 128];
        assert_eq!(
            dispatcher.dispatch(
                segment(&storage, SIGNAL),
                &mut mpdu,
                &mut ethernet,
                &mut sink,
            ),
            ConnectedRxDispatch::PeerDisconnect
        );
        assert_eq!(
            sink.peer_disconnects,
            [StaDisconnect {
                kind: open_esp_radio_ieee80211::station::StaDisconnectKind::Deauthentication,
                reason_code: 7,
            }]
        );

        storage[FRAME_OFFSET + 4..FRAME_OFFSET + 10].copy_from_slice(&SOURCE);
        assert_eq!(
            dispatcher.dispatch(
                segment(&storage, SIGNAL),
                &mut mpdu,
                &mut ethernet,
                &mut sink,
            ),
            ConnectedRxDispatch::Ignored
        );
        assert_eq!(sink.peer_disconnects.len(), 1);
    }
}
