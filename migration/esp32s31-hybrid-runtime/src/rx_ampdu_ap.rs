//! Strict receive BlockAck ownership for SoftAP and station links.
//!
//! This HIL layer bridges one measured ADDBA exchange to the allocation-free
//! reorder machine. All mutable protocol state is touched only by the Rust
//! radio owner; the RX interrupt owns only the fixed kind-7 ESF pool.

use core::{
    cell::UnsafeCell,
    ffi::c_void,
    future::Future,
    pin::Pin,
    sync::atomic::{AtomicUsize, Ordering},
    task::{Context, Poll},
};

use crate::{
    queue::WakerCell,
    rx_ampdu::{
        RxAmpduError, RxAmpduMpdu, RxAmpduRelease, RxBlockAckReorder, RX_BLOCK_ACK_MAX_WINDOW,
    },
    rx_ampdu_hw::S31RxBlockAckAgreement,
    tx_ampdu::BlockAckAction,
};

const AP_INTERFACE_INDEX: u8 = 1;
const STA_INTERFACE_INDEX: u8 = 0;
const HARDWARE_INDEX: u8 = 0;
const INITIAL_SUPPORTED_TID: u8 = 0;
const ADDBA_RESPONSE_BODY_LEN: usize = 9;

unsafe extern "C" {
    fn esf_buf_recycle(frame: *mut c_void);
}

#[derive(Clone, Copy)]
struct PendingRequest {
    peer: [u8; 6],
    dialog_token: u8,
    tid: u8,
    starting_sequence: u16,
}

struct ActiveAgreement {
    peer: [u8; 6],
    tid: u8,
    interface: u8,
    reorder: RxBlockAckReorder,
    gap_generation: Option<usize>,
}

struct State {
    pending: Option<PendingRequest>,
    active: Option<ActiveAgreement>,
}

impl State {
    const fn new() -> Self {
        Self {
            pending: None,
            active: None,
        }
    }
}

struct RadioOwnerState(UnsafeCell<State>);

impl RadioOwnerState {
    const fn new() -> Self {
        Self(UnsafeCell::new(State::new()))
    }
}

// The only accessors below first require the strict radio hart and active
// radio-owner context. RX interrupts never touch this object.
unsafe impl Sync for RadioOwnerState {}

