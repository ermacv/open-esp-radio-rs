//! Core0 RX reorder-ingress phase accounting.
//!
//! This diagnostic profile ends before Ethernet dispatch. It decomposes the
//! previously opaque dequeue-to-dispatch interval without attributing all of
//! that time to either the scheduler or BlockAck logic by assumption.

use core::sync::atomic::{AtomicU32, Ordering};

use super::core0_rx_cycles::cycle_count;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Core0ReorderSnapshot {
    pub calls: u32,
    pub no_key: u32,
    pub inactive: u32,
    pub immediate: u32,
    pub slow: u32,
    pub total: u32,
    pub key: u32,
    pub bank: u32,
    pub ingress_observer: u32,
    pub first: u32,
    pub ingest: u32,
    pub deadline: u32,
    pub release_observer: u32,
    pub occupied_observer: u32,
    pub prepared_observer: u32,
    pub tail: u32,
    pub telemetry_record: u32,
}

impl Core0ReorderSnapshot {
    pub fn wrapping_delta_since(self, earlier: Self) -> Self {
        Self {
            calls: self.calls.wrapping_sub(earlier.calls),
            no_key: self.no_key.wrapping_sub(earlier.no_key),
            inactive: self.inactive.wrapping_sub(earlier.inactive),
            immediate: self.immediate.wrapping_sub(earlier.immediate),
            slow: self.slow.wrapping_sub(earlier.slow),
            total: self.total.wrapping_sub(earlier.total),
            key: self.key.wrapping_sub(earlier.key),
            bank: self.bank.wrapping_sub(earlier.bank),
            ingress_observer: self.ingress_observer.wrapping_sub(earlier.ingress_observer),
            first: self.first.wrapping_sub(earlier.first),
            ingest: self.ingest.wrapping_sub(earlier.ingest),
            deadline: self.deadline.wrapping_sub(earlier.deadline),
            release_observer: self.release_observer.wrapping_sub(earlier.release_observer),
            occupied_observer: self
                .occupied_observer
                .wrapping_sub(earlier.occupied_observer),
            prepared_observer: self
                .prepared_observer
                .wrapping_sub(earlier.prepared_observer),
            tail: self.tail.wrapping_sub(earlier.tail),
            telemetry_record: self.telemetry_record.wrapping_sub(earlier.telemetry_record),
        }
    }
}

pub struct Core0ReorderCounters {
    calls: AtomicU32,
    no_key: AtomicU32,
    inactive: AtomicU32,
    immediate: AtomicU32,
    slow: AtomicU32,
    total: AtomicU32,
    key: AtomicU32,
    bank: AtomicU32,
    ingress_observer: AtomicU32,
    first: AtomicU32,
    ingest: AtomicU32,
    deadline: AtomicU32,
    release_observer: AtomicU32,
    occupied_observer: AtomicU32,
    prepared_observer: AtomicU32,
    tail: AtomicU32,
    telemetry_record: AtomicU32,
    active_last: AtomicU32,
}

impl Core0ReorderCounters {
    pub const fn new() -> Self {
        Self {
            calls: AtomicU32::new(0),
            no_key: AtomicU32::new(0),
            inactive: AtomicU32::new(0),
            immediate: AtomicU32::new(0),
            slow: AtomicU32::new(0),
            total: AtomicU32::new(0),
            key: AtomicU32::new(0),
            bank: AtomicU32::new(0),
            ingress_observer: AtomicU32::new(0),
            first: AtomicU32::new(0),
            ingest: AtomicU32::new(0),
            deadline: AtomicU32::new(0),
            release_observer: AtomicU32::new(0),
            occupied_observer: AtomicU32::new(0),
            prepared_observer: AtomicU32::new(0),
            tail: AtomicU32::new(0),
            telemetry_record: AtomicU32::new(0),
            active_last: AtomicU32::new(0),
        }
    }

    #[inline(always)]
    fn record_phase(&self, phase: &AtomicU32) {
        let phase_ended = cycle_count();
        let elapsed = phase_ended.wrapping_sub(self.active_last.load(Ordering::Relaxed));
        let telemetry_started = cycle_count();
        phase.fetch_add(elapsed, Ordering::Relaxed);
        self.total.fetch_add(elapsed, Ordering::Relaxed);
        let telemetry_ended = cycle_count();
        self.telemetry_record.fetch_add(
            telemetry_ended.wrapping_sub(telemetry_started),
            Ordering::Relaxed,
        );
        self.active_last.store(telemetry_ended, Ordering::Relaxed);
    }

