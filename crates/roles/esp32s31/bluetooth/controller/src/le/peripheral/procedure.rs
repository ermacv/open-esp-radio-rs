//! Response deadline for locally initiated Link Layer control procedures.

use oer_esp32s31_bluetooth::SchedulerInstant;

/// Bluetooth `connProcedureTimeout`, fixed at 40 seconds.
pub(crate) const CONNECTION_PROCEDURE_TIMEOUT_MICROS: u32 = 40_000_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PeripheralProcedureDeadline(SchedulerInstant);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PeripheralProcedureDecision {
    Run,
    Wait,
    ConnectionLost { reason: u8 },
}

impl PeripheralProcedureDeadline {
    pub(crate) const fn new(reference: SchedulerInstant) -> Self {
        Self(reference.wrapping_add(CONNECTION_PROCEDURE_TIMEOUT_MICROS))
    }

    /// Arm or restart only when a freshly sampled control PDU entered the TX graph.
    pub(crate) const fn after_graph_update(
        current: Option<Self>,
        reference: Option<SchedulerInstant>,
        procedure_transmitted: bool,
        control_enqueued: bool,
    ) -> Option<Self> {
        match (reference, procedure_transmitted, control_enqueued) {
            (Some(reference), true, true) => Some(Self::new(reference)),
            _ => current,
        }
    }

    pub(crate) const fn decide(
        self,
        now: SchedulerInstant,
        event_start: SchedulerInstant,
    ) -> PeripheralProcedureDecision {
        if !now.is_before(self.0) {
            PeripheralProcedureDecision::ConnectionLost { reason: 0x22 }
        } else if !event_start.is_before(self.0) {
            PeripheralProcedureDecision::Wait
        } else {
            PeripheralProcedureDecision::Run
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
    fn procedure_timeout_is_armed_by_transmission_and_wrap_safe() {
        assert_eq!(
            PeripheralProcedureDeadline::after_graph_update(None, Some(at(10)), false, true),
            None
        );
        let deadline =
            PeripheralProcedureDeadline::after_graph_update(None, Some(at(10)), true, true)
                .unwrap();
        assert_eq!(
            deadline.decide(at(40_000_009), at(40_000_009)),
            PeripheralProcedureDecision::Run
        );
        assert_eq!(
            deadline.decide(at(40_000_010), at(40_000_020)),
            PeripheralProcedureDecision::ConnectionLost { reason: 0x22 }
        );

        let wrapped = PeripheralProcedureDeadline::new(at(u32::MAX - 99));
        assert_eq!(
            wrapped.decide(at(39_999_899), at(39_999_900)),
            PeripheralProcedureDecision::Wait
        );
        assert_eq!(
            wrapped.decide(at(39_999_900), at(40_000_000)),
            PeripheralProcedureDecision::ConnectionLost { reason: 0x22 }
        );
    }

    #[test]
    fn every_queued_control_pdu_restarts_an_active_procedure_timeout() {
        let initial =
            PeripheralProcedureDeadline::after_graph_update(None, Some(at(10)), true, true)
                .unwrap();
        assert_eq!(
            PeripheralProcedureDeadline::after_graph_update(
                Some(initial),
                Some(at(20)),
                true,
                false,
            ),
            Some(initial)
        );
        let restarted = PeripheralProcedureDeadline::after_graph_update(
            Some(initial),
            Some(at(20)),
            true,
            true,
        )
        .unwrap();
        assert_eq!(
            restarted.decide(at(40_000_019), at(40_000_019)),
            PeripheralProcedureDecision::Run
        );
        assert_eq!(
            restarted.decide(at(40_000_020), at(40_000_020)),
            PeripheralProcedureDecision::ConnectionLost { reason: 0x22 }
        );
    }
}
