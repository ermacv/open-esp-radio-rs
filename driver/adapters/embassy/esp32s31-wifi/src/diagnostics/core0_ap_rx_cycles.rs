//! Low-overhead phase timing for successful AP protected-data ingress.

use core::sync::atomic::{AtomicU32, Ordering};

use super::core0_rx_cycles::cycle_count;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Core0ApRxCycleSnapshot {
    pub calls: u32,
    pub total: u32,
    pub view: u32,
    pub dispatch: u32,
    pub dispatch_leaf: u32,
    pub reorder_key: u32,
    pub leaf_peer: u32,
    pub leaf_publish_check: u32,
    pub leaf_body: u32,
    pub leaf_admission: u32,
    pub leaf_observe: u32,
    pub publication: u32,
    pub activity_tail: u32,
    pub telemetry: u32,
}

impl Core0ApRxCycleSnapshot {
    pub fn wrapping_delta_since(self, earlier: Self) -> Self {
        Self {
            calls: self.calls.wrapping_sub(earlier.calls),
            total: self.total.wrapping_sub(earlier.total),
            view: self.view.wrapping_sub(earlier.view),
            dispatch: self.dispatch.wrapping_sub(earlier.dispatch),
            dispatch_leaf: self.dispatch_leaf.wrapping_sub(earlier.dispatch_leaf),
            reorder_key: self.reorder_key.wrapping_sub(earlier.reorder_key),
            leaf_peer: self.leaf_peer.wrapping_sub(earlier.leaf_peer),
            leaf_publish_check: self
                .leaf_publish_check
                .wrapping_sub(earlier.leaf_publish_check),
            leaf_body: self.leaf_body.wrapping_sub(earlier.leaf_body),
            leaf_admission: self.leaf_admission.wrapping_sub(earlier.leaf_admission),
            leaf_observe: self.leaf_observe.wrapping_sub(earlier.leaf_observe),
            publication: self.publication.wrapping_sub(earlier.publication),
            activity_tail: self.activity_tail.wrapping_sub(earlier.activity_tail),
            telemetry: self.telemetry.wrapping_sub(earlier.telemetry),
        }
    }
}

pub struct Core0ApRxCycleCounters {
    calls: AtomicU32,
    total: AtomicU32,
    view: AtomicU32,
    dispatch: AtomicU32,
    dispatch_leaf: AtomicU32,
    reorder_key: AtomicU32,
    leaf_peer: AtomicU32,
    leaf_publish_check: AtomicU32,
    leaf_body: AtomicU32,
    leaf_admission: AtomicU32,
    leaf_observe: AtomicU32,
    publication: AtomicU32,
    activity_tail: AtomicU32,
    telemetry: AtomicU32,
}

impl Core0ApRxCycleCounters {
    const fn new() -> Self {
        Self {
            calls: AtomicU32::new(0),
            total: AtomicU32::new(0),
            view: AtomicU32::new(0),
            dispatch: AtomicU32::new(0),
            dispatch_leaf: AtomicU32::new(0),
            reorder_key: AtomicU32::new(0),
            leaf_peer: AtomicU32::new(0),
            leaf_publish_check: AtomicU32::new(0),
            leaf_body: AtomicU32::new(0),
            leaf_admission: AtomicU32::new(0),
            leaf_observe: AtomicU32::new(0),
            publication: AtomicU32::new(0),
            activity_tail: AtomicU32::new(0),
            telemetry: AtomicU32::new(0),
        }
    }

    fn record(&self, profile: Core0ApRxCycleProfile, ended: u32) {
        let telemetry_started = cycle_count();
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.total
            .fetch_add(ended.wrapping_sub(profile.started), Ordering::Relaxed);
        self.view.fetch_add(profile.view, Ordering::Relaxed);
        self.dispatch.fetch_add(profile.dispatch, Ordering::Relaxed);
        self.dispatch_leaf
            .fetch_add(profile.dispatch_leaf, Ordering::Relaxed);
        self.reorder_key
            .fetch_add(profile.reorder_key, Ordering::Relaxed);
        self.leaf_peer
            .fetch_add(profile.leaf_peer, Ordering::Relaxed);
        self.leaf_publish_check
            .fetch_add(profile.leaf_publish_check, Ordering::Relaxed);
        self.leaf_body
            .fetch_add(profile.leaf_body, Ordering::Relaxed);
        self.leaf_admission
            .fetch_add(profile.leaf_admission, Ordering::Relaxed);
        self.leaf_observe
            .fetch_add(profile.leaf_observe, Ordering::Relaxed);
        self.publication
            .fetch_add(profile.publication, Ordering::Relaxed);
        self.activity_tail
            .fetch_add(ended.wrapping_sub(profile.last), Ordering::Relaxed);
        self.telemetry.fetch_add(
            cycle_count().wrapping_sub(telemetry_started),
            Ordering::Relaxed,
        );
    }