    #[inline(always)]
    fn record_path(&self, path: Core0ReorderPath) {
        let telemetry_started = cycle_count();
        self.calls.fetch_add(1, Ordering::Relaxed);
        match path {
            Core0ReorderPath::NoKey => self.no_key.fetch_add(1, Ordering::Relaxed),
            Core0ReorderPath::Inactive => self.inactive.fetch_add(1, Ordering::Relaxed),
            Core0ReorderPath::Immediate => self.immediate.fetch_add(1, Ordering::Relaxed),
            Core0ReorderPath::Slow => self.slow.fetch_add(1, Ordering::Relaxed),
        };
        self.telemetry_record.fetch_add(
            cycle_count().wrapping_sub(telemetry_started),
            Ordering::Relaxed,
        );
    }

    /// Begin one synchronous reorder-ingress transaction.
    ///
    /// The connected protocol has one Core0 execution owner, so this active
    /// timestamp cannot be nested or observed by another caller.
    #[inline(always)]
    pub(crate) fn begin(&self) {
        self.active_last.store(cycle_count(), Ordering::Relaxed);
    }

    #[inline(always)]
    pub(crate) fn key_completed(&self) {
        self.record_phase(&self.key);
    }

    #[inline(always)]
    pub(crate) fn bank_completed(&self) {
        self.record_phase(&self.bank);
    }

    #[inline(always)]
    pub(crate) fn ingress_observer_completed(&self) {
        self.record_phase(&self.ingress_observer);
    }

    #[inline(always)]
    pub(crate) fn first_completed(&self) {
        self.record_phase(&self.first);
    }

    #[inline(always)]
    pub(crate) fn ingest_completed(&self) {
        self.record_phase(&self.ingest);
    }

    #[inline(always)]
    pub(crate) fn deadline_completed(&self) {
        self.record_phase(&self.deadline);
    }

    #[inline(always)]
    pub(crate) fn release_observer_completed(&self) {
        self.record_phase(&self.release_observer);
    }

    #[inline(always)]
    pub(crate) fn occupied_observer_completed(&self) {
        self.record_phase(&self.occupied_observer);
    }

    #[inline(always)]
    pub(crate) fn prepared_observer_completed(&self) {
        self.record_phase(&self.prepared_observer);
    }

    #[inline(always)]
    pub(crate) fn finish(&self, path: Core0ReorderPath) {
        self.record_phase(&self.tail);
        self.record_path(path);
    }

    pub fn snapshot(&self) -> Core0ReorderSnapshot {
        Core0ReorderSnapshot {
            calls: self.calls.load(Ordering::Relaxed),
            no_key: self.no_key.load(Ordering::Relaxed),
            inactive: self.inactive.load(Ordering::Relaxed),
            immediate: self.immediate.load(Ordering::Relaxed),
            slow: self.slow.load(Ordering::Relaxed),
            total: self.total.load(Ordering::Relaxed),
            key: self.key.load(Ordering::Relaxed),
            bank: self.bank.load(Ordering::Relaxed),
            ingress_observer: self.ingress_observer.load(Ordering::Relaxed),
            first: self.first.load(Ordering::Relaxed),
            ingest: self.ingest.load(Ordering::Relaxed),
            deadline: self.deadline.load(Ordering::Relaxed),
            release_observer: self.release_observer.load(Ordering::Relaxed),
            occupied_observer: self.occupied_observer.load(Ordering::Relaxed),
            prepared_observer: self.prepared_observer.load(Ordering::Relaxed),
            tail: self.tail.load(Ordering::Relaxed),
            telemetry_record: self.telemetry_record.load(Ordering::Relaxed),
        }
    }
}

impl Default for Core0ReorderCounters {
    fn default() -> Self {
        Self::new()
    }
}

pub static CORE0_REORDER_CYCLES: Core0ReorderCounters = Core0ReorderCounters::new();

#[derive(Clone, Copy)]
pub(crate) enum Core0ReorderPath {
    NoKey,
    Inactive,
    Immediate,
    Slow,
}

#[cfg(test)]
mod tests {
    use super::Core0ReorderSnapshot;

    #[test]
    fn interval_snapshot_uses_wrapping_deltas() {
        let earlier = Core0ReorderSnapshot {
            calls: u32::MAX,
            total: 80,
            ..Core0ReorderSnapshot::default()
        };
        let current = Core0ReorderSnapshot {
            calls: 2,
            total: 130,
            ..Core0ReorderSnapshot::default()
        };
        let delta = current.wrapping_delta_since(earlier);
        assert_eq!(delta.calls, 3);
        assert_eq!(delta.total, 50);
    }
}
