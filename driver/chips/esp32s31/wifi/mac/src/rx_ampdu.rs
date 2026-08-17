//! Allocation-free receive BlockAck reorder state in the live MAC crate.
//!
//! The vendor implementation allocates one 40-byte agreement object and a
//! variable pointer array for every receive TID. This module keeps negotiation
//! state and the data-plane reorder engine as separate owned values. The
//! integration layer binds the latter to its exact staging-pool capacity, and
//! raw packet pointers stay outside both state machines.

use crate::{
    MacInterface,
    rx::PUBLIC_HEADER_SIZE,
    rx_ampdu_hw::{S31_RX_BLOCK_ACK_MAX_TID, S31RxBlockAckAgreement},
};

// SOURCE: complete `libnet80211.a[ieee80211_ht.o]::
// ampdu_rx_start.constprop.0`. The vendor agreement owner selects the smaller
// of the peer request and `g_wifi_menuconfig+0x30`, whose normal configured
// value is 64. Its later ordinary activation path passes the constant 64 to
// `ic_add_rx_ba`; complete `libpp.a[hal_ampdu.o]::
// hal_agreement_add_rx_ba` publishes that value in the seven-bit hardware
// window. Keep the protocol/reorder window independent of the number of RX DMA
// descriptors: descriptors are recycled while this sequence window remains
// active. A reduced-memory build can still select eight explicitly.
#[cfg(feature = "rx-ba-window-8")]
pub const RX_BLOCK_ACK_MAX_WINDOW: u16 = 8;
#[cfg(not(feature = "rx-ba-window-8"))]
pub const RX_BLOCK_ACK_MAX_WINDOW: u16 = 64;
pub const RX_AMPDU_SLOT_CAPACITY: usize = RX_BLOCK_ACK_MAX_WINDOW as usize;
#[cfg(feature = "large-rx-pool-48")]
pub(crate) const RX_ESF_SLOT_ID_CAPACITY: usize = 48;
#[cfg(not(feature = "large-rx-pool-48"))]
pub(crate) const RX_ESF_SLOT_ID_CAPACITY: usize = 32;
// Rare multi-descriptor MPDUs use a separate split SRAM/PSRAM pool. Its
// IDs participate in reorder ownership, but must not inflate the ordinary
// zero-copy channel budget computed from `RX_ESF_SLOT_ID_CAPACITY`.
pub const RX_REORDER_SLOT_ID_CAPACITY: usize = RX_ESF_SLOT_ID_CAPACITY + 2;
const SEQUENCE_MASK: u16 = 0x0fff;
const SEQUENCE_HALF_RANGE: u16 = 0x0800;
const DATA_TYPE: u16 = 0x0008;
const DATA_TYPE_MASK: u16 = 0x000c;
const PROTECTED: u16 = 0x4000;
const QOS_SUBTYPE: u16 = 0x0080;
const RETRY: u16 = 0x0800;
const TO_FROM_DS: u16 = 0x0300;

/// Public ordering identity of one protected QoS data MPDU.
///
/// These fields precede the encrypted body. Both station and access-point
/// receive paths use this exact classifier before deciding whether an active
/// agreement owns the frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxBlockAckMpduKey {
    pub peer: [u8; 6],
    pub tid: u8,
    pub sequence: u16,
    pub retry: bool,
}