    pub fn snapshot(&self) -> Core0ApRxCycleSnapshot {
        Core0ApRxCycleSnapshot {
            calls: self.calls.load(Ordering::Relaxed),
            total: self.total.load(Ordering::Relaxed),
            view: self.view.load(Ordering::Relaxed),
            dispatch: self.dispatch.load(Ordering::Relaxed),
            dispatch_leaf: self.dispatch_leaf.load(Ordering::Relaxed),
            reorder_key: self.reorder_key.load(Ordering::Relaxed),
            leaf_peer: self.leaf_peer.load(Ordering::Relaxed),
            leaf_publish_check: self.leaf_publish_check.load(Ordering::Relaxed),
            leaf_body: self.leaf_body.load(Ordering::Relaxed),
            leaf_admission: self.leaf_admission.load(Ordering::Relaxed),
            leaf_observe: self.leaf_observe.load(Ordering::Relaxed),
            publication: self.publication.load(Ordering::Relaxed),
            activity_tail: self.activity_tail.load(Ordering::Relaxed),
            telemetry: self.telemetry.load(Ordering::Relaxed),
        }
    }
}

pub static CORE0_AP_RX_CYCLES: Core0ApRxCycleCounters = Core0ApRxCycleCounters::new();

pub(crate) struct Core0ApRxCycleProfile {
    started: u32,
    last: u32,
    view: u32,
    dispatch: u32,
    dispatch_leaf: u32,
    reorder_key: u32,
    leaf_peer: u32,
    leaf_publish_check: u32,
    leaf_body: u32,
    leaf_admission: u32,
    leaf_observe: u32,
    publication: u32,
}

impl Core0ApRxCycleProfile {
    pub(crate) fn begin() -> Self {
        let now = cycle_count();
        Self {
            started: now,
            last: now,
            view: 0,
            dispatch: 0,
            dispatch_leaf: 0,
            reorder_key: 0,
            leaf_peer: 0,
            leaf_publish_check: 0,
            leaf_body: 0,
            leaf_admission: 0,
            leaf_observe: 0,
            publication: 0,
        }
    }

    pub(crate) fn view_complete(&mut self) {
        let now = cycle_count();
        self.view = now.wrapping_sub(self.last);
        self.last = now;
    }

    pub(crate) fn dispatch_complete(&mut self, leaf_cycles: u32) {
        let now = cycle_count();
        self.dispatch = now.wrapping_sub(self.last);
        self.dispatch_leaf = leaf_cycles;
        self.last = now;
    }

    pub(crate) fn record_reorder_key(&mut self, cycles: u32) {
        self.reorder_key = self.reorder_key.wrapping_add(cycles);
    }

    pub(crate) fn record_leaf_peer(&mut self, cycles: u32) {
        self.leaf_peer = self.leaf_peer.wrapping_add(cycles);
    }

    pub(crate) fn record_leaf_publish_check(&mut self, cycles: u32) {
        self.leaf_publish_check = self.leaf_publish_check.wrapping_add(cycles);
    }

    pub(crate) fn record_leaf_body(&mut self, cycles: u32) {
        self.leaf_body = self.leaf_body.wrapping_add(cycles);
    }

    pub(crate) fn record_leaf_admission(&mut self, cycles: u32) {
        self.leaf_admission = self.leaf_admission.wrapping_add(cycles);
    }

    pub(crate) fn record_leaf_observe(&mut self, cycles: u32) {
        self.leaf_observe = self.leaf_observe.wrapping_add(cycles);
    }

    pub(crate) fn publication_complete(&mut self) {
        let now = cycle_count();
        self.publication = now.wrapping_sub(self.last);
        self.last = now;
    }

    pub(crate) fn finish(self) {
        let ended = cycle_count();
        CORE0_AP_RX_CYCLES.record(self, ended);
    }
}
