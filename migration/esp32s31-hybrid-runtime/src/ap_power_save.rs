//! Event edge between AP receive processing and a deferred unicast transmit.
//!
//! The vendor hostap output wrapper owns a dynamically linked power-save
//! queue. Strict mode does not enter that queue. Instead, the radio owner
//! retains the original owned command and is woken only when that peer sends an
//! active-mode data frame or a PS-Poll. TIM mutation remains a small, measured
//! Rust leaf. Every readiness edge is bound to the fixed AP association
//! generation as well as its MAC address, so removal/reassociation cannot
//! transfer a credit to another session.

use core::{
    sync::atomic::{AtomicU8, AtomicUsize, Ordering},
    task::{Context, Poll},
};

use crate::queue::WakerCell;

static ACTIVE_EDGE: WakerCell = WakerCell::new();
static PS_POLL_EPOCH: AtomicUsize = AtomicUsize::new(0);
static PEER_EVENT_EPOCH: AtomicUsize = AtomicUsize::new(0);
static GROUP_DTIM_EPOCH: AtomicUsize = AtomicUsize::new(0);
static BEACON_OBSERVATIONS: AtomicUsize = AtomicUsize::new(0);
static BEACON_PARSE_FAILURES: AtomicUsize = AtomicUsize::new(0);
static GROUP_DTIM_OBSERVATIONS: AtomicUsize = AtomicUsize::new(0);
static LAST_DTIM_COUNT: AtomicU8 = AtomicU8::new(u8::MAX);
static LAST_DTIM_PERIOD: AtomicU8 = AtomicU8::new(0);
static SLEEP_OBSERVATIONS: AtomicUsize = AtomicUsize::new(0);
static ACTIVE_OBSERVATIONS: AtomicUsize = AtomicUsize::new(0);
static REMOVAL_OBSERVATIONS: AtomicUsize = AtomicUsize::new(0);
static DEFERRED_TRANSMITS: AtomicUsize = AtomicUsize::new(0);
static CANCELLED_TRANSMITS: AtomicUsize = AtomicUsize::new(0);
static OVERFLOWED_TRANSMITS: AtomicUsize = AtomicUsize::new(0);
static LAST_DEFERRED_PEER: [AtomicU8; 6] = [const { AtomicU8::new(0) }; 6];
static LAST_OVERFLOWED_PEER: [AtomicU8; 6] = [const { AtomicU8::new(0) }; 6];

// This matches the fixed WPA2 AP association capacity. The table is written
// only by serialized callbacks on the radio owner. Atomic fields keep waker
// publication well-defined without a critical section.
const PEER_EVENT_CAPACITY: usize = crate::wpa2_ap::WPA2_AP_ASSOC_CAPACITY;

struct PeerEventSlot {
    peer: [AtomicU8; 6],
    association_epoch: AtomicUsize,
    active_epoch: AtomicUsize,
    ps_poll_epoch: AtomicUsize,
    removal_epoch: AtomicUsize,
}

impl PeerEventSlot {
    const fn new() -> Self {
        Self {
            peer: [const { AtomicU8::new(0) }; 6],
            association_epoch: AtomicUsize::new(0),
            active_epoch: AtomicUsize::new(0),
            ps_poll_epoch: AtomicUsize::new(0),
            removal_epoch: AtomicUsize::new(0),
        }
    }

    fn matches(&self, peer: &[u8; 6], association_epoch: usize) -> bool {
        self.association_epoch.load(Ordering::Acquire) == association_epoch
            && self
                .peer
                .iter()
                .zip(peer)
                .all(|(stored, expected)| stored.load(Ordering::Relaxed) == *expected)
    }

    fn last_epoch(&self) -> usize {
        self.active_epoch
            .load(Ordering::Acquire)
            .max(self.ps_poll_epoch.load(Ordering::Acquire))
            .max(self.removal_epoch.load(Ordering::Acquire))
    }

    fn replace_peer(&self, peer: &[u8; 6], association_epoch: usize) {
        // Zero is never a published event. Invalidate the slot before changing
        // its key so a reader cannot attach an old credit to a new association
        // which happens to reuse the same MAC address.
        self.association_epoch.store(0, Ordering::Release);
        self.active_epoch.store(0, Ordering::Release);
        self.ps_poll_epoch.store(0, Ordering::Release);
        self.removal_epoch.store(0, Ordering::Release);
        for (stored, value) in self.peer.iter().zip(peer) {
            stored.store(*value, Ordering::Relaxed);
        }
        self.association_epoch
            .store(association_epoch, Ordering::Release);
    }
}