/// Extract the role-neutral infrastructure RX BlockAck key.
///
/// `local_address` is the receiver address (Address 1). A connected STA also
/// supplies `expected_peer`; an AP leaves it absent and validates admission in
/// its peer table after extraction. Group, foreign, unprotected, non-QoS and
/// fragmented frames do not enter a reorder sequence space.
pub fn rx_block_ack_mpdu_key(
    raw: &[u8],
    local_address: [u8; 6],
    expected_peer: Option<[u8; 6]>,
) -> Option<RxBlockAckMpduKey> {
    let frame_offset = PUBLIC_HEADER_SIZE;
    let frame_control = u16::from_le_bytes([*raw.get(frame_offset)?, *raw.get(frame_offset + 1)?]);
    if frame_control & (DATA_TYPE_MASK | PROTECTED | QOS_SUBTYPE)
        != DATA_TYPE | PROTECTED | QOS_SUBTYPE
        || raw.get(frame_offset + 4..frame_offset + 10)? != local_address
    {
        return None;
    }
    let peer: [u8; 6] = raw
        .get(frame_offset + 10..frame_offset + 16)?
        .try_into()
        .ok()?;
    if expected_peer.is_some_and(|expected| expected != peer) {
        return None;
    }
    let sequence_control =
        u16::from_le_bytes([*raw.get(frame_offset + 22)?, *raw.get(frame_offset + 23)?]);
    if sequence_control & 0x000f != 0 {
        return None;
    }
    let qos_offset = frame_offset + 24 + usize::from(frame_control & TO_FROM_DS == TO_FROM_DS) * 6;
    Some(RxBlockAckMpduKey {
        peer,
        tid: *raw.get(qos_offset)? & 0x0f,
        sequence: sequence_control >> 4,
        retry: frame_control & RETRY != 0,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxAmpduMpdu {
    pub sequence: u16,
    pub slot: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxAmpduError {
    InvalidWindow(u16),
    InvalidSequence(u16),
    InvalidSlot(u8),
    InvalidHardwareBank(u8),
    DuplicateSequence(u16),
    SlotAlreadyOwned(u8),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxAddbaResponseError {
    InvalidBodyLength(usize),
    InvalidTid(u8),
    InvalidWindow(u16),
}

pub const RX_BLOCK_ACK_BANK_COUNT: usize = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxBlockAckSessionsError {
    DelayedPolicyUnsupported,
    InvalidTid(u8),
    InvalidWindow(u16),
    NonzeroTimeout(u16),
    InvalidStartingSequence(u16),
    ActivationBusy,
    StaleActivation,
    NoFreePendingSlot,
    NoFreePeerSlot,
    NoFreeHardwareBank,
    Response(RxAddbaResponseError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PendingRxBlockAck {
    requested_window: u16,
    starting_sequence: u16,
    dialog_token: u8,
    tid: u8,
    peer_index: u8,
}

impl PendingRxBlockAck {
    const EMPTY_PEER_INDEX: u8 = u8::MAX;
    const EMPTY: Self = Self {
        requested_window: 0,
        starting_sequence: 0,
        dialog_token: 0,
        tid: 0,
        peer_index: Self::EMPTY_PEER_INDEX,
    };

    const fn occupied(&self) -> bool {
        self.peer_index != Self::EMPTY_PEER_INDEX
    }
}

/// One validated-boundary request to establish peer-to-local RX ordering.
///
/// Parsing remains owned by the portable 802.11 layer. This value binds the
/// parsed action to its authenticated link peer before the request crosses
/// into hardware-bank ownership.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxBlockAckRequest {
    pub peer: [u8; 6],
    pub dialog_token: u8,
    pub tid: u8,
    pub immediate: bool,
    pub requested_window: u16,
    pub timeout_tu: u16,
    pub starting_sequence: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxBlockAckSnapshot {
    pub hardware_index: u8,
    pub peer: [u8; 6],
    pub tid: u8,
    pub window: u16,
    pub starting_sequence: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxBlockAckIdentity {
    pub hardware_index: u8,
    pub peer: [u8; 6],
    pub tid: u8,
}

impl RxBlockAckSnapshot {
    pub const fn identity(self) -> RxBlockAckIdentity {
        RxBlockAckIdentity {
            hardware_index: self.hardware_index,
            peer: self.peer,
            tid: self.tid,
        }
    }
}

/// Semantic agreement edge from BlockAck control to the independent reorder
/// owner. Hardware-bank identity is explicit so an AP can host equal TIDs for
/// different peers without merging their sequence spaces.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxReorderCommand {
    Start(RxBlockAckSnapshot),
    Stop(RxBlockAckIdentity),
    StopAll,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxReorderCommandError {
    Full(RxReorderCommand),
}

#[derive(Clone, Copy)]
struct ActiveRxBlockAck {
    window: u16,
    starting_sequence: u16,
    peer_index: u8,
    tid: u8,
}

impl ActiveRxBlockAck {
    const EMPTY: Self = Self {
        window: 0,
        starting_sequence: 0,
        peer_index: PendingRxBlockAck::EMPTY_PEER_INDEX,
        tid: 0,
    };

    const fn occupied(&self) -> bool {
        self.peer_index != PendingRxBlockAck::EMPTY_PEER_INDEX
    }
}

/// One private activation transaction spanning software and hardware state.
///
/// A caller cannot construct or clone this value. It must either return it to
/// [`RxBlockAckSessions::commit`] after programming the hardware and
/// transmitting the response, or to [`RxBlockAckSessions::cancel`] after
/// undoing any hardware publication.
pub struct RxBlockAckActivation {
    generation: u32,
    hardware: S31RxBlockAckAgreement,
    response_body: [u8; 9],
    negotiated: RxBlockAckSnapshot,
    replaced: Option<RxBlockAckSnapshot>,
}

impl RxBlockAckActivation {
    pub const fn hardware(&self) -> S31RxBlockAckAgreement {
        self.hardware
    }

    pub const fn response_body(&self) -> &[u8; 9] {
        &self.response_body
    }

    pub const fn negotiated(&self) -> RxBlockAckSnapshot {
        self.negotiated
    }

    pub const fn replaced(&self) -> Option<RxBlockAckSnapshot> {
        self.replaced
    }
}

/// Fixed owner of the eight ordinary S31 receive BlockAck banks.
///
/// SOURCE: complete `libnet80211.a[ieee80211_ht.o]::
/// ampdu_rx_start.constprop.0` stores one independent agreement per peer/TID and
/// bounds the negotiated software window by the configured maximum. Complete
/// `libpp.a[hal_ampdu.o]::hal_agreement_add_rx_ba` owns eight
/// hardware banks and receives the literal hardware window 64 on the ordinary
/// receive path. The protocol window and hardware window therefore remain
/// deliberately distinct here.
pub struct RxBlockAckSessions<const PEER_CAPACITY: usize = 1> {
    peers: [Option<[u8; 6]>; PEER_CAPACITY],
    pending: [PendingRxBlockAck; RX_BLOCK_ACK_BANK_COUNT],
    active: [ActiveRxBlockAck; RX_BLOCK_ACK_BANK_COUNT],
    generation: u32,
    in_flight: Option<u32>,
    maximum_window: u16,
}

impl<const PEER_CAPACITY: usize> RxBlockAckSessions<PEER_CAPACITY> {
    pub fn new() -> Self {
        assert!(
            PEER_CAPACITY != 0,
            "RX BlockAck peer table must not be empty"
        );
        assert!(
            PEER_CAPACITY <= usize::from(u8::MAX),
            "RX BlockAck peer index must fit its compact owner"
        );
        Self {
            peers: [None; PEER_CAPACITY],
            pending: [PendingRxBlockAck::EMPTY; RX_BLOCK_ACK_BANK_COUNT],
            active: [ActiveRxBlockAck::EMPTY; RX_BLOCK_ACK_BANK_COUNT],
            generation: 0,
            in_flight: None,
            maximum_window: RX_BLOCK_ACK_MAX_WINDOW,
        }
    }

    /// Construct an agreement owner with an integration-qualified reorder
    /// limit no wider than the vendor maximum.
    ///
    /// The composition layer uses this when its independent staging pool must
    /// retain credits beyond the negotiated reorder window. Hardware still
    /// receives its separately qualified 64-entry agreement geometry.
    pub fn with_maximum_window(maximum_window: u16) -> Result<Self, RxBlockAckSessionsError> {
        if maximum_window == 0 || maximum_window > RX_BLOCK_ACK_MAX_WINDOW {
            return Err(RxBlockAckSessionsError::InvalidWindow(maximum_window));
        }
        Ok(Self {
            maximum_window,
            ..Self::new()
        })
    }

    pub const fn maximum_window(&self) -> u16 {
        self.maximum_window
    }

    /// Clear every peer, pending request and active agreement while retaining
    /// the integration-owned negotiated-window limit.
    ///
    /// Role transitions reuse the statically allocated session owner. The
    /// resource profile, rather than the role implementation, owns how much
    /// downstream reorder capacity is available; resetting an AP epoch must
    /// therefore not silently restore the vendor maximum.
    pub fn reset(&mut self) {
        let maximum_window = self.maximum_window;
        *self = Self {
            maximum_window,
            ..Self::new()
        };
    }

    /// Admit one parsed immediate ADDBA request into the shared pending bank.
    /// A newer request for the same peer/TID replaces the older unexecuted
    /// request without consuming another hardware-bank candidate.
    pub fn offer(&mut self, request: RxBlockAckRequest) -> Result<(), RxBlockAckSessionsError> {
        let RxBlockAckRequest {
            peer,
            dialog_token,
            tid,
            immediate,
            requested_window,
            timeout_tu,
            starting_sequence,
        } = request;
        if !immediate {
            return Err(RxBlockAckSessionsError::DelayedPolicyUnsupported);
        }
        if tid > S31_RX_BLOCK_ACK_MAX_TID {
            return Err(RxBlockAckSessionsError::InvalidTid(tid));
        }
        if requested_window == 0 || requested_window > 0x03ff {
            return Err(RxBlockAckSessionsError::InvalidWindow(requested_window));
        }
        if timeout_tu != 0 {
            return Err(RxBlockAckSessionsError::NonzeroTimeout(timeout_tu));
        }
        if starting_sequence > 0x0fff {
            return Err(RxBlockAckSessionsError::InvalidStartingSequence(
                starting_sequence,
            ));
        }
        self.reclaim_unused_peers();
        let existing_peer_index = self.peer_index(peer);
        let pending_index = self
            .pending
            .iter()
            .position(|pending| {
                existing_peer_index.is_some_and(|peer_index| {
                    pending.occupied()
                        && usize::from(pending.peer_index) == peer_index
                        && pending.tid == tid
                })
            })
            .or_else(|| self.pending.iter().position(|pending| !pending.occupied()))
            .ok_or(RxBlockAckSessionsError::NoFreePendingSlot)?;
        let peer_index = match existing_peer_index {
            Some(index) => index,
            None => self
                .peers
                .iter()
                .position(Option::is_none)
                .ok_or(RxBlockAckSessionsError::NoFreePeerSlot)?,
        };
        self.peers[peer_index] = Some(peer);
        self.pending[pending_index] = PendingRxBlockAck {
            peer_index: peer_index as u8,
            dialog_token,
            tid,
            requested_window,
            starting_sequence,
        };
        Ok(())
    }

    /// Begin the oldest pending request by bank index and reserve its hardware
    /// bank until the caller commits or cancels the returned transaction.
    pub fn begin_pending(
        &mut self,
        interface: MacInterface,
    ) -> Result<Option<RxBlockAckActivation>, RxBlockAckSessionsError> {
        if self.in_flight.is_some() {
            return Err(RxBlockAckSessionsError::ActivationBusy);
        }
        let Some((pending_index, request)) = self
            .pending
            .iter()
            .enumerate()
            .find_map(|(index, request)| request.occupied().then_some((index, *request)))
        else {
            return Ok(None);
        };
        let hardware_index = self
            .active
            .iter()
            .position(|agreement| {
                agreement.occupied()
                    && agreement.peer_index == request.peer_index
                    && agreement.tid == request.tid
            })
            .or_else(|| {
                self.active
                    .iter()
                    .position(|agreement| !agreement.occupied())
            })
            .ok_or(RxBlockAckSessionsError::NoFreeHardwareBank)?;
        // Keep the parsed request pending when all physical banks are busy.
        // A later peer teardown can free a bank without requiring the peer to
        // retransmit its action frame merely because local control scheduling
        // reached this edge first.
        self.pending[pending_index] = PendingRxBlockAck::EMPTY;
        let replaced = self.snapshot_at(hardware_index);
        self.active[hardware_index] = ActiveRxBlockAck::EMPTY;
        let peer = self.peers[usize::from(request.peer_index)]
            .expect("occupied request owns one peer-table entry");
        let window = request.requested_window.min(self.maximum_window);
        let mut response_body = [0_u8; 9];
        write_successful_addba_response(
            &mut response_body,
            request.dialog_token,
            request.tid,
            window,
        )
        .map_err(RxBlockAckSessionsError::Response)?;

        self.generation = next_rx_block_ack_generation(self.generation);
        self.in_flight = Some(self.generation);
        let negotiated = RxBlockAckSnapshot {
            hardware_index: hardware_index as u8,
            peer,
            tid: request.tid,
            window,
            starting_sequence: request.starting_sequence,
        };
        Ok(Some(RxBlockAckActivation {
            generation: self.generation,
            hardware: S31RxBlockAckAgreement {
                hardware_index: hardware_index as u8,
                interface,
                peer,
                tid: request.tid,
                starting_sequence: request.starting_sequence,
                // The vendor ordinary receive hardware leaf receives 64 even
                // when the negotiated/reorder window is smaller.
                window: RX_BLOCK_ACK_MAX_WINDOW,
            },
            response_body,
            negotiated,
            replaced,
        }))
    }

    pub fn commit(
        &mut self,
        activation: RxBlockAckActivation,
    ) -> Result<RxBlockAckSnapshot, RxBlockAckSessionsError> {
        if self.in_flight != Some(activation.generation) {
            return Err(RxBlockAckSessionsError::StaleActivation);
        }
        self.in_flight = None;
        let snapshot = activation.negotiated;
        let peer_index = self
            .peer_index(snapshot.peer)
            .expect("activation peer remains retained until commit");
        self.active[usize::from(snapshot.hardware_index)] = ActiveRxBlockAck {
            window: snapshot.window,
            starting_sequence: snapshot.starting_sequence,
            peer_index: peer_index as u8,
            tid: snapshot.tid,
        };
        self.reclaim_unused_peers();
        Ok(snapshot)
    }

    pub fn cancel(
        &mut self,
        activation: RxBlockAckActivation,
    ) -> Result<(), RxBlockAckSessionsError> {
        if self.in_flight != Some(activation.generation) {
            return Err(RxBlockAckSessionsError::StaleActivation);
        }
        self.in_flight = None;
        self.reclaim_unused_peers();
        Ok(())
    }

    /// Remove the software owner before the caller clears the returned bank.
    pub fn stop(&mut self, peer: [u8; 6], tid: u8) -> Option<RxBlockAckSnapshot> {
        let peer_index = self.peer_index(peer)?;
        let index = self.active.iter().position(|agreement| {
            agreement.occupied()
                && usize::from(agreement.peer_index) == peer_index
                && agreement.tid == tid
        })?;
        let snapshot = self.snapshot_at(index);
        self.active[index] = ActiveRxBlockAck::EMPTY;
        self.reclaim_unused_peers();
        snapshot
    }

    /// Remove an unexecuted request after the caller has explicitly declined
    /// it on air. Active agreement state is deliberately untouched.
    pub fn discard_pending(&mut self, peer: [u8; 6], tid: u8) -> bool {
        let Some(peer_index) = self.peer_index(peer) else {
            return false;
        };
        let mut discarded = false;
        for pending in &mut self.pending {
            if pending.occupied()
                && usize::from(pending.peer_index) == peer_index
                && pending.tid == tid
            {
                *pending = PendingRxBlockAck::EMPTY;
                discarded = true;
            }
        }
        self.reclaim_unused_peers();
        discarded
    }

    /// Remove every agreement owned by one peer before clearing the returned
    /// hardware banks. The fixed result preserves bank identity and needs no
    /// allocation in AP peer teardown.
    pub fn stop_peer(
        &mut self,
        peer: [u8; 6],
    ) -> [Option<RxBlockAckSnapshot>; RX_BLOCK_ACK_BANK_COUNT] {
        let Some(peer_index) = self.peer_index(peer) else {
            return [None; RX_BLOCK_ACK_BANK_COUNT];
        };
        for pending in &mut self.pending {
            if pending.occupied() && usize::from(pending.peer_index) == peer_index {
                *pending = PendingRxBlockAck::EMPTY;
            }
        }
        let stopped = core::array::from_fn(|index| {
            if self.active[index].occupied()
                && usize::from(self.active[index].peer_index) == peer_index
            {
                let snapshot = self.snapshot_at(index);
                self.active[index] = ActiveRxBlockAck::EMPTY;
                snapshot
            } else {
                None
            }
        });
        self.reclaim_unused_peers();
        stopped
    }

    pub fn snapshots(&self) -> [Option<RxBlockAckSnapshot>; RX_BLOCK_ACK_BANK_COUNT] {
        core::array::from_fn(|index| self.snapshot_at(index))
    }

    fn peer_index(&self, peer: [u8; 6]) -> Option<usize> {
        self.peers.iter().position(|entry| *entry == Some(peer))
    }

    fn snapshot_at(&self, hardware_index: usize) -> Option<RxBlockAckSnapshot> {
        let agreement = self.active[hardware_index];
        if !agreement.occupied() {
            return None;
        }
        Some(RxBlockAckSnapshot {
            hardware_index: hardware_index as u8,
            peer: self.peers[usize::from(agreement.peer_index)]
                .expect("occupied agreement owns one peer-table entry"),
            tid: agreement.tid,
            window: agreement.window,
            starting_sequence: agreement.starting_sequence,
        })
    }

    fn reclaim_unused_peers(&mut self) {
        if self.in_flight.is_some() {
            return;
        }
        for peer_index in 0..self.peers.len() {
            let referenced =
                self.pending.iter().any(|pending| {
                    pending.occupied() && usize::from(pending.peer_index) == peer_index
                }) || self.active.iter().any(|agreement| {
                    agreement.occupied() && usize::from(agreement.peer_index) == peer_index
                });
            if !referenced {
                self.peers[peer_index] = None;
            }
        }
    }
}

impl<const PEER_CAPACITY: usize> Default for RxBlockAckSessions<PEER_CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

/// Role-neutral owner of the eight software reorder sequence spaces.
///
/// Hardware-bank identity and its reorder state cannot be updated separately.
/// Frame backing and gap timers deliberately remain outside this type because
/// they belong to the executor/integration memory policy.
pub struct RxBlockAckReorderBanks<const SLOT_CAPACITY: usize> {
    identities: [Option<RxBlockAckIdentity>; RX_BLOCK_ACK_BANK_COUNT],
    states: [Option<RxBlockAckReorderState<SLOT_CAPACITY>>; RX_BLOCK_ACK_BANK_COUNT],
}

impl<const SLOT_CAPACITY: usize> RxBlockAckReorderBanks<SLOT_CAPACITY> {
    pub const fn new() -> Self {
        Self {
            identities: [None; RX_BLOCK_ACK_BANK_COUNT],
            states: [const { None }; RX_BLOCK_ACK_BANK_COUNT],
        }
    }

    pub fn find(&self, peer: [u8; 6], tid: u8) -> Option<usize> {
        self.identities
            .iter()
            .enumerate()
            .find_map(|(bank, identity)| {
                identity
                    .is_some_and(|identity| identity.peer == peer && identity.tid == tid)
                    .then_some(bank)
            })
    }

    pub const fn identity(&self, bank: usize) -> Option<RxBlockAckIdentity> {
        self.identities[bank]
    }

    pub const fn state(&self, bank: usize) -> Option<&RxBlockAckReorderState<SLOT_CAPACITY>> {
        self.states[bank].as_ref()
    }

    pub const fn state_mut(
        &mut self,
        bank: usize,
    ) -> Option<&mut RxBlockAckReorderState<SLOT_CAPACITY>> {
        self.states[bank].as_mut()
    }

    /// Replace the sequence space assigned to the negotiated physical bank.
    /// Any frames retained by the previous generation are returned to their
    /// integration owner before the new identity becomes observable.
    pub fn start(
        &mut self,
        agreement: RxBlockAckSnapshot,
    ) -> Result<Option<RxAmpduRelease>, RxAmpduError> {
        let bank = usize::from(agreement.hardware_index);
        if bank >= RX_BLOCK_ACK_BANK_COUNT {
            return Err(RxAmpduError::InvalidHardwareBank(agreement.hardware_index));
        }
        let state = RxBlockAckReorderState::new(agreement.starting_sequence, agreement.window)?;
        let released = self.stop_bank(bank);
        self.identities[bank] = Some(agreement.identity());
        self.states[bank] = Some(state);
        Ok(released)
    }

    /// Stop only the exact agreement generation identified by control-plane
    /// state. A stale DELBA/rollback cannot tear down a replacement.
    pub fn stop(&mut self, identity: RxBlockAckIdentity) -> Option<RxAmpduRelease> {
        let bank = usize::from(identity.hardware_index);
        if bank >= RX_BLOCK_ACK_BANK_COUNT || self.identities[bank] != Some(identity) {
            return None;
        }
        self.stop_bank(bank)
    }

    pub fn stop_bank(&mut self, bank: usize) -> Option<RxAmpduRelease> {
        self.identities[bank] = None;
        self.states[bank].take().map(|mut state| state.stop())
    }

    pub fn occupied(&self) -> u32 {
        self.states
            .iter()
            .flatten()
            .map(RxBlockAckReorderState::occupied)
            .sum()
    }
}

impl<const SLOT_CAPACITY: usize> Default for RxBlockAckReorderBanks<SLOT_CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

const fn next_rx_block_ack_generation(current: u32) -> u32 {
    let next = current.wrapping_add(1);
    if next == 0 { 1 } else { next }
}

/// Write one successful immediate BlockAck response into an owned action body.
///
/// A-MSDU is deliberately not advertised by the initial strict RX path.
pub fn write_successful_addba_response(
    body: &mut [u8],
    dialog_token: u8,
    tid: u8,
    window: u16,
) -> Result<(), RxAddbaResponseError> {
    if body.len() != 9 {
        return Err(RxAddbaResponseError::InvalidBodyLength(body.len()));
    }
    if tid > 15 {
        return Err(RxAddbaResponseError::InvalidTid(tid));
    }
    if window == 0 || window > 0x03ff {
        return Err(RxAddbaResponseError::InvalidWindow(window));
    }
    body.fill(0);
    body[0] = crate::tx_ampdu::BLOCK_ACK_CATEGORY;
    body[1] = crate::tx_ampdu::ADDBA_RESPONSE_ACTION;
    body[2] = dialog_token;
    let parameters = 1_u16 << 1 | u16::from(tid) << 2 | window << 6;
    body[5..7].copy_from_slice(&parameters.to_le_bytes());
    Ok(())
}

/// IEEE 802.11 status 37: the recipient cannot accept this agreement.
pub const ADDBA_STATUS_REQUEST_DECLINED: u16 = 37;

/// Write a finite rejection for a syntactically parsed ADDBA request.
///
/// The request fields remain evidence in the response, but status owns the
/// semantic outcome. No software session or hardware bank may accompany this
/// body.
pub fn write_declined_addba_response(
    body: &mut [u8],
    dialog_token: u8,
    tid: u8,
    requested_window: u16,
) -> Result<(), RxAddbaResponseError> {
    if body.len() != 9 {
        return Err(RxAddbaResponseError::InvalidBodyLength(body.len()));
    }
    if tid > 15 {
        return Err(RxAddbaResponseError::InvalidTid(tid));
    }
    if requested_window > 0x03ff {
        return Err(RxAddbaResponseError::InvalidWindow(requested_window));
    }
    body.fill(0);
    body[0] = crate::tx_ampdu::BLOCK_ACK_CATEGORY;
    body[1] = crate::tx_ampdu::ADDBA_RESPONSE_ACTION;
    body[2] = dialog_token;
    body[3..5].copy_from_slice(&ADDBA_STATUS_REQUEST_DECLINED.to_le_bytes());
    let parameters = 1_u16 << 1 | u16::from(tid) << 2 | requested_window << 6;
    body[5..7].copy_from_slice(&parameters.to_le_bytes());
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxAmpduRelease {
    pub frames: [Option<RxAmpduMpdu>; RX_AMPDU_SLOT_CAPACITY],
    pub count: u8,
    pub missing: u16,
    pub rejected: Option<RxAmpduMpdu>,
    pub buffered: bool,
}

impl RxAmpduRelease {
    const fn empty() -> Self {
        Self {
            frames: [None; RX_AMPDU_SLOT_CAPACITY],
            count: 0,
            missing: 0,
            rejected: None,
            buffered: false,
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = RxAmpduMpdu> + '_ {
        self.frames[..usize::from(self.count)]
            .iter()
            .copied()
            .flatten()
    }

    fn push(&mut self, frame: RxAmpduMpdu) {
        let index = usize::from(self.count);
        debug_assert!(index < self.frames.len());
        self.frames[index] = Some(frame);
        self.count += 1;
    }
}

/// One receive BlockAck agreement with a fixed maximum sequence window and an
/// explicit integration-owned frame-token domain.
///
/// Every retained packet is represented only by a checked slot index. The
/// owner maps that index to its independent frame storage and recycles every frame
/// returned by `ingest`, `expire_gap`, or `stop`.
pub struct RxBlockAckReorderState<const SLOT_CAPACITY: usize> {
    starting_sequence: u16,
    next_sequence: u16,
    window: u16,
    occupied: u64,
    frames: [Option<RxAmpduMpdu>; RX_AMPDU_SLOT_CAPACITY],
}

impl<const SLOT_CAPACITY: usize> RxBlockAckReorderState<SLOT_CAPACITY> {
    pub const fn new(starting_sequence: u16, window: u16) -> Result<Self, RxAmpduError> {
        if window == 0 || window > RX_BLOCK_ACK_MAX_WINDOW {
            return Err(RxAmpduError::InvalidWindow(window));
        }
        if starting_sequence > SEQUENCE_MASK {
            return Err(RxAmpduError::InvalidSequence(starting_sequence));
        }
        Ok(Self {
            starting_sequence,
            next_sequence: starting_sequence,
            window,
            occupied: 0,
            frames: [None; RX_AMPDU_SLOT_CAPACITY],
        })
    }

    pub const fn next_sequence(&self) -> u16 {
        self.next_sequence
    }

    pub const fn window(&self) -> u16 {
        self.window
    }

    pub const fn occupied(&self) -> u32 {
        self.occupied.count_ones()
    }

    /// Rebase the one-shot first-A-MPDU sequence mismatch handled by the
    /// vendor reorder path.
    ///
    /// A newly negotiated agreement normally accepts the first aggregate in
    /// the forward half of its sequence space. If that aggregate is instead
    /// behind the current software frontier, the vendor releases all retained
    /// frames and moves both the software and hardware windows. HT and older
    /// formats first fall back to the negotiated SSN; only an aggregate that
    /// is also behind that SSN moves the window to include its received
    /// sequence. Newer formats use the received sequence directly.
    pub fn resynchronize_stale_initial_ampdu(
        &mut self,
        sequence: u16,
        use_received_sequence: bool,
    ) -> Result<Option<(RxAmpduRelease, u16)>, RxAmpduError> {
        if sequence > SEQUENCE_MASK {
            return Err(RxAmpduError::InvalidSequence(sequence));
        }
        if forward_distance(self.next_sequence, sequence) < SEQUENCE_HALF_RANGE {
            return Ok(None);
        }

        let mut next_sequence = self.starting_sequence;
        if use_received_sequence
            || forward_distance(self.starting_sequence, sequence) >= SEQUENCE_HALF_RANGE
        {
            next_sequence = wrapping_sequence(sequence, 1_u16.wrapping_sub(self.window));
            if use_received_sequence {
                next_sequence = sequence;
            }
        }

        let released = self.stop();
        self.next_sequence = next_sequence;
        Ok(Some((released, next_sequence)))
    }

    /// Decide whether this sequence will remain owned after a successful
    /// [`Self::ingest`].
    ///
    /// The next expected sequence is released immediately. Every forward
    /// sequence has at least that first gap in front of it, including after a
    /// bounded window advance, and must therefore receive persistent backing.
    /// A stale frame is rejected by `ingest` and needs no retained owner.
    pub fn retains_on_ingest(&self, sequence: u16) -> Result<bool, RxAmpduError> {
        if sequence > SEQUENCE_MASK {
            return Err(RxAmpduError::InvalidSequence(sequence));
        }
        let distance = forward_distance(self.next_sequence, sequence);
        if distance >= SEQUENCE_HALF_RANGE {
            return Ok(false);
        }
        if distance < self.window && self.has_sequence(sequence) {
            return Err(RxAmpduError::DuplicateSequence(sequence));
        }
        let distance_after_advance = if distance >= self.window {
            self.window - 1
        } else {
            distance
        };
        Ok(distance_after_advance != 0)
    }

    pub fn ingest(&mut self, frame: RxAmpduMpdu) -> Result<RxAmpduRelease, RxAmpduError> {
        if frame.sequence > SEQUENCE_MASK {
            return Err(RxAmpduError::InvalidSequence(frame.sequence));
        }
        if SLOT_CAPACITY == 0
            || SLOT_CAPACITY > usize::from(u8::MAX) + 1
            || usize::from(frame.slot) >= SLOT_CAPACITY
        {
            return Err(RxAmpduError::InvalidSlot(frame.slot));
        }
        if self
            .frames
            .iter()
            .flatten()
            .any(|owned| owned.slot == frame.slot)
        {
            return Err(RxAmpduError::SlotAlreadyOwned(frame.slot));
        }

        let mut release = RxAmpduRelease::empty();
        let distance = forward_distance(self.next_sequence, frame.sequence);
        if distance >= SEQUENCE_HALF_RANGE {
            release.rejected = Some(frame);
            return Ok(release);
        }
        if distance >= self.window {
            let advance = distance - self.window + 1;
            self.advance(advance, &mut release);
        }

        let index = slot_index(frame.sequence);
        if self.occupied & (1_u64 << index) != 0 {
            return Err(RxAmpduError::DuplicateSequence(frame.sequence));
        }
        self.frames[index] = Some(frame);
        self.occupied |= 1_u64 << index;
        self.release_contiguous(&mut release);
        release.buffered = self.occupied & (1_u64 << index) != 0;
        Ok(release)
    }

    /// Release the first buffered run after an async reorder-age timer edge.
    ///
    /// The scan is bounded by the negotiated window. It never reads time or
    /// waits; the async owner decides when this edge is due.
    pub fn expire_gap(&mut self) -> RxAmpduRelease {
        let mut release = RxAmpduRelease::empty();
        let mut distance = 0_u16;
        while distance < self.window {
            let sequence = wrapping_sequence(self.next_sequence, distance);
            if self.has_sequence(sequence) {
                self.advance(distance, &mut release);
                self.release_contiguous(&mut release);
                return release;
            }
            distance += 1;
        }
        release
    }

    pub fn stop(&mut self) -> RxAmpduRelease {
        let mut release = RxAmpduRelease::empty();
        let mut distance = 0_u16;
        while distance < self.window {
            let sequence = wrapping_sequence(self.next_sequence, distance);
            if let Some(frame) = self.take_sequence(sequence) {
                release.push(frame);
            }
            distance += 1;
        }
        self.occupied = 0;
        release
    }

    fn advance(&mut self, count: u16, release: &mut RxAmpduRelease) {
        let retained_span = count.min(self.window);
        let released_before = release.count;
        let mut offset = 0_u16;
        while offset < retained_span {
            let sequence = wrapping_sequence(self.next_sequence, offset);
            if let Some(frame) = self.take_sequence(sequence) {
                release.push(frame);
            }
            offset += 1;
        }
        let released_while_advancing = release.count - released_before;
        release.missing = release
            .missing
            .saturating_add(count.saturating_sub(u16::from(released_while_advancing)));
        self.next_sequence = wrapping_sequence(self.next_sequence, count);
    }

    fn release_contiguous(&mut self, release: &mut RxAmpduRelease) {
        let mut count = 0_u16;
        while count < self.window {
            let Some(frame) = self.take_sequence(self.next_sequence) else {
                break;
            };
            release.push(frame);
            self.next_sequence = wrapping_sequence(self.next_sequence, 1);
            count += 1;
        }
    }

    fn has_sequence(&self, sequence: u16) -> bool {
        let index = slot_index(sequence);
        self.occupied & (1_u64 << index) != 0
            && self.frames[index].is_some_and(|frame| frame.sequence == sequence)
    }

    fn take_sequence(&mut self, sequence: u16) -> Option<RxAmpduMpdu> {
        let index = slot_index(sequence);
        if !self.has_sequence(sequence) {
            return None;
        }
        self.occupied &= !(1_u64 << index);
        self.frames[index].take()
    }
}

/// Reorder state bound to the MAC crate's default receive-slot domain.
///
/// Integrations whose staging pool has a different compile-time size use
/// [`RxBlockAckReorderState`] directly.
pub type RxBlockAckReorder = RxBlockAckReorderState<RX_REORDER_SLOT_ID_CAPACITY>;

const fn forward_distance(from: u16, to: u16) -> u16 {
    to.wrapping_sub(from) & SEQUENCE_MASK
}

const fn wrapping_sequence(sequence: u16, increment: u16) -> u16 {
    sequence.wrapping_add(increment) & SEQUENCE_MASK
}

const fn slot_index(sequence: u16) -> usize {
    sequence as usize % RX_AMPDU_SLOT_CAPACITY
}

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! rx_request {
        ($peer:expr, $token:expr, $tid:expr, $immediate:expr, $window:expr, $timeout:expr, $start:expr) => {
            RxBlockAckRequest {
                peer: $peer,
                dialog_token: $token,
                tid: $tid,
                immediate: $immediate,
                requested_window: $window,
                timeout_tu: $timeout,
                starting_sequence: $start,
            }
        };
    }

    fn frame(sequence: u16, slot: u8) -> RxAmpduMpdu {
        RxAmpduMpdu { sequence, slot }
    }

    #[test]
    fn station_and_access_point_share_one_public_reorder_classifier() {
        let local = [2, 0, 0, 0, 0, 1];
        let peer = [2, 0, 0, 0, 0, 2];
        let mut raw = [0_u8; PUBLIC_HEADER_SIZE + 32];
        let frame = &mut raw[PUBLIC_HEADER_SIZE..];
        frame[..2].copy_from_slice(&0x4188_u16.to_le_bytes());
        frame[4..10].copy_from_slice(&local);
        frame[10..16].copy_from_slice(&peer);
        frame[22..24].copy_from_slice(&0x1230_u16.to_le_bytes());
        frame[24] = 5;
        let expected = RxBlockAckMpduKey {
            peer,
            tid: 5,
            sequence: 0x123,
            retry: false,
        };

        assert_eq!(
            rx_block_ack_mpdu_key(&raw, local, Some(peer)),
            Some(expected)
        );
        assert_eq!(rx_block_ack_mpdu_key(&raw, local, None), Some(expected));
        assert_eq!(rx_block_ack_mpdu_key(&raw, local, Some([3; 6])), None);

        raw[PUBLIC_HEADER_SIZE + 22] |= 1;
        assert_eq!(rx_block_ack_mpdu_key(&raw, local, None), None);
    }

    #[test]
    fn reset_clears_sessions_without_widening_the_integration_limit() {
        let peer = [2, 0, 0, 0, 0, 1];
        let mut sessions = RxBlockAckSessions::<1>::with_maximum_window(16).unwrap();
        sessions
            .offer(rx_request!(peer, 7, 0, true, 64, 0, 10))
            .unwrap();
        let activation = sessions
            .begin_pending(MacInterface::AccessPoint)
            .unwrap()
            .unwrap();
        sessions.commit(activation).unwrap();
        assert!(sessions.snapshots().iter().any(Option::is_some));

        sessions.reset();

        assert_eq!(sessions.maximum_window(), 16);
        assert!(sessions.snapshots().iter().all(Option::is_none));
        sessions
            .offer(rx_request!(peer, 8, 0, true, 64, 0, 20))
            .unwrap();
        let activation = sessions
            .begin_pending(MacInterface::AccessPoint)
            .unwrap()
            .unwrap();
        assert_eq!(activation.negotiated().window, 16);
        assert_eq!(activation.hardware().window, RX_BLOCK_ACK_MAX_WINDOW);
    }

    #[test]
    fn in_order_frames_are_released_immediately() {
        let mut reorder = RxBlockAckReorder::new(10, RX_BLOCK_ACK_MAX_WINDOW).unwrap();
        assert_eq!(reorder.retains_on_ingest(10), Ok(false));
        let release = reorder.ingest(frame(10, 0)).unwrap();
        assert_eq!(release.iter().collect::<std::vec::Vec<_>>(), [frame(10, 0)]);
        assert_eq!(reorder.next_sequence(), 11);
        assert_eq!(reorder.occupied(), 0);
    }

    #[test]
    fn gap_is_buffered_and_then_released_in_sequence_order() {
        let mut reorder = RxBlockAckReorder::new(100, RX_BLOCK_ACK_MAX_WINDOW).unwrap();
        assert_eq!(reorder.retains_on_ingest(102), Ok(true));
        assert!(reorder.ingest(frame(102, 2)).unwrap().buffered);
        assert_eq!(reorder.retains_on_ingest(101), Ok(true));
        assert!(reorder.ingest(frame(101, 1)).unwrap().buffered);
        assert_eq!(reorder.retains_on_ingest(100), Ok(false));
        let release = reorder.ingest(frame(100, 0)).unwrap();
        assert_eq!(
            release.iter().collect::<std::vec::Vec<_>>(),
            [frame(100, 0), frame(101, 1), frame(102, 2)]
        );
        assert_eq!(reorder.next_sequence(), 103);
    }

    #[test]
    #[cfg(not(feature = "rx-ba-window-8"))]
    fn window_advance_releases_owned_frames_and_counts_missing_without_long_loop() {
        let mut reorder = RxBlockAckReorder::new(0, RX_BLOCK_ACK_MAX_WINDOW).unwrap();
        reorder.ingest(frame(2, 2)).unwrap();
        reorder.ingest(frame(31, 31)).unwrap();
        let release = reorder.ingest(frame(1000, 1)).unwrap();
        assert_eq!(
            release.iter().collect::<std::vec::Vec<_>>(),
            [frame(2, 2), frame(31, 31)]
        );
        let expected_advance = 1000 - RX_BLOCK_ACK_MAX_WINDOW + 1;
        assert_eq!(release.missing, expected_advance - 2);
        assert!(release.buffered);
        assert_eq!(reorder.next_sequence(), expected_advance);
    }

    #[test]
    fn async_expiry_skips_only_the_current_gap() {
        let mut reorder = RxBlockAckReorder::new(20, 8).unwrap();
        reorder.ingest(frame(22, 2)).unwrap();
        reorder.ingest(frame(23, 3)).unwrap();
        let release = reorder.expire_gap();
        assert_eq!(release.missing, 2);
        assert_eq!(
            release.iter().collect::<std::vec::Vec<_>>(),
            [frame(22, 2), frame(23, 3)]
        );
        assert_eq!(reorder.next_sequence(), 24);
    }

    #[test]
    fn sequence_wrap_and_stale_rejection_are_unambiguous() {
        let mut reorder = RxBlockAckReorder::new(0x0fff, 8).unwrap();
        assert_eq!(reorder.retains_on_ingest(0), Ok(true));
        reorder.ingest(frame(0, 1)).unwrap();
        let release = reorder.ingest(frame(0x0fff, 0)).unwrap();
        assert_eq!(
            release.iter().collect::<std::vec::Vec<_>>(),
            [frame(0x0fff, 0), frame(0, 1)]
        );
        let stale = reorder.ingest(frame(0x0fff, 2)).unwrap();
        assert_eq!(stale.rejected, Some(frame(0x0fff, 2)));
        assert_eq!(reorder.retains_on_ingest(0x0fff), Ok(false));
    }

    #[test]
    fn retention_prediction_matches_window_advance_and_duplicate_edges() {
        let mut reorder = RxBlockAckReorder::new(10, 8).unwrap();
        assert_eq!(reorder.retains_on_ingest(20), Ok(true));
        reorder.ingest(frame(20, 0)).unwrap();
        assert_eq!(
            reorder.retains_on_ingest(20),
            Err(RxAmpduError::DuplicateSequence(20))
        );

        let mut singleton = RxBlockAckReorderState::<1>::new(10, 1).unwrap();
        assert_eq!(singleton.retains_on_ingest(20), Ok(false));
        assert!(!singleton.ingest(frame(20, 0)).unwrap().buffered);
    }

    #[test]
    fn a_slot_index_cannot_be_owned_twice() {
        let mut reorder = RxBlockAckReorder::new(1, 8).unwrap();
        reorder.ingest(frame(2, 4)).unwrap();
        assert_eq!(
            reorder.ingest(frame(3, 4)),
            Err(RxAmpduError::SlotAlreadyOwned(4))
        );
    }

    #[test]
    fn esf_slot_id_is_independent_of_reorder_window_index() {
        let mut reorder = RxBlockAckReorder::new(1, RX_BLOCK_ACK_MAX_WINDOW).unwrap();
        let highest_valid = (RX_REORDER_SLOT_ID_CAPACITY - 1) as u8;
        let first_invalid = RX_REORDER_SLOT_ID_CAPACITY as u8;
        assert_eq!(
            reorder
                .ingest(frame(1, highest_valid))
                .unwrap()
                .iter()
                .collect::<std::vec::Vec<_>>(),
            [frame(1, highest_valid)]
        );
        assert_eq!(
            reorder.ingest(frame(2, first_invalid)),
            Err(RxAmpduError::InvalidSlot(first_invalid))
        );
    }

    #[test]
    fn integration_can_bind_the_reorder_to_its_exact_slot_domain() {
        let mut reorder = RxBlockAckReorderState::<40>::new(1, 8).unwrap();
        assert_eq!(
            reorder
                .ingest(frame(1, 39))
                .unwrap()
                .iter()
                .collect::<std::vec::Vec<_>>(),
            [frame(1, 39)]
        );
        assert_eq!(
            reorder.ingest(frame(2, 40)),
            Err(RxAmpduError::InvalidSlot(40))
        );
    }

    #[test]
    fn window_cannot_exceed_the_owned_reorder_slot_pool() {
        assert!(RxBlockAckReorder::new(0, RX_BLOCK_ACK_MAX_WINDOW).is_ok());
        assert!(matches!(
            RxBlockAckReorder::new(0, RX_BLOCK_ACK_MAX_WINDOW + 1),
            Err(RxAmpduError::InvalidWindow(_))
        ));
    }

    #[test]
    fn stop_releases_every_owned_slot_in_sequence_order() {
        let mut reorder = RxBlockAckReorder::new(4094, 8).unwrap();
        reorder.ingest(frame(1, 3)).unwrap();
        reorder.ingest(frame(4095, 1)).unwrap();
        let release = reorder.stop();
        assert_eq!(
            release.iter().collect::<std::vec::Vec<_>>(),
            [frame(4095, 1), frame(1, 3)]
        );
        assert_eq!(reorder.occupied(), 0);
    }

    #[test]
    fn successful_response_narrows_the_window_and_disables_amsdu() {
        let mut body = [0xff; 9];
        write_successful_addba_response(&mut body, 137, 0, 16).unwrap();
        assert_eq!(body, [3, 1, 137, 0, 0, 0x02, 0x04, 0, 0]);
        assert_eq!(
            crate::tx_ampdu::parse_block_ack_action(&body),
            Some(crate::tx_ampdu::BlockAckAction::AddbaResponse {
                dialog_token: 137,
                status: 0,
                tid: 0,
                immediate: true,
                amsdu: false,
                window: 16,
                timeout_tu: 0,
            })
        );
    }

    #[test]
    fn declined_response_preserves_request_identity_without_claiming_success() {
        let mut body = [0xff; 9];
        write_declined_addba_response(&mut body, 23, 6, 64).unwrap();
        assert_eq!(
            crate::tx_ampdu::parse_block_ack_action(&body),
            Some(crate::tx_ampdu::BlockAckAction::AddbaResponse {
                dialog_token: 23,
                status: ADDBA_STATUS_REQUEST_DECLINED,
                tid: 6,
                immediate: true,
                amsdu: false,
                window: 64,
                timeout_tu: 0,
            })
        );
    }

    #[test]
    fn station_rx_sessions_bind_protocol_window_hardware_bank_and_response() {
        let peer = [0x30, 0xed, 0xa0, 0xf3, 0xf6, 0xd0];
        let mut sessions = RxBlockAckSessions::<RX_BLOCK_ACK_BANK_COUNT>::new();
        sessions
            .offer(rx_request!(peer, 17, 7, true, 1023, 0, 0x0abc))
            .unwrap();
        let activation = sessions
            .begin_pending(MacInterface::Station)
            .unwrap()
            .unwrap();
        assert_eq!(
            activation.negotiated(),
            RxBlockAckSnapshot {
                hardware_index: 0,
                peer,
                tid: 7,
                window: RX_BLOCK_ACK_MAX_WINDOW,
                starting_sequence: 0x0abc,
            }
        );
        assert_eq!(
            activation.hardware(),
            S31RxBlockAckAgreement {
                hardware_index: 0,
                interface: MacInterface::Station,
                peer,
                tid: 7,
                starting_sequence: 0x0abc,
                window: RX_BLOCK_ACK_MAX_WINDOW,
            }
        );
        assert_eq!(
            crate::tx_ampdu::parse_block_ack_action(activation.response_body()),
            Some(crate::tx_ampdu::BlockAckAction::AddbaResponse {
                dialog_token: 17,
                status: 0,
                tid: 7,
                immediate: true,
                amsdu: false,
                window: RX_BLOCK_ACK_MAX_WINDOW,
                timeout_tu: 0,
            })
        );
        assert!(matches!(
            sessions.begin_pending(MacInterface::Station),
            Err(RxBlockAckSessionsError::ActivationBusy)
        ));
        let snapshot = sessions.commit(activation).unwrap();
        assert_eq!(sessions.snapshots()[0], Some(snapshot));
        assert_eq!(sessions.stop(peer, 7), Some(snapshot));
        assert_eq!(sessions.snapshots(), [None; RX_BLOCK_ACK_BANK_COUNT]);
    }

    #[test]
    fn integration_can_narrow_the_negotiated_rx_window_without_changing_hardware_geometry() {
        let peer = [0x30, 0xed, 0xa0, 0xf3, 0xf6, 0xd0];
        let mut sessions = RxBlockAckSessions::<1>::with_maximum_window(32).unwrap();
        assert_eq!(sessions.maximum_window(), 32);
        sessions
            .offer(rx_request!(peer, 17, 0, true, 64, 0, 123))
            .unwrap();

        let activation = sessions
            .begin_pending(MacInterface::Station)
            .unwrap()
            .unwrap();
        assert_eq!(activation.negotiated().window, 32);
        assert_eq!(activation.hardware().window, RX_BLOCK_ACK_MAX_WINDOW);
        assert_eq!(
            crate::tx_ampdu::parse_block_ack_action(activation.response_body()),
            Some(crate::tx_ampdu::BlockAckAction::AddbaResponse {
                dialog_token: 17,
                status: 0,
                tid: 0,
                immediate: true,
                amsdu: false,
                window: 32,
                timeout_tu: 0,
            })
        );

        assert!(matches!(
            RxBlockAckSessions::<1>::with_maximum_window(0),
            Err(RxBlockAckSessionsError::InvalidWindow(0))
        ));
        assert!(matches!(
            RxBlockAckSessions::<1>::with_maximum_window(RX_BLOCK_ACK_MAX_WINDOW + 1),
            Err(RxBlockAckSessionsError::InvalidWindow(window))
                if window == RX_BLOCK_ACK_MAX_WINDOW + 1
        ));
    }

    #[test]
    fn replacement_and_cancel_remove_the_previous_hardware_owner() {
        let peer = [0x30, 0xed, 0xa0, 0xf3, 0xf6, 0xd0];
        let mut sessions = RxBlockAckSessions::<RX_BLOCK_ACK_BANK_COUNT>::new();
        sessions
            .offer(rx_request!(peer, 1, 0, true, 32, 0, 10))
            .unwrap();
        let first = sessions
            .begin_pending(MacInterface::Station)
            .unwrap()
            .unwrap();
        let first_snapshot = sessions.commit(first).unwrap();

        sessions
            .offer(rx_request!(peer, 2, 0, true, 16, 0, 20))
            .unwrap();
        let replacement = sessions
            .begin_pending(MacInterface::Station)
            .unwrap()
            .unwrap();
        assert_eq!(replacement.replaced(), Some(first_snapshot));
        assert_eq!(replacement.hardware().hardware_index, 0);
        sessions.cancel(replacement).unwrap();
        assert_eq!(sessions.snapshots(), [None; RX_BLOCK_ACK_BANK_COUNT]);
    }

    #[test]
    fn station_rx_sessions_reject_every_unsupported_request_class() {
        let peer = [0x30, 0xed, 0xa0, 0xf3, 0xf6, 0xd0];
        let mut sessions = RxBlockAckSessions::<RX_BLOCK_ACK_BANK_COUNT>::new();
        assert_eq!(
            sessions.offer(rx_request!(peer, 1, 0, false, 32, 0, 0)),
            Err(RxBlockAckSessionsError::DelayedPolicyUnsupported)
        );
        assert_eq!(
            sessions.offer(rx_request!(peer, 1, 8, true, 32, 0, 0)),
            Err(RxBlockAckSessionsError::InvalidTid(8))
        );
        assert_eq!(
            sessions.offer(rx_request!(peer, 1, 0, true, 0, 0, 0)),
            Err(RxBlockAckSessionsError::InvalidWindow(0))
        );
        assert_eq!(
            sessions.offer(rx_request!(peer, 1, 0, true, 32, 1, 0)),
            Err(RxBlockAckSessionsError::NonzeroTimeout(1))
        );
        assert_eq!(
            sessions.offer(rx_request!(peer, 1, 0, true, 32, 0, 0x1000)),
            Err(RxBlockAckSessionsError::InvalidStartingSequence(0x1000))
        );
    }

    #[test]
    fn access_point_peers_with_the_same_tid_receive_distinct_hardware_banks() {
        let first_peer = [0x30, 0xed, 0xa0, 0xf3, 0xf6, 0xd0];
        let second_peer = [0x70, 0x15, 0xfb, 0xa8, 0x48, 0xf0];
        let mut sessions = RxBlockAckSessions::<RX_BLOCK_ACK_BANK_COUNT>::new();

        sessions
            .offer(rx_request!(first_peer, 1, 0, true, 32, 0, 10))
            .unwrap();
        let first = sessions
            .begin_pending(MacInterface::AccessPoint)
            .unwrap()
            .unwrap();
        assert_eq!(first.hardware().interface, MacInterface::AccessPoint);
        assert_eq!(first.hardware().hardware_index, 0);
        sessions.commit(first).unwrap();

        sessions
            .offer(rx_request!(second_peer, 2, 0, true, 32, 0, 20))
            .unwrap();
        let second = sessions
            .begin_pending(MacInterface::AccessPoint)
            .unwrap()
            .unwrap();
        assert_eq!(second.hardware().hardware_index, 1);
        sessions.commit(second).unwrap();

        assert!(sessions.stop(first_peer, 0).is_some());
        assert!(sessions.stop(second_peer, 0).is_some());
    }

    #[test]
    fn request_remains_pending_while_every_hardware_bank_is_owned() {
        let mut sessions = RxBlockAckSessions::<{ RX_BLOCK_ACK_BANK_COUNT + 1 }>::new();
        for index in 0..RX_BLOCK_ACK_BANK_COUNT {
            let peer = [2, 0, 0, 0, 0, index as u8];
            sessions
                .offer(rx_request!(peer, index as u8, 0, true, 32, 0, index as u16))
                .unwrap();
            let activation = sessions
                .begin_pending(MacInterface::AccessPoint)
                .unwrap()
                .unwrap();
            sessions.commit(activation).unwrap();
        }

        let waiting_peer = [2, 0, 0, 0, 1, 0];
        sessions
            .offer(rx_request!(waiting_peer, 9, 0, true, 32, 0, 100))
            .unwrap();
        assert!(matches!(
            sessions.begin_pending(MacInterface::AccessPoint),
            Err(RxBlockAckSessionsError::NoFreeHardwareBank)
        ));

        let released_peer = [2, 0, 0, 0, 0, 0];
        assert!(sessions.stop(released_peer, 0).is_some());
        let activation = sessions
            .begin_pending(MacInterface::AccessPoint)
            .unwrap()
            .expect("bank release must admit the retained pending request");
        assert_eq!(activation.negotiated().peer, waiting_peer);
    }

    #[test]
    fn explicit_decline_removes_only_the_selected_pending_request() {
        let first_peer = [2, 0, 0, 0, 0, 1];
        let second_peer = [2, 0, 0, 0, 0, 2];
        let mut sessions = RxBlockAckSessions::<2>::new();
        sessions
            .offer(rx_request!(first_peer, 1, 1, true, 16, 0, 10))
            .unwrap();
        sessions
            .offer(rx_request!(second_peer, 2, 1, true, 16, 0, 20))
            .unwrap();

        assert!(sessions.discard_pending(first_peer, 1));
        assert!(!sessions.discard_pending(first_peer, 1));
        let activation = sessions
            .begin_pending(MacInterface::AccessPoint)
            .unwrap()
            .expect("other peer request remains pending");
        assert_eq!(activation.negotiated().peer, second_peer);
    }

    #[test]
    fn peer_teardown_removes_pending_and_active_agreements_only_for_that_peer() {
        let first_peer = [0x30, 0xed, 0xa0, 0xf3, 0xf6, 0xd0];
        let second_peer = [0x70, 0x15, 0xfb, 0xa8, 0x48, 0xf0];
        let mut sessions = RxBlockAckSessions::<RX_BLOCK_ACK_BANK_COUNT>::new();
        sessions
            .offer(rx_request!(first_peer, 1, 0, true, 32, 0, 10))
            .unwrap();
        let active = sessions
            .begin_pending(MacInterface::AccessPoint)
            .unwrap()
            .unwrap();
        sessions.commit(active).unwrap();
        sessions
            .offer(rx_request!(first_peer, 2, 1, true, 32, 0, 20))
            .unwrap();
        sessions
            .offer(rx_request!(second_peer, 3, 1, true, 32, 0, 30))
            .unwrap();

        let stopped = sessions.stop_peer(first_peer);
        assert_eq!(stopped.into_iter().flatten().count(), 1);
        let remaining = sessions
            .begin_pending(MacInterface::AccessPoint)
            .unwrap()
            .unwrap();
        assert_eq!(remaining.negotiated().peer, second_peer);
    }
}
