//! Elapsed-time limit for a locally initiated ACL Termination procedure.

use crate::SchedulerInstant;

/// Deadline armed from the first fresh Controller sample immediately before
/// `LL_TERMINATE_IND` is accepted by the CPU-owned transmit graph.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PeripheralTerminationDeadline(SchedulerInstant);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PeripheralTerminationDecision {
    Run,
    Wait,
    Expired,
}

impl PeripheralTerminationDeadline {
    pub(crate) const fn new(reference: SchedulerInstant, timeout_micros: u32) -> Self {
        Self(reference.wrapping_add(timeout_micros))
    }

    /// Arm only when the sampled transition actually moved the termination
    /// response from the portable queue into the transmit allocation.
    pub(crate) const fn after_graph_update(
        reference: Option<SchedulerInstant>,
        timeout_micros: u32,
        still_queued: bool,
        retained_reason: Option<u8>,
    ) -> Option<Self> {
        match (reference, still_queued, retained_reason) {
            (Some(reference), false, Some(_)) => Some(Self::new(reference, timeout_micros)),
            _ => None,
        }
    }

    /// Stop publication at the deadline while allowing an event which starts
    /// before it to finish and provide the terminating PDU's acknowledgement.
    pub(crate) const fn decide(
        self,
        now: SchedulerInstant,
        event_start: SchedulerInstant,
    ) -> PeripheralTerminationDecision {
        if !now.is_before(self.0) {
            PeripheralTerminationDecision::Expired
        } else if !event_start.is_before(self.0) {
            PeripheralTerminationDecision::Wait
        } else {
            PeripheralTerminationDecision::Run
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(micros: u32) -> SchedulerInstant {
        SchedulerInstant::from_image(micros)
    }

    #[test]
    fn termination_uses_the_connection_supervision_duration() {
        let deadline = PeripheralTerminationDeadline::new(at(10_000), 2_000_000);
        assert_eq!(
            deadline.decide(at(1_900_000), at(2_000_000)),
            PeripheralTerminationDecision::Run
        );
        assert_eq!(
            deadline.decide(at(2_009_999), at(2_010_000)),
            PeripheralTerminationDecision::Wait
        );
        assert_eq!(
            deadline.decide(at(2_010_000), at(2_020_000)),
            PeripheralTerminationDecision::Expired
        );
    }

    #[test]
    fn arming_requires_the_sampled_pdu_to_enter_the_tx_graph() {
        let reference = at(10_000);
        assert_eq!(
            PeripheralTerminationDeadline::after_graph_update(
                Some(reference),
                2_000_000,
                false,
                Some(0x16),
            ),
            Some(PeripheralTerminationDeadline::new(reference, 2_000_000))
        );
        assert_eq!(
            PeripheralTerminationDeadline::after_graph_update(
                Some(reference),
                2_000_000,
                true,
                Some(0x16),
            ),
            None
        );
        assert_eq!(
            PeripheralTerminationDeadline::after_graph_update(None, 2_000_000, false, Some(0x16),),
            None
        );
    }

    #[test]
    fn deadline_wrap_does_not_expire_early_or_late() {
        let deadline = PeripheralTerminationDeadline::new(at(u32::MAX - 99), 100_000);
        assert_eq!(
            deadline.decide(at(u32::MAX), at(0)),
            PeripheralTerminationDecision::Run
        );
        assert_eq!(
            deadline.decide(at(99_899), at(99_900)),
            PeripheralTerminationDecision::Wait
        );
        assert_eq!(
            deadline.decide(at(99_900), at(100_000)),
            PeripheralTerminationDecision::Expired
        );
    }
}