static PEER_EVENTS: [PeerEventSlot; PEER_EVENT_CAPACITY] =
    [const { PeerEventSlot::new() }; PEER_EVENT_CAPACITY];

#[derive(Clone, Copy)]
enum PeerEvent {
    Active(usize),
    PsPoll(usize),
    Removed(usize),
}

#[cfg(target_arch = "riscv32")]
fn current_association_epoch(peer: &[u8; 6]) -> usize {
    crate::wpa2_ap::wpa2_ap_peer_association_epoch(peer).unwrap_or(0)
}

// Host tests exercise the finite event transport without linking the pinned
// target-only AP association table.
#[cfg(not(target_arch = "riscv32"))]
fn current_association_epoch(_peer: &[u8; 6]) -> usize {
    1
}

#[cfg(target_arch = "riscv32")]
fn current_association_id(peer: &[u8; 6]) -> Option<u16> {
    crate::wpa2_ap::wpa2_ap_peer_association_id(peer)
}

#[cfg(not(target_arch = "riscv32"))]
fn current_association_id(_peer: &[u8; 6]) -> Option<u16> {
    Some(1)
}

fn ps_poll_association_id(frame: &[u8]) -> Option<u16> {
    let raw = u16::from_le_bytes([*frame.get(2)?, *frame.get(3)?]);
    let association_id = raw & 0x3fff;
    (raw & 0xc000 == 0xc000 && association_id != 0).then_some(association_id)
}

fn publish_peer_event(peer: &[u8; 6], event: PeerEvent) {
    let association_epoch = current_association_epoch(peer);
    if association_epoch == 0 {
        return;
    }
    let mut replacement = 0;
    let mut replacement_epoch = usize::MAX;
    for (index, slot) in PEER_EVENTS.iter().enumerate() {
        if slot.matches(peer, association_epoch) && slot.last_epoch() != 0 {
            publish_in_slot(slot, event);
            return;
        }
        let epoch = slot.last_epoch();
        if epoch < replacement_epoch {
            replacement = index;
            replacement_epoch = epoch;
        }
    }

    let slot = &PEER_EVENTS[replacement];
    slot.replace_peer(peer, association_epoch);
    publish_in_slot(slot, event);
}

fn publish_in_slot(slot: &PeerEventSlot, event: PeerEvent) {
    match event {
        PeerEvent::Active(epoch) => slot.active_epoch.store(epoch, Ordering::Release),
        PeerEvent::PsPoll(epoch) => slot.ps_poll_epoch.store(epoch, Ordering::Release),
        PeerEvent::Removed(epoch) => slot.removal_epoch.store(epoch, Ordering::Release),
    }
}

fn next_peer_event_epoch() -> usize {
    let epoch = PEER_EVENT_EPOCH
        .fetch_add(1, Ordering::Relaxed)
        .wrapping_add(1);
    if epoch == 0 {
        PEER_EVENT_EPOCH
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1)
    } else {
        epoch
    }
}

