//! Software boundaries of one connected maintenance transaction.
//!
//! These observations describe owner handoffs, not RF-off times or WCET.
//! Request queuing precedes this timeline. Nested PHY timings must not be
//! added to its intervals: they are already contained in the work interval.

/// Monotonic microsecond timestamps from the composition clock.
///
/// Present only for a complete physical round trip. The last edge means the
/// restored worker was handed back for scheduling, not that it was polled or
/// that an over-the-air exchange completed. Adjacent edges may be equal at
/// the clock's resolution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PauseTimeline {
    /// Maintenance selected; immediately before watchdog admission and drain request.
    pub requested: u64,
    /// Worker returned its owner after draining active TX.
    pub drained: u64,
    /// MAC stopped, RX checkpointed, IRQ epoch paused.
    pub quiesced: u64,
    /// Register arena withdrawn and exclusive PHY access admitted.
    pub acquired: u64,
    /// PHY operation or synthetic hold returned; restoration has not begun.
    pub work_completed: u64,
    /// Registers republished and RX, IRQ and MAC restored.
    pub hardware_restored: u64,
    /// Optional PM=0 exchange and physical round-trip call completed.
    pub protocol_restored: u64,
    /// Restored runner returned to the worker mailbox.
    pub worker_released: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Edge {
    Requested,
    Drained,
    Quiesced,
    Acquired,
    WorkCompleted,
    HardwareRestored,
    ProtocolRestored,
    WorkerReleased,
}

/// Incomplete, reordered, repeated or non-monotonic observations never yield
/// a success-shaped report. Reset is explicit at the next admitted request.
#[derive(Default)]
#[cfg(any(feature = "diagnostics", test))]
pub(crate) struct Recorder {
    times: [u64; 8],
    next: usize,
    invalid: bool,
}

#[cfg(any(feature = "diagnostics", test))]
impl Recorder {
    pub fn observe(&mut self, edge: Edge, now: u64) {
        if self.invalid
            || edge as usize != self.next
            || (self.next != 0 && now < self.times[self.next - 1])
        {
            self.invalid = true;
            return;
        }
        self.times[self.next] = now;
        self.next += 1;
    }

    pub fn report(&self) -> Option<PauseTimeline> {
        if self.invalid || self.next != self.times.len() {
            return None;
        }
        let [
            requested,
            drained,
            quiesced,
            acquired,
            work_completed,
            hardware_restored,
            protocol_restored,
            worker_released,
        ] = self.times;
        Some(PauseTimeline {
            requested,
            drained,
            quiesced,
            acquired,
            work_completed,
            hardware_restored,
            protocol_restored,
            worker_released,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const EDGES: [Edge; 8] = [
        Edge::Requested,
        Edge::Drained,
        Edge::Quiesced,
        Edge::Acquired,
        Edge::WorkCompleted,
        Edge::HardwareRestored,
        Edge::ProtocolRestored,
        Edge::WorkerReleased,
    ];

    #[test]
    fn every_owner_handoff_is_required_before_reporting_success() {
        let mut recorder = Recorder::default();
        for (index, edge) in EDGES.into_iter().enumerate() {
            assert!(recorder.report().is_none());
            recorder.observe(edge, 100 + index as u64);
        }
        let report = recorder.report().unwrap();
        assert_eq!(report.requested, 100);
        assert_eq!(report.worker_released, 107);
    }

    #[test]
    fn repeated_skipped_and_reversed_edges_poison_the_report() {
        for faulty in 0..EDGES.len() {
            for kind in 0..3 {
                let mut recorder = Recorder::default();
                for (index, edge) in EDGES.into_iter().enumerate() {
                    if index == faulty {
                        match kind {
                            0 => recorder.observe(edge, 100), // repeated edge below
                            1 => continue,                    // missing owner handoff
                            _ => recorder.observe(Edge::WorkerReleased, 99),
                        }
                    }
                    recorder.observe(edge, 100);
                }
                assert!(recorder.report().is_none(), "edge {faulty}, fault {kind}");
            }
        }
    }

    #[test]
    fn clock_reversal_is_rejected_but_equal_ticks_and_large_timestamps_are_valid() {
        for reversed in 1..EDGES.len() {
            let mut recorder = Recorder::default();
            for (index, edge) in EDGES.into_iter().enumerate() {
                recorder.observe(edge, if index == reversed { 99 } else { 100 });
            }
            assert!(recorder.report().is_none());
        }
        let mut recorder = Recorder::default();
        for edge in EDGES {
            recorder.observe(edge, u64::MAX);
        }
        assert!(recorder.report().is_some());
        recorder.observe(Edge::Requested, 0);
        assert!(recorder.report().is_none());
        recorder = Recorder::default();
        for edge in EDGES {
            recorder.observe(edge, 0);
        }
        assert!(recorder.report().is_some());
    }
}
