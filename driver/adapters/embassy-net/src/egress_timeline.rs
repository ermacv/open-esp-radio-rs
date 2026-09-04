//! Diagnostic-only, serial-keyed timing of the authoritative egress protocol.
//!
//! The observer deliberately samples only grant boundaries. It never runs per
//! frame, never carries ownership, and does not alter packet, grant, progress
//! or DMA layouts. Both cores use the same Embassy timebase and join the
//! observations by the existing affine grant serial.

use core::{
    num::NonZeroU32,
    sync::atomic::{AtomicU32, Ordering},
};

use embassy_time::Instant;

const TIMELINE_SLOTS: usize = 8;

struct EgressGrantTimelineSlot {
    serial: AtomicU32,
    issued_at: AtomicU32,
    received_at: AtomicU32,
    network_finished_at: AtomicU32,
    progress_published_at: AtomicU32,
}

impl EgressGrantTimelineSlot {
    const fn new() -> Self {
        Self {
            serial: AtomicU32::new(0),
            issued_at: AtomicU32::new(0),
            received_at: AtomicU32::new(0),
            network_finished_at: AtomicU32::new(0),
            progress_published_at: AtomicU32::new(0),
        }
    }

    fn reset_for(&self, serial: NonZeroU32, issued_at: u32) -> Result<(), ()> {
        self.serial
            .compare_exchange(0, serial.get(), Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| ())?;
        self.received_at.store(0, Ordering::Relaxed);
        self.network_finished_at.store(0, Ordering::Relaxed);
        self.progress_published_at.store(0, Ordering::Relaxed);
        // The grant cannot be transported until `record_issued_at` returns.
        self.issued_at.store(issued_at, Ordering::Release);
        Ok(())
    }

    fn matches(&self, serial: NonZeroU32) -> bool {
        self.serial.load(Ordering::Acquire) == serial.get()
    }

    fn clear(&self, serial: NonZeroU32) {
        let _ = self
            .serial
            .compare_exchange(serial.get(), 0, Ordering::AcqRel, Ordering::Acquire);
    }
}

struct PhaseCounters {
    samples: AtomicU32,
    total_micros: AtomicU32,
    lifetime_max_micros: AtomicU32,
}

impl PhaseCounters {
    const fn new() -> Self {
        Self {
            samples: AtomicU32::new(0),
            total_micros: AtomicU32::new(0),
            lifetime_max_micros: AtomicU32::new(0),
        }
    }

    fn record(&self, start: u32, end: u32) -> bool {
        if start == 0 || end == 0 {
            return false;
        }
        let elapsed = end.wrapping_sub(start);
        self.samples.fetch_add(1, Ordering::Relaxed);
        self.total_micros.fetch_add(elapsed, Ordering::Relaxed);
        self.lifetime_max_micros
            .fetch_max(elapsed, Ordering::Relaxed);
        true
    }