static STATE: RadioOwnerState = RadioOwnerState::new();
static GAP_GENERATION: AtomicUsize = AtomicUsize::new(0);
static GAP_EDGE: WakerCell = WakerCell::new();
static PENDING_REQUESTS: AtomicUsize = AtomicUsize::new(0);
static ACCEPTED_RESPONSES: AtomicUsize = AtomicUsize::new(0);
static HARDWARE_PROGRAM_FAILURES: AtomicUsize = AtomicUsize::new(0);
static OUTPUT_ROLLBACKS: AtomicUsize = AtomicUsize::new(0);
static RETAINED_FRAMES: AtomicUsize = AtomicUsize::new(0);
static RELEASED_FRAMES: AtomicUsize = AtomicUsize::new(0);
static REJECTED_FRAMES: AtomicUsize = AtomicUsize::new(0);
static REJECTED_MISSING_SLOT: AtomicUsize = AtomicUsize::new(0);
static REJECTED_MALFORMED_QOS: AtomicUsize = AtomicUsize::new(0);
static REJECTED_STATE_UNAVAILABLE: AtomicUsize = AtomicUsize::new(0);
static REJECTED_INACTIVE: AtomicUsize = AtomicUsize::new(0);
static REJECTED_DIRECTION: AtomicUsize = AtomicUsize::new(0);
static REJECTED_PEER: AtomicUsize = AtomicUsize::new(0);
static REJECTED_TID: AtomicUsize = AtomicUsize::new(0);
static REJECTED_INVALID_WINDOW: AtomicUsize = AtomicUsize::new(0);
static REJECTED_INVALID_SEQUENCE: AtomicUsize = AtomicUsize::new(0);
static REJECTED_INVALID_SLOT: AtomicUsize = AtomicUsize::new(0);
static REJECTED_DUPLICATE_SEQUENCE: AtomicUsize = AtomicUsize::new(0);
static REJECTED_SLOT_ALREADY_OWNED: AtomicUsize = AtomicUsize::new(0);
static REJECTED_STALE_SEQUENCE: AtomicUsize = AtomicUsize::new(0);
static REORDER_MISSING_SEQUENCES: AtomicUsize = AtomicUsize::new(0);
static LAST_REJECTED_SEQUENCE: AtomicUsize = AtomicUsize::new(0);
static LAST_REJECTED_SLOT: AtomicUsize = AtomicUsize::new(0);
static LAST_REJECTED_TID: AtomicUsize = AtomicUsize::new(0);
static LAST_REJECTED_DIRECTION: AtomicUsize = AtomicUsize::new(0);
static LAST_EXPECTED_SEQUENCE: AtomicUsize = AtomicUsize::new(0);
static GAP_EDGES: AtomicUsize = AtomicUsize::new(0);
static GAP_EXPIRIES: AtomicUsize = AtomicUsize::new(0);
static STALE_EXPIRIES: AtomicUsize = AtomicUsize::new(0);
static STOPS: AtomicUsize = AtomicUsize::new(0);
static ACTIVE: AtomicUsize = AtomicUsize::new(0);
static OCCUPIED: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RxAmpduApSnapshot {
    pub pending_requests: usize,
    pub accepted_responses: usize,
    pub hardware_program_failures: usize,
    pub output_rollbacks: usize,
    pub retained_frames: usize,
    pub released_frames: usize,
    pub rejected_frames: usize,
    pub rejected_missing_slot: usize,
    pub rejected_malformed_qos: usize,
    pub rejected_state_unavailable: usize,
    pub rejected_inactive: usize,
    pub rejected_direction: usize,
    pub rejected_peer: usize,
    pub rejected_tid: usize,
    pub rejected_invalid_window: usize,
    pub rejected_invalid_sequence: usize,
    pub rejected_invalid_slot: usize,
    pub rejected_duplicate_sequence: usize,
    pub rejected_slot_already_owned: usize,
    pub rejected_stale_sequence: usize,
    pub reorder_missing_sequences: usize,
    pub last_rejected_sequence: usize,
    pub last_rejected_slot: usize,
    pub last_rejected_tid: usize,
    pub last_rejected_direction: usize,
    pub last_expected_sequence: usize,
    pub gap_edges: usize,
    pub gap_expiries: usize,
    pub stale_expiries: usize,
    pub stops: usize,
    pub active: bool,
    pub occupied: usize,
}

pub fn snapshot() -> RxAmpduApSnapshot {
    RxAmpduApSnapshot {
        pending_requests: PENDING_REQUESTS.load(Ordering::Relaxed),
        accepted_responses: ACCEPTED_RESPONSES.load(Ordering::Relaxed),
        hardware_program_failures: HARDWARE_PROGRAM_FAILURES.load(Ordering::Relaxed),
        output_rollbacks: OUTPUT_ROLLBACKS.load(Ordering::Relaxed),
        retained_frames: RETAINED_FRAMES.load(Ordering::Relaxed),
        released_frames: RELEASED_FRAMES.load(Ordering::Relaxed),
        rejected_frames: REJECTED_FRAMES.load(Ordering::Relaxed),
        rejected_missing_slot: REJECTED_MISSING_SLOT.load(Ordering::Relaxed),
        rejected_malformed_qos: REJECTED_MALFORMED_QOS.load(Ordering::Relaxed),
        rejected_state_unavailable: REJECTED_STATE_UNAVAILABLE.load(Ordering::Relaxed),
        rejected_inactive: REJECTED_INACTIVE.load(Ordering::Relaxed),
        rejected_direction: REJECTED_DIRECTION.load(Ordering::Relaxed),
        rejected_peer: REJECTED_PEER.load(Ordering::Relaxed),
        rejected_tid: REJECTED_TID.load(Ordering::Relaxed),
        rejected_invalid_window: REJECTED_INVALID_WINDOW.load(Ordering::Relaxed),
        rejected_invalid_sequence: REJECTED_INVALID_SEQUENCE.load(Ordering::Relaxed),
        rejected_invalid_slot: REJECTED_INVALID_SLOT.load(Ordering::Relaxed),
        rejected_duplicate_sequence: REJECTED_DUPLICATE_SEQUENCE.load(Ordering::Relaxed),
        rejected_slot_already_owned: REJECTED_SLOT_ALREADY_OWNED.load(Ordering::Relaxed),
        rejected_stale_sequence: REJECTED_STALE_SEQUENCE.load(Ordering::Relaxed),
        reorder_missing_sequences: REORDER_MISSING_SEQUENCES.load(Ordering::Relaxed),
        last_rejected_sequence: LAST_REJECTED_SEQUENCE.load(Ordering::Relaxed),
        last_rejected_slot: LAST_REJECTED_SLOT.load(Ordering::Relaxed),
        last_rejected_tid: LAST_REJECTED_TID.load(Ordering::Relaxed),
        last_rejected_direction: LAST_REJECTED_DIRECTION.load(Ordering::Relaxed),
        last_expected_sequence: LAST_EXPECTED_SEQUENCE.load(Ordering::Relaxed),
        gap_edges: GAP_EDGES.load(Ordering::Relaxed),
        gap_expiries: GAP_EXPIRIES.load(Ordering::Relaxed),
        stale_expiries: STALE_EXPIRIES.load(Ordering::Relaxed),
        stops: STOPS.load(Ordering::Relaxed),
        active: ACTIVE.load(Ordering::Acquire) != 0,
        occupied: OCCUPIED.load(Ordering::Acquire),
    }
}

