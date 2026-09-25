//! Optional cumulative observations of production RX preparation and recycling.
//! Reading these counters neither acknowledges hardware nor changes ownership.

#![forbid(unsafe_code)]

#[cfg(any(target_arch = "riscv32", test))]
use super::DtmRxCompletionOutcome;
use core::cell::Cell;
use critical_section::Mutex;
#[cfg(any(target_arch = "riscv32", test))]
use oer_esp32s31_bluetooth_memory::DtmSchedulerItemCompletionStatus;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DtmRxDiagnostics {
    /// Recurring RX candidates checked against a fresh sequence sample.
    pub sequence_checks: u32,
    /// Candidates rejected before publication because their deadline expired.
    pub sequence_deadline_rejections: u32,
    /// Signed wrapping raw-tick distance from the last sample to its RX start.
    pub last_sequence_lead_ticks: i32,
    /// Recycled events with the vendor's zero scheduler status.
    pub successful_events: u32,
    /// Events reaching the recycle boundary with a nonzero scheduler status.
    pub failed_events: u32,
    /// Last nonzero scheduler result, with no inferred meaning for its bits.
    pub last_failure_status: u32,
    /// Successful events whose private RX chain returned no packet.
    pub empty_events: u32,
    /// Returned packets accepted by production DTM accounting.
    pub counted_packets: u32,
    /// Returned packets rejected by production DTM result validation.
    pub rejected_packets: u32,
}

static RX: Mutex<Cell<DtmRxDiagnostics>> = Mutex::new(Cell::new(DtmRxDiagnostics {
    sequence_checks: 0,
    sequence_deadline_rejections: 0,
    last_sequence_lead_ticks: 0,
    successful_events: 0,
    failed_events: 0,
    last_failure_status: 0,
    empty_events: 0,
    counted_packets: 0,
    rejected_packets: 0,
}));

/// Cumulative counters since boot; callers may subtract two snapshots.
pub fn snapshot() -> DtmRxDiagnostics {
    critical_section::with(|cs| RX.borrow(cs).get())
}

#[cfg(any(target_arch = "riscv32", test))]
impl DtmRxDiagnostics {
    fn observe(
        &mut self,
        status: DtmSchedulerItemCompletionStatus,
        outcome: Option<DtmRxCompletionOutcome>,
    ) {
        match status {
            DtmSchedulerItemCompletionStatus::Aborted => return,
            DtmSchedulerItemCompletionStatus::Zero => {
                self.successful_events = self.successful_events.wrapping_add(1)
            }
            DtmSchedulerItemCompletionStatus::NonZero(value) => {
                self.failed_events = self.failed_events.wrapping_add(1);
                self.last_failure_status = value.get();
            }
        }
        match outcome {
            Some(DtmRxCompletionOutcome::NoReturnedPacket) => {
                self.empty_events = self.empty_events.wrapping_add(1)
            }
            Some(DtmRxCompletionOutcome::Counted { .. }) => {
                self.counted_packets = self.counted_packets.wrapping_add(1)
            }
            Some(DtmRxCompletionOutcome::NotCounted { .. }) => {
                self.rejected_packets = self.rejected_packets.wrapping_add(1)
            }
            None => {}
        }
    }
}

#[cfg(target_arch = "riscv32")]
pub(super) fn record(
    status: DtmSchedulerItemCompletionStatus,
    outcome: Option<DtmRxCompletionOutcome>,
) {
    critical_section::with(|cs| {
        let cell = RX.borrow(cs);
        let mut counters = cell.get();
        counters.observe(status, outcome);
        cell.set(counters);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn distinguishes_empty_receive_from_failed_event_and_rejected_packet() {
        let mut counters = DtmRxDiagnostics::default();
        counters.observe(
            DtmSchedulerItemCompletionStatus::Zero,
            Some(DtmRxCompletionOutcome::NoReturnedPacket),
        );
        counters.observe(
            DtmSchedulerItemCompletionStatus::NonZero(core::num::NonZeroU32::new(7).unwrap()),
            None,
        );
        counters.observe(DtmSchedulerItemCompletionStatus::Zero, Some(DtmRxCompletionOutcome::NotCounted { error: oer_esp32s31_bluetooth_memory::DtmRxResultProjectionError::NonzeroLowTwentyFourBits }));
        assert_eq!(counters.successful_events, 2);
        assert_eq!(counters.failed_events, 1);
        assert_eq!(counters.last_failure_status, 7);
        assert_eq!(counters.empty_events, 1);
        assert_eq!(counters.rejected_packets, 1);
        assert_eq!(counters.counted_packets, 0);
    }
    #[test]
    fn aborted_event_is_not_a_packet_or_a_hardware_error() {
        let mut counters = DtmRxDiagnostics::default();
        let before = counters;
        counters.observe(DtmSchedulerItemCompletionStatus::Aborted, None);
        assert_eq!(counters, before);
    }
}

#[cfg(target_arch = "riscv32")]
pub(crate) fn record_sequence(lead_ticks: i32, rejected: bool) {
    critical_section::with(|cs| {
        let cell = RX.borrow(cs);
        let mut counters = cell.get();
        counters.sequence_checks = counters.sequence_checks.wrapping_add(1);
        counters.sequence_deadline_rejections = counters
            .sequence_deadline_rejections
            .wrapping_add(u32::from(rejected));
        counters.last_sequence_lead_ticks = lead_ticks;
        cell.set(counters);
    });
}