    fn snapshot(&self) -> EgressGrantTimelinePhaseSnapshot {
        EgressGrantTimelinePhaseSnapshot {
            samples: self.samples.load(Ordering::Relaxed),
            total_micros: self.total_micros.load(Ordering::Relaxed),
            lifetime_max_micros: self.lifetime_max_micros.load(Ordering::Relaxed),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EgressGrantTimelinePhaseSnapshot {
    pub samples: u32,
    pub total_micros: u32,
    pub lifetime_max_micros: u32,
}

impl EgressGrantTimelinePhaseSnapshot {
    const fn wrapping_delta_since(self, earlier: Self) -> Self {
        Self {
            samples: self.samples.wrapping_sub(earlier.samples),
            total_micros: self.total_micros.wrapping_sub(earlier.total_micros),
            // The maximum is absolute; subtracting it would invent a sample.
            lifetime_max_micros: self.lifetime_max_micros,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EgressGrantTimelineSnapshot {
    pub grants_issued: u32,
    pub grants_completed: u32,
    pub incomplete_completions: u32,
    pub slot_collisions: u32,
    pub unmatched_events: u32,
    pub issue_to_receive: EgressGrantTimelinePhaseSnapshot,
    pub receive_to_network_finish: EgressGrantTimelinePhaseSnapshot,
    pub network_finish_to_progress_publish: EgressGrantTimelinePhaseSnapshot,
    pub progress_publish_to_radio_receive: EgressGrantTimelinePhaseSnapshot,
    pub issue_to_radio_receive: EgressGrantTimelinePhaseSnapshot,
    pub radio_receive_to_successor_issue: EgressGrantTimelinePhaseSnapshot,
}

impl EgressGrantTimelineSnapshot {
    pub fn wrapping_delta_since(self, earlier: Self) -> Self {
        Self {
            grants_issued: self.grants_issued.wrapping_sub(earlier.grants_issued),
            grants_completed: self.grants_completed.wrapping_sub(earlier.grants_completed),
            incomplete_completions: self
                .incomplete_completions
                .wrapping_sub(earlier.incomplete_completions),
            slot_collisions: self.slot_collisions.wrapping_sub(earlier.slot_collisions),
            unmatched_events: self.unmatched_events.wrapping_sub(earlier.unmatched_events),
            issue_to_receive: self
                .issue_to_receive
                .wrapping_delta_since(earlier.issue_to_receive),
            receive_to_network_finish: self
                .receive_to_network_finish
                .wrapping_delta_since(earlier.receive_to_network_finish),
            network_finish_to_progress_publish: self
                .network_finish_to_progress_publish
                .wrapping_delta_since(earlier.network_finish_to_progress_publish),
            progress_publish_to_radio_receive: self
                .progress_publish_to_radio_receive
                .wrapping_delta_since(earlier.progress_publish_to_radio_receive),
            issue_to_radio_receive: self
                .issue_to_radio_receive
                .wrapping_delta_since(earlier.issue_to_radio_receive),
            radio_receive_to_successor_issue: self
                .radio_receive_to_successor_issue
                .wrapping_delta_since(earlier.radio_receive_to_successor_issue),
        }
    }
}

/// Shared diagnostic counters. A collision is reported instead of replacing
/// an incomplete lifecycle.
pub struct EgressGrantTimelineCounters {
    slots: [EgressGrantTimelineSlot; TIMELINE_SLOTS],
    grants_issued: AtomicU32,
    grants_completed: AtomicU32,
    incomplete_completions: AtomicU32,
    slot_collisions: AtomicU32,
    unmatched_events: AtomicU32,
    last_radio_received_at: AtomicU32,
    issue_to_receive: PhaseCounters,
    receive_to_network_finish: PhaseCounters,
    network_finish_to_progress_publish: PhaseCounters,
    progress_publish_to_radio_receive: PhaseCounters,
    issue_to_radio_receive: PhaseCounters,
    radio_receive_to_successor_issue: PhaseCounters,
}

impl EgressGrantTimelineCounters {
    pub const fn new() -> Self {
        Self {
            slots: [const { EgressGrantTimelineSlot::new() }; TIMELINE_SLOTS],
            grants_issued: AtomicU32::new(0),
            grants_completed: AtomicU32::new(0),
            incomplete_completions: AtomicU32::new(0),
            slot_collisions: AtomicU32::new(0),
            unmatched_events: AtomicU32::new(0),
            last_radio_received_at: AtomicU32::new(0),
            issue_to_receive: PhaseCounters::new(),
            receive_to_network_finish: PhaseCounters::new(),
            network_finish_to_progress_publish: PhaseCounters::new(),
            progress_publish_to_radio_receive: PhaseCounters::new(),
            issue_to_radio_receive: PhaseCounters::new(),
            radio_receive_to_successor_issue: PhaseCounters::new(),
        }
    }

    fn stamp() -> u32 {
        (Instant::now().as_micros() as u32).wrapping_add(1)
    }

    fn slot(&self, serial: NonZeroU32) -> &EgressGrantTimelineSlot {
        &self.slots[(serial.get() as usize) % TIMELINE_SLOTS]
    }

    pub fn record_issued(&self, serial: NonZeroU32) {
        self.record_issued_at(serial, Self::stamp());
    }

    fn record_issued_at(&self, serial: NonZeroU32, at: u32) {
        if self.slot(serial).reset_for(serial, at).is_err() {
            self.slot_collisions.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let predecessor = self.last_radio_received_at.swap(0, Ordering::AcqRel);
        let _ = self
            .radio_receive_to_successor_issue
            .record(predecessor, at);
        self.grants_issued.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_network_received(&self, serial: NonZeroU32) {
        self.record_network_received_at(serial, Self::stamp());
    }

    fn record_network_received_at(&self, serial: NonZeroU32, at: u32) {
        let slot = self.slot(serial);
        if !slot.matches(serial) {
            self.unmatched_events.fetch_add(1, Ordering::Relaxed);
            return;
        }
        slot.received_at.store(at, Ordering::Release);
    }

    pub fn record_network_finished(&self, serial: NonZeroU32) {
        self.record_network_finished_at(serial, Self::stamp());
    }

    fn record_network_finished_at(&self, serial: NonZeroU32, at: u32) {
        let slot = self.slot(serial);
        if !slot.matches(serial) {
            self.unmatched_events.fetch_add(1, Ordering::Relaxed);
            return;
        }
        slot.network_finished_at.store(at, Ordering::Release);
    }

    pub fn record_progress_published(&self, serial: NonZeroU32) {
        self.record_progress_published_at(serial, Self::stamp());
    }

    fn record_progress_published_at(&self, serial: NonZeroU32, at: u32) {
        let slot = self.slot(serial);
        if !slot.matches(serial) {
            self.unmatched_events.fetch_add(1, Ordering::Relaxed);
            return;
        }
        slot.progress_published_at.store(at, Ordering::Release);
    }

    pub fn record_radio_received(&self, serial: NonZeroU32) {
        self.record_radio_received_at(serial, Self::stamp());
    }

    fn record_radio_received_at(&self, serial: NonZeroU32, at: u32) {
        let slot = self.slot(serial);
        if !slot.matches(serial) {
            self.unmatched_events.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let issued = slot.issued_at.load(Ordering::Acquire);
        let received = slot.received_at.load(Ordering::Acquire);
        let network_finished = slot.network_finished_at.load(Ordering::Acquire);
        let progress_published = slot.progress_published_at.load(Ordering::Acquire);

        let mut complete = self.issue_to_receive.record(issued, received);
        complete &= self
            .receive_to_network_finish
            .record(received, network_finished);
        complete &= self
            .network_finish_to_progress_publish
            .record(network_finished, progress_published);
        complete &= self
            .progress_publish_to_radio_receive
            .record(progress_published, at);
        complete &= self.issue_to_radio_receive.record(issued, at);

        if complete {
            self.grants_completed.fetch_add(1, Ordering::Relaxed);
        } else {
            self.incomplete_completions.fetch_add(1, Ordering::Relaxed);
        }
        self.last_radio_received_at.store(at, Ordering::Release);
        slot.clear(serial);
    }

    pub fn snapshot(&self) -> EgressGrantTimelineSnapshot {
        EgressGrantTimelineSnapshot {
            grants_issued: self.grants_issued.load(Ordering::Relaxed),
            grants_completed: self.grants_completed.load(Ordering::Relaxed),
            incomplete_completions: self.incomplete_completions.load(Ordering::Relaxed),
            slot_collisions: self.slot_collisions.load(Ordering::Relaxed),
            unmatched_events: self.unmatched_events.load(Ordering::Relaxed),
            issue_to_receive: self.issue_to_receive.snapshot(),
            receive_to_network_finish: self.receive_to_network_finish.snapshot(),
            network_finish_to_progress_publish: self.network_finish_to_progress_publish.snapshot(),
            progress_publish_to_radio_receive: self.progress_publish_to_radio_receive.snapshot(),
            issue_to_radio_receive: self.issue_to_radio_receive.snapshot(),
            radio_receive_to_successor_issue: self.radio_receive_to_successor_issue.snapshot(),
        }
    }
}

impl Default for EgressGrantTimelineCounters {
    fn default() -> Self {
        Self::new()
    }
}

pub static EGRESS_GRANT_TIMELINE: EgressGrantTimelineCounters = EgressGrantTimelineCounters::new();

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_serial_lifecycle_produces_complete_boundary_accounting() {
        let counters = EgressGrantTimelineCounters::new();
        let first = NonZeroU32::new(9).unwrap();
        counters.record_issued_at(first, 101);
        counters.record_network_received_at(first, 111);
        counters.record_network_finished_at(first, 141);
        counters.record_progress_published_at(first, 145);
        counters.record_radio_received_at(first, 151);
        let second = NonZeroU32::new(10).unwrap();
        counters.record_issued_at(second, 161);

        let snapshot = counters.snapshot();
        assert_eq!(snapshot.grants_issued, 2);
        assert_eq!(snapshot.grants_completed, 1);
        assert_eq!(snapshot.incomplete_completions, 0);
        assert_eq!(snapshot.issue_to_receive.total_micros, 10);
        assert_eq!(snapshot.receive_to_network_finish.total_micros, 30);
        assert_eq!(snapshot.network_finish_to_progress_publish.total_micros, 4);
        assert_eq!(snapshot.progress_publish_to_radio_receive.total_micros, 6);
        assert_eq!(snapshot.issue_to_radio_receive.total_micros, 50);
        assert_eq!(snapshot.radio_receive_to_successor_issue.total_micros, 10);
    }

    #[test]
    fn collision_and_foreign_events_fail_observation_closed() {
        let counters = EgressGrantTimelineCounters::new();
        let first = NonZeroU32::new(1).unwrap();
        let colliding = NonZeroU32::new(9).unwrap();
        counters.record_issued_at(first, 1);
        counters.record_issued_at(colliding, 2);
        counters.record_network_received_at(colliding, 3);
        let snapshot = counters.snapshot();
        assert_eq!(snapshot.grants_issued, 1);
        assert_eq!(snapshot.slot_collisions, 1);
        assert_eq!(snapshot.unmatched_events, 1);
    }
}