pub struct RxAmpduGapFuture {
    after: usize,
}

impl Future for RxAmpduGapFuture {
    type Output = usize;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        GAP_EDGE.register(cx.waker());
        let generation = GAP_GENERATION.load(Ordering::Acquire);
        if generation != self.after {
            Poll::Ready(generation)
        } else {
            Poll::Pending
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Ingress {
    Retained,
    Release(RxAmpduRelease),
    Reject,
}

#[derive(Clone, Copy)]
enum RejectReason {
    MissingSlot,
    MalformedQos,
    StateUnavailable,
    Inactive,
    Direction,
    Peer,
    Tid,
    InvalidWindow,
    InvalidSequence,
    InvalidSlot,
    DuplicateSequence,
    SlotAlreadyOwned,
    StaleSequence,
}

fn reject(reason: RejectReason) -> Ingress {
    REJECTED_FRAMES.fetch_add(1, Ordering::Relaxed);
    let counter = match reason {
        RejectReason::MissingSlot => &REJECTED_MISSING_SLOT,
        RejectReason::MalformedQos => &REJECTED_MALFORMED_QOS,
        RejectReason::StateUnavailable => &REJECTED_STATE_UNAVAILABLE,
        RejectReason::Inactive => &REJECTED_INACTIVE,
        RejectReason::Direction => &REJECTED_DIRECTION,
        RejectReason::Peer => &REJECTED_PEER,
        RejectReason::Tid => &REJECTED_TID,
        RejectReason::InvalidWindow => &REJECTED_INVALID_WINDOW,
        RejectReason::InvalidSequence => &REJECTED_INVALID_SEQUENCE,
        RejectReason::InvalidSlot => &REJECTED_INVALID_SLOT,
        RejectReason::DuplicateSequence => &REJECTED_DUPLICATE_SEQUENCE,
        RejectReason::SlotAlreadyOwned => &REJECTED_SLOT_ALREADY_OWNED,
        RejectReason::StaleSequence => &REJECTED_STALE_SEQUENCE,
    };
    counter.fetch_add(1, Ordering::Relaxed);
    Ingress::Reject
}

fn state() -> Option<&'static mut State> {
    if !crate::critical::on_strict_wifi_hart() || !crate::context::in_radio_context() {
        return None;
    }
    // Soundness: strict takeover admits exactly one radio owner on one hart.
    // This module is not called recursively and the returned borrow does not
    // escape an accessor.
    Some(unsafe { &mut *STATE.0.get() })
}

pub(crate) fn observe_action(frame: &[u8], action: BlockAckAction) {
    if frame.len() < 24 {
        return;
    }
    let mut peer = [0_u8; 6];
    peer.copy_from_slice(&frame[10..16]);
    if peer[0] & 1 != 0 {
        return;
    }
    match action {
        BlockAckAction::AddbaRequest {
            dialog_token,
            tid,
            immediate,
            timeout_tu,
            starting_sequence,
            ..
        } if immediate
            && timeout_tu == 0
            && tid == INITIAL_SUPPORTED_TID
            && starting_sequence <= 0x0fff
            && crate::wpa2_ap::wpa2_ap_peer_association_epoch(&peer).is_some() =>
        {
            let Some(state) = state() else {
                return;
            };
            state.pending = Some(PendingRequest {
                peer,
                dialog_token,
                tid,
                starting_sequence,
            });
            PENDING_REQUESTS.fetch_add(1, Ordering::Relaxed);
        }
        BlockAckAction::Delba { tid, .. } => stop_peer(peer, tid),
        _ => {}
    }
}

/// Turn the bounded vendor decline into one Rust-owned successful response.
///
/// Software reorder state becomes visible before the hardware agreement is
/// enabled. The response is modified only after the finite MMIO transaction
/// succeeds.
pub(crate) fn try_accept_response(peer: [u8; 6], body: &mut [u8]) -> bool {
    if body.len() != ADDBA_RESPONSE_BODY_LEN || body[0] != 3 || body[1] != 1 {
        return false;
    }
    let pending = {
        let Some(state) = state() else {
            return false;
        };
        let Some(pending) = state.pending.take() else {
            return false;
        };
        if pending.peer != peer || pending.dialog_token != body[2] || state.active.is_some() {
            return false;
        }
        pending
    };
    if !install_agreement(
        peer,
        pending.tid,
        pending.starting_sequence,
        RX_BLOCK_ACK_MAX_WINDOW,
        AP_INTERFACE_INDEX,
    ) {
        return false;
    }

    let accepted = crate::rx_ampdu::write_successful_addba_response(
        body,
        pending.dialog_token,
        pending.tid,
        RX_BLOCK_ACK_MAX_WINDOW,
    )
    .is_ok();
    if accepted {
        ACTIVE.store(1, Ordering::Release);
        OCCUPIED.store(0, Ordering::Release);
        ACCEPTED_RESPONSES.fetch_add(1, Ordering::Relaxed);
    } else {
        rollback_failed_response(peer);
    }
    accepted
}

/// Accept one validated station-side ADDBA request into the same fixed reorder
/// owner used by SoftAP.
///
/// The caller owns the management response buffer. It must roll the agreement
/// back if the finite transmit leaf rejects the response or TX completes
/// without an acknowledgement.
pub(crate) fn try_accept_sta_request(peer: [u8; 6], request: &[u8], response: &mut [u8]) -> bool {
    if peer[0] & 1 != 0 || response.len() != ADDBA_RESPONSE_BODY_LEN {
        return false;
    }
    let Some(BlockAckAction::AddbaRequest {
        dialog_token,
        tid,
        immediate,
        window,
        timeout_tu,
        starting_sequence,
        ..
    }) = crate::tx_ampdu::parse_block_ack_action(request)
    else {
        return false;
    };
    if !immediate
        || window == 0
        || timeout_tu != 0
        || tid != INITIAL_SUPPORTED_TID
        || starting_sequence > 0x0fff
    {
        return false;
    }
    let selected_window = window.min(RX_BLOCK_ACK_MAX_WINDOW);
    if !install_agreement(
        peer,
        tid,
        starting_sequence,
        selected_window,
        STA_INTERFACE_INDEX,
    ) {
        return false;
    }
    let accepted = crate::rx_ampdu::write_successful_addba_response(
        response,
        dialog_token,
        tid,
        selected_window,
    )
    .is_ok();
    if accepted {
        ACTIVE.store(1, Ordering::Release);
        OCCUPIED.store(0, Ordering::Release);
        ACCEPTED_RESPONSES.fetch_add(1, Ordering::Relaxed);
    } else {
        rollback_failed_response(peer);
    }
    accepted
}

fn install_agreement(
    peer: [u8; 6],
    tid: u8,
    starting_sequence: u16,
    window: u16,
    interface: u8,
) -> bool {
    let Ok(reorder) = RxBlockAckReorder::new(starting_sequence, window) else {
        return false;
    };
    let Some(state) = state() else {
        return false;
    };
    if state.active.is_some() {
        return false;
    }
    state.active = Some(ActiveAgreement {
        peer,
        tid,
        interface,
        reorder,
        gap_generation: None,
    });
    let agreement = S31RxBlockAckAgreement {
        hardware_index: HARDWARE_INDEX,
        interface,
        peer,
        tid,
        starting_sequence,
        window,
    };
    if unsafe { crate::rx_ampdu_hw::program(agreement) }.is_err() {
        state.active = None;
        HARDWARE_PROGRAM_FAILURES.fetch_add(1, Ordering::Relaxed);
        return false;
    }
    true
}

/// Undo an agreement when the finite management TX leaf rejects its response.
pub(crate) fn rollback_failed_response(peer: [u8; 6]) {
    let tid = state()
        .and_then(|state| state.active.as_ref())
        .filter(|active| active.peer == peer)
        .map(|active| active.tid);
    if let Some(tid) = tid {
        OUTPUT_ROLLBACKS.fetch_add(1, Ordering::Relaxed);
        stop_peer(peer, tid);
    }
}

pub(crate) fn ingest(packet: *mut u8, frame: &[u8]) -> Ingress {
    let Some(slot) = crate::esf::large_rx_slot_id(packet) else {
        return reject(RejectReason::MissingSlot);
    };
    if frame.len() < 26 || frame[0] & 0x0c != 0x08 || frame[0] & 0x80 == 0 {
        return reject(RejectReason::MalformedQos);
    }
    let sequence = u16::from_le_bytes([frame[22], frame[23]]) >> 4;
    let tid = frame[24] & 0x0f;
    let direction = frame[1] & 0x03;
    let mut peer = [0_u8; 6];
    peer.copy_from_slice(&frame[10..16]);
    let Some(state) = state() else {
        record_rejected_frame(sequence, slot, tid, direction, 0);
        return reject(RejectReason::StateUnavailable);
    };
    let Some(active) = state.active.as_mut() else {
        record_rejected_frame(sequence, slot, tid, direction, 0);
        return reject(RejectReason::Inactive);
    };
    let expected_direction = if active.interface == AP_INTERFACE_INDEX {
        0x01
    } else {
        0x02
    };
    if direction != expected_direction {
        record_rejected_frame(
            sequence,
            slot,
            tid,
            direction,
            active.reorder.next_sequence(),
        );
        return reject(RejectReason::Direction);
    }
    if active.peer != peer {
        record_rejected_frame(
            sequence,
            slot,
            tid,
            direction,
            active.reorder.next_sequence(),
        );
        return reject(RejectReason::Peer);
    }
    if active.tid != tid {
        record_rejected_frame(
            sequence,
            slot,
            tid,
            direction,
            active.reorder.next_sequence(),
        );
        return reject(RejectReason::Tid);
    }
    let release = match active.reorder.ingest(RxAmpduMpdu { sequence, slot }) {
        Ok(release) => release,
        Err(error) => {
            record_rejected_frame(
                sequence,
                slot,
                tid,
                direction,
                active.reorder.next_sequence(),
            );
            let reason = match error {
                RxAmpduError::InvalidWindow(_) => RejectReason::InvalidWindow,
                RxAmpduError::InvalidSequence(_) => RejectReason::InvalidSequence,
                RxAmpduError::InvalidSlot(_) => RejectReason::InvalidSlot,
                RxAmpduError::DuplicateSequence(_) => RejectReason::DuplicateSequence,
                RxAmpduError::SlotAlreadyOwned(_) => RejectReason::SlotAlreadyOwned,
            };
            return reject(reason);
        }
    };
    if release.rejected.is_some() {
        record_rejected_frame(
            sequence,
            slot,
            tid,
            direction,
            active.reorder.next_sequence(),
        );
        return reject(RejectReason::StaleSequence);
    }
    REORDER_MISSING_SEQUENCES.fetch_add(release.missing as usize, Ordering::Relaxed);
    OCCUPIED.store(active.reorder.occupied() as usize, Ordering::Release);
    update_gap_edge(active);
    if release.count == 0 {
        RETAINED_FRAMES.fetch_add(1, Ordering::Relaxed);
        Ingress::Retained
    } else {
        RELEASED_FRAMES.fetch_add(release.count as usize, Ordering::Relaxed);
        Ingress::Release(release)
    }
}

fn record_rejected_frame(sequence: u16, slot: u8, tid: u8, direction: u8, expected_sequence: u16) {
    LAST_REJECTED_SEQUENCE.store(sequence as usize, Ordering::Relaxed);
    LAST_REJECTED_SLOT.store(slot as usize, Ordering::Relaxed);
    LAST_REJECTED_TID.store(tid as usize, Ordering::Relaxed);
    LAST_REJECTED_DIRECTION.store(direction as usize, Ordering::Relaxed);
    LAST_EXPECTED_SEQUENCE.store(expected_sequence as usize, Ordering::Relaxed);
}

pub(crate) fn frame_for_slot(slot: u8) -> Option<*mut u8> {
    crate::esf::large_rx_frame(slot)
}

pub fn wait_for_gap(after: usize) -> RxAmpduGapFuture {
    RxAmpduGapFuture { after }
}

pub fn remove_peer(peer: [u8; 6]) {
    let tid = state()
        .and_then(|state| state.active.as_ref())
        .filter(|active| active.peer == peer)
        .map(|active| active.tid);
    if let Some(tid) = tid {
        stop_peer(peer, tid);
    } else if let Some(state) = state() {
        if state.pending.is_some_and(|pending| pending.peer == peer) {
            state.pending = None;
        }
    }
}

pub(crate) fn expire_gap(generation: usize) -> Option<RxAmpduRelease> {
    let Some(state) = state() else {
        STALE_EXPIRIES.fetch_add(1, Ordering::Relaxed);
        return None;
    };
    let Some(active) = state.active.as_mut() else {
        STALE_EXPIRIES.fetch_add(1, Ordering::Relaxed);
        return None;
    };
    if active.gap_generation != Some(generation) {
        STALE_EXPIRIES.fetch_add(1, Ordering::Relaxed);
        return None;
    }
    active.gap_generation = None;
    let release = active.reorder.expire_gap();
    GAP_EXPIRIES.fetch_add(1, Ordering::Relaxed);
    REORDER_MISSING_SEQUENCES.fetch_add(release.missing as usize, Ordering::Relaxed);
    RELEASED_FRAMES.fetch_add(release.count as usize, Ordering::Relaxed);
    OCCUPIED.store(active.reorder.occupied() as usize, Ordering::Release);
    update_gap_edge(active);
    Some(release)
}

fn update_gap_edge(active: &mut ActiveAgreement) {
    if active.reorder.occupied() == 0 {
        active.gap_generation = None;
        return;
    }
    if active.gap_generation.is_some() {
        return;
    }
    let generation = GAP_GENERATION
        .fetch_add(1, Ordering::AcqRel)
        .wrapping_add(1);
    active.gap_generation = Some(generation);
    GAP_EDGES.fetch_add(1, Ordering::Relaxed);
    GAP_EDGE.wake();
}

fn stop_peer(peer: [u8; 6], tid: u8) {
    let Some(state) = state() else {
        return;
    };
    if state
        .pending
        .is_some_and(|pending| pending.peer == peer && pending.tid == tid)
    {
        state.pending = None;
    }
    let Some(active) = state.active.as_ref() else {
        return;
    };
    if active.peer != peer || active.tid != tid {
        return;
    }
    let Some(mut active) = state.active.take() else {
        return;
    };
    let retained = active.reorder.stop();
    for frame in retained.iter() {
        if let Some(packet) = crate::esf::large_rx_frame(frame.slot) {
            unsafe { esf_buf_recycle(packet.cast()) };
        }
    }
    let _ = unsafe { crate::rx_ampdu_hw::clear(HARDWARE_INDEX) };
    STOPS.fetch_add(1, Ordering::Relaxed);
    ACTIVE.store(0, Ordering::Release);
    OCCUPIED.store(0, Ordering::Release);
}