fn peer_epochs(peer: &[u8; 6]) -> (usize, usize, usize) {
    let association_epoch = current_association_epoch(peer);
    if association_epoch == 0 {
        return (0, 0, 0);
    }
    for slot in &PEER_EVENTS {
        if slot.matches(peer, association_epoch) {
            return (
                slot.active_epoch.load(Ordering::Acquire),
                slot.ps_poll_epoch.load(Ordering::Acquire),
                slot.removal_epoch.load(Ordering::Acquire),
            );
        }
    }
    (0, 0, 0)
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ApPowerSaveSnapshot {
    pub sleep_observations: usize,
    pub active_observations: usize,
    pub ps_poll_observations: usize,
    pub beacon_observations: usize,
    pub beacon_parse_failures: usize,
    pub group_dtim_observations: usize,
    pub last_dtim_count: u8,
    pub last_dtim_period: u8,
    pub removal_observations: usize,
    pub deferred_transmits: usize,
    pub cancelled_transmits: usize,
    pub overflowed_transmits: usize,
    pub last_deferred_peer: [u8; 6],
    pub last_overflowed_peer: [u8; 6],
}

pub fn ap_power_save_snapshot() -> ApPowerSaveSnapshot {
    ApPowerSaveSnapshot {
        sleep_observations: SLEEP_OBSERVATIONS.load(Ordering::Acquire),
        active_observations: ACTIVE_OBSERVATIONS.load(Ordering::Acquire),
        ps_poll_observations: PS_POLL_EPOCH.load(Ordering::Acquire),
        beacon_observations: BEACON_OBSERVATIONS.load(Ordering::Acquire),
        beacon_parse_failures: BEACON_PARSE_FAILURES.load(Ordering::Acquire),
        group_dtim_observations: GROUP_DTIM_OBSERVATIONS.load(Ordering::Acquire),
        last_dtim_count: LAST_DTIM_COUNT.load(Ordering::Acquire),
        last_dtim_period: LAST_DTIM_PERIOD.load(Ordering::Acquire),
        removal_observations: REMOVAL_OBSERVATIONS.load(Ordering::Acquire),
        deferred_transmits: DEFERRED_TRANSMITS.load(Ordering::Acquire),
        cancelled_transmits: CANCELLED_TRANSMITS.load(Ordering::Acquire),
        overflowed_transmits: OVERFLOWED_TRANSMITS.load(Ordering::Acquire),
        last_deferred_peer: read_diagnostic_peer(&LAST_DEFERRED_PEER),
        last_overflowed_peer: read_diagnostic_peer(&LAST_OVERFLOWED_PEER),
    }
}

/// Observe a raw 802.11 frame before the vendor receive callback consumes it.
///
/// Only an infrastructure data frame directed to the AP can publish a client
/// power-management transition. Readiness is retained per source address and
/// current association generation, so a foreign peer or an old session cannot
/// wake or cancel a command owned by another peer.
pub(crate) fn observe_frame(frame: &[u8]) {
    if frame.len() < 2 {
        return;
    }
    let frame_control = u16::from_le_bytes([frame[0], frame[1]]);
    let frame_type = (frame_control >> 2) & 3;
    let subtype = (frame_control >> 4) & 0x0f;

    // Legacy PS-Poll is a 16-byte control frame. Address 2 is the station
    // transmitter and binds the one-frame delivery credit to one peer.
    if frame_type == 1 && subtype == 10 && frame.len() >= 16 {
        let peer = [
            frame[10], frame[11], frame[12], frame[13], frame[14], frame[15],
        ];
        let Some(association_id) = ps_poll_association_id(frame) else {
            return;
        };
        if current_association_id(&peer) != Some(association_id) {
            return;
        }
        PS_POLL_EPOCH.fetch_add(1, Ordering::Relaxed);
        let epoch = next_peer_event_epoch();
        publish_peer_event(&peer, PeerEvent::PsPoll(epoch));
        ACTIVE_EDGE.wake();
        return;
    }

    if frame.len() < 24 {
        return;
    }
    let is_data = frame_type == 2;
    let to_ds = frame_control & 0x0100 != 0;
    let from_ds = frame_control & 0x0200 != 0;
    if !is_data || !to_ds || from_ds {
        return;
    }

    if frame_control & 0x1000 != 0 {
        SLEEP_OBSERVATIONS.fetch_add(1, Ordering::Relaxed);
    } else {
        ACTIVE_OBSERVATIONS.fetch_add(1, Ordering::Relaxed);
        let peer = [
            frame[10], frame[11], frame[12], frame[13], frame[14], frame[15],
        ];
        let epoch = next_peer_event_epoch();
        publish_peer_event(&peer, PeerEvent::Active(epoch));
        ACTIVE_EDGE.wake();
    }
}

fn write_diagnostic_peer(destination: &[AtomicU8; 6], peer: &[u8; 6]) {
    for (slot, value) in destination.iter().zip(peer) {
        slot.store(*value, Ordering::Relaxed);
    }
}

fn read_diagnostic_peer(source: &[AtomicU8; 6]) -> [u8; 6] {
    core::array::from_fn(|index| source[index].load(Ordering::Relaxed))
}

pub(crate) fn record_deferred_transmit(peer: &[u8; 6]) {
    write_diagnostic_peer(&LAST_DEFERRED_PEER, peer);
    DEFERRED_TRANSMITS.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn observe_peer_removed(peer: &[u8; 6]) {
    REMOVAL_OBSERVATIONS.fetch_add(1, Ordering::Relaxed);
    publish_peer_event(peer, PeerEvent::Removed(next_peer_event_epoch()));
    ACTIVE_EDGE.wake();
}

/// Publish one transmitted DTIM beacon as a multicast delivery edge.
///
/// This is called from the strict beacon TX-done continuation, never from a
/// timer poll. One retained edge remains visible until the radio owner has
/// moved every group frame which preceded that beacon.
pub(crate) fn observe_beacon_dtim(dtim: Option<(u8, u8)>) {
    BEACON_OBSERVATIONS.fetch_add(1, Ordering::Relaxed);
    let Some((count, period)) = dtim else {
        BEACON_PARSE_FAILURES.fetch_add(1, Ordering::Relaxed);
        LAST_DTIM_COUNT.store(u8::MAX, Ordering::Relaxed);
        LAST_DTIM_PERIOD.store(0, Ordering::Relaxed);
        return;
    };
    LAST_DTIM_COUNT.store(count, Ordering::Relaxed);
    LAST_DTIM_PERIOD.store(period, Ordering::Relaxed);
    if count == 0 {
        GROUP_DTIM_OBSERVATIONS.fetch_add(1, Ordering::Relaxed);
        GROUP_DTIM_EPOCH.store(next_peer_event_epoch(), Ordering::Release);
        ACTIVE_EDGE.wake();
    }
}

pub(crate) fn record_cancelled_transmit() {
    CANCELLED_TRANSMITS.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn record_overflowed_transmit(peer: &[u8; 6]) {
    write_diagnostic_peer(&LAST_OVERFLOWED_PEER, peer);
    OVERFLOWED_TRANSMITS.fetch_add(1, Ordering::Relaxed);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PeerEdge {
    Retry,
    Removed,
}

/// Register the radio owner for the next RX-derived active-mode edge.
///
/// Readiness is never produced by inspecting node state, avoiding a status or
/// node polling loop.
pub(crate) fn active_epoch(peer: &[u8; 6]) -> usize {
    peer_epochs(peer).0
}

pub(crate) fn ps_poll_epoch(peer: &[u8; 6]) -> usize {
    peer_epochs(peer).1
}

pub(crate) fn removal_epoch(peer: &[u8; 6]) -> usize {
    peer_epochs(peer).2
}

pub(crate) fn group_dtim_epoch() -> usize {
    GROUP_DTIM_EPOCH.load(Ordering::Acquire)
}

pub(crate) fn poll_group_dtim(after: usize, cx: &mut Context<'_>) -> Poll<()> {
    ACTIVE_EDGE.register(cx.waker());
    if group_dtim_epoch() != after {
        Poll::Ready(())
    } else {
        Poll::Pending
    }
}

pub(crate) fn ps_poll_credit_after(after: usize, peer: &[u8; 6]) -> Option<usize> {
    let epoch = ps_poll_epoch(peer);
    (epoch != 0 && epoch != after).then_some(epoch)
}

pub(crate) fn poll_peer_edge(
    active_after: usize,
    ps_poll_after: usize,
    removal_after: usize,
    peer: &[u8; 6],
    cx: &mut Context<'_>,
) -> Poll<PeerEdge> {
    ACTIVE_EDGE.register(cx.waker());
    let removal_epoch = removal_epoch(peer);
    let active_epoch = active_epoch(peer);
    if removal_epoch != 0 && removal_epoch != removal_after {
        Poll::Ready(PeerEdge::Removed)
    } else if (active_epoch != 0 && active_epoch != active_after)
        || ps_poll_credit_after(ps_poll_after, peer).is_some()
    {
        Poll::Ready(PeerEdge::Retry)
    } else {
        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::{
        active_epoch, ap_power_save_snapshot, observe_frame, observe_peer_removed, poll_peer_edge,
        ps_poll_credit_after, ps_poll_epoch, removal_epoch, PeerEdge,
    };
    use core::task::{Context, Poll, Waker};
    use std::sync::{Mutex, MutexGuard};

    // The production state is intentionally one global radio-owner resource.
    // Serialize tests which mutate that resource so the host test scheduler
    // cannot make per-test counter deltas observe another test's frame.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn test_guard() -> MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn only_to_ds_data_publishes_power_state() {
        let _guard = test_guard();
        let before = ap_power_save_snapshot();
        let mut frame = [0_u8; 24];
        frame[..2].copy_from_slice(&0x1108_u16.to_le_bytes());
        observe_frame(&frame);
        frame[..2].copy_from_slice(&0x0108_u16.to_le_bytes());
        observe_frame(&frame);
        frame[..2].copy_from_slice(&0x0008_u16.to_le_bytes());
        observe_frame(&frame);
        let after = ap_power_save_snapshot();
        assert_eq!(after.sleep_observations, before.sleep_observations + 1);
        assert_eq!(after.active_observations, before.active_observations + 1);
    }

    #[test]
    fn ps_poll_credit_is_bound_to_transmitter() {
        let _guard = test_guard();
        let peer = [1, 2, 3, 4, 5, 6];
        let before = ps_poll_epoch(&peer);
        let mut frame = [0_u8; 16];
        frame[..2].copy_from_slice(&0x00a4_u16.to_le_bytes());
        frame[2..4].copy_from_slice(&0xc001_u16.to_le_bytes());
        frame[10..16].copy_from_slice(&peer);
        observe_frame(&frame);
        let published = ps_poll_credit_after(before, &peer).unwrap();
        assert_eq!(ps_poll_credit_after(published, &peer), None);
        assert_eq!(ps_poll_credit_after(before, &[9; 6]), None);
    }

    #[test]
    fn ps_poll_events_for_two_peers_remain_independent() {
        let _guard = test_guard();
        let first = [20, 2, 3, 4, 5, 6];
        let second = [21, 2, 3, 4, 5, 6];
        let first_before = ps_poll_epoch(&first);
        let second_before = ps_poll_epoch(&second);
        let mut frame = [0_u8; 16];
        frame[..2].copy_from_slice(&0x00a4_u16.to_le_bytes());
        frame[2..4].copy_from_slice(&0xc001_u16.to_le_bytes());
        frame[10..16].copy_from_slice(&first);
        observe_frame(&frame);
        frame[10..16].copy_from_slice(&second);
        observe_frame(&frame);
        assert!(ps_poll_credit_after(first_before, &first).is_some());
        assert!(ps_poll_credit_after(second_before, &second).is_some());
    }

    #[test]
    fn ps_poll_requires_the_current_nonzero_association_id() {
        let _guard = test_guard();
        let peer = [22, 2, 3, 4, 5, 6];
        let before = ps_poll_epoch(&peer);
        let mut frame = [0_u8; 16];
        frame[..2].copy_from_slice(&0x00a4_u16.to_le_bytes());
        frame[10..16].copy_from_slice(&peer);

        frame[2..4].copy_from_slice(&0x0001_u16.to_le_bytes());
        observe_frame(&frame);
        frame[2..4].copy_from_slice(&0xc002_u16.to_le_bytes());
        observe_frame(&frame);
        assert_eq!(ps_poll_epoch(&peer), before);

        frame[2..4].copy_from_slice(&0xc001_u16.to_le_bytes());
        observe_frame(&frame);
        assert_ne!(ps_poll_epoch(&peer), before);
    }

    #[test]
    fn peer_active_edges_do_not_wake_a_different_peer() {
        let _guard = test_guard();
        let peer = [7, 2, 3, 4, 5, 6];
        let other = [8, 2, 3, 4, 5, 6];
        let before = active_epoch(&peer);
        let other_before = active_epoch(&other);
        let mut frame = [0_u8; 24];
        frame[..2].copy_from_slice(&0x0108_u16.to_le_bytes());
        frame[10..16].copy_from_slice(&peer);
        observe_frame(&frame);
        assert_ne!(active_epoch(&peer), before);
        assert_eq!(active_epoch(&other), other_before);
    }

    #[test]
    fn peer_removal_cancels_only_the_matching_waiter() {
        let _guard = test_guard();
        let peer = [30, 2, 3, 4, 5, 6];
        let other = [31, 2, 3, 4, 5, 6];
        let active_before = active_epoch(&peer);
        let ps_poll_before = ps_poll_epoch(&peer);
        let removal_before = removal_epoch(&peer);
        let other_removal_before = removal_epoch(&other);
        observe_peer_removed(&peer);
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);
        assert_eq!(
            poll_peer_edge(
                active_before,
                ps_poll_before,
                removal_before,
                &peer,
                &mut context,
            ),
            Poll::Ready(PeerEdge::Removed)
        );
        assert_eq!(removal_epoch(&other), other_removal_before);
    }

    #[test]
    fn a_missing_event_slot_is_not_a_readiness_edge() {
        let _guard = test_guard();
        let peer = [0xee, 0xee, 0xee, 0xee, 0xee, 0xee];
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);
        assert_eq!(
            poll_peer_edge(123, 124, 125, &peer, &mut context),
            Poll::Pending
        );
    }
}
