//! One decision for all protocol deadlines at the same fresh Controller time.
//!
//! A future event blocked by one timer must not hide another timer's expiry.
//! This gate owns no radio authority and never renews a deadline. The caller
//! retains the candidate and performs the ordinary cancellation/retirement.

use super::{
    procedure::{PeripheralProcedureDeadline, PeripheralProcedureDecision},
    supervision::{PeripheralSupervisionDeadline, PeripheralSupervisionDecision},
    termination::{PeripheralTerminationDeadline, PeripheralTerminationDecision},
};
use crate::SchedulerInstant;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Expired {
    Termination,
    Procedure { reason: u8 },
    Supervision,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Decision {
    Run,
    Wait,
    Expired(Expired),
}

#[derive(Clone, Copy)]
pub(crate) struct Deadlines {
    pub(crate) termination: Option<PeripheralTerminationDeadline>,
    pub(crate) procedure: Option<PeripheralProcedureDeadline>,
    pub(crate) supervision: Option<PeripheralSupervisionDeadline>,
}

impl Deadlines {
    /// Preserve termination/procedure/supervision precedence when several have
    /// expired at one observation. A future deadline never masks an expired one.
    pub(crate) fn decide(self, now: SchedulerInstant, event_start: SchedulerInstant) -> Decision {
        let mut wait = false;
        if let Some(deadline) = self.termination {
            match deadline.decide(now, event_start) {
                PeripheralTerminationDecision::Expired => {
                    return Decision::Expired(Expired::Termination);
                }
                PeripheralTerminationDecision::Wait => wait = true,
                PeripheralTerminationDecision::Run => {}
            }
        }
        if let Some(deadline) = self.procedure {
            match deadline.decide(now, event_start) {
                PeripheralProcedureDecision::ConnectionLost { reason } => {
                    return Decision::Expired(Expired::Procedure { reason });
                }
                PeripheralProcedureDecision::Wait => wait = true,
                PeripheralProcedureDecision::Run => {}
            }
        }
        if let Some(deadline) = self.supervision {
            match deadline.decide(now, event_start) {
                PeripheralSupervisionDecision::Expired => {
                    return Decision::Expired(Expired::Supervision);
                }
                PeripheralSupervisionDecision::Wait => wait = true,
                PeripheralSupervisionDecision::Run => {}
            }
        }
        if wait { Decision::Wait } else { Decision::Run }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(micros: u32) -> SchedulerInstant {
        SchedulerInstant::from_image(micros)
    }

    #[test]
    fn termination_wait_cannot_hide_expired_supervision() {
        let deadlines = Deadlines {
            termination: Some(PeripheralTerminationDeadline::new(at(1_000_000), 2_000_000)),
            procedure: None,
            supervision: Some(PeripheralSupervisionDeadline::new(at(0), 2_000_000)),
        };
        assert_eq!(
            deadlines.decide(at(2_000_000), at(3_000_000)),
            Decision::Expired(Expired::Supervision)
        );
    }

    #[test]
    fn procedure_wait_cannot_hide_expired_supervision() {
        let deadlines = Deadlines {
            termination: None,
            procedure: Some(PeripheralProcedureDeadline::new(at(0))),
            supervision: Some(PeripheralSupervisionDeadline::new(
                at(37_000_000),
                2_000_000,
            )),
        };
        assert_eq!(
            deadlines.decide(at(39_000_000), at(40_000_000)),
            Decision::Expired(Expired::Supervision)
        );
    }

    #[test]
    fn termination_wait_cannot_hide_expired_procedure() {
        let deadlines = Deadlines {
            termination: Some(PeripheralTerminationDeadline::new(
                at(39_000_000),
                2_000_000,
            )),
            procedure: Some(PeripheralProcedureDeadline::new(at(0))),
            supervision: Some(PeripheralSupervisionDeadline::new(
                at(39_000_000),
                2_000_000,
            )),
        };
        assert_eq!(
            deadlines.decide(at(40_000_000), at(41_000_000)),
            Decision::Expired(Expired::Procedure { reason: 0x22 })
        );
    }

    #[test]
    fn blocked_reservation_waits_without_renewing_any_timer() {
        let deadlines = Deadlines {
            termination: None,
            procedure: Some(PeripheralProcedureDeadline::new(at(0))),
            supervision: Some(PeripheralSupervisionDeadline::new(at(0), 2_000_000)),
        };
        assert_eq!(
            deadlines.decide(at(1_900_000), at(1_999_999)),
            Decision::Run
        );
        for current in [1_900_000, 1_999_999] {
            assert_eq!(deadlines.decide(at(current), at(2_000_000)), Decision::Wait);
        }
        assert_eq!(
            deadlines.decide(at(2_000_000), at(2_000_000)),
            Decision::Expired(Expired::Supervision)
        );
        // The CPU-owned ACL backpressure path has no future reservation. It
        // uses the same expiry decision with the current time in both slots.
        assert_eq!(
            deadlines.decide(at(2_000_000), at(2_000_000)),
            deadlines.decide(at(2_000_000), at(40_000_000))
        );
    }

    #[test]
    fn wait_does_not_hide_expiry_across_scheduler_wrap() {
        let reference = at(u32::MAX - 49_999);
        let deadlines = Deadlines {
            termination: Some(PeripheralTerminationDeadline::new(reference, 200_000)),
            procedure: None,
            supervision: Some(PeripheralSupervisionDeadline::new(reference, 100_000)),
        };
        assert_eq!(deadlines.decide(at(49_999), at(150_000)), Decision::Wait);
        assert_eq!(
            deadlines.decide(at(50_000), at(150_000)),
            Decision::Expired(Expired::Supervision)
        );
    }

    #[test]
    fn simultaneous_expirations_preserve_existing_reason_precedence() {
        let mut deadlines = Deadlines {
            termination: Some(PeripheralTerminationDeadline::new(
                at(39_000_000),
                1_000_000,
            )),
            procedure: Some(PeripheralProcedureDeadline::new(at(0))),
            supervision: Some(PeripheralSupervisionDeadline::new(
                at(39_000_000),
                1_000_000,
            )),
        };
        assert_eq!(
            deadlines.decide(at(40_000_000), at(40_000_000)),
            Decision::Expired(Expired::Termination)
        );
        deadlines.termination = None;
        assert_eq!(
            deadlines.decide(at(40_000_000), at(40_000_000)),
            Decision::Expired(Expired::Procedure { reason: 0x22 })
        );
        deadlines.procedure = None;
        assert_eq!(
            deadlines.decide(at(40_000_000), at(40_000_000)),
            Decision::Expired(Expired::Supervision)
        );
        deadlines.supervision = None;
        assert_eq!(
            deadlines.decide(at(40_000_000), at(40_000_000)),
            Decision::Run
        );
    }
}
