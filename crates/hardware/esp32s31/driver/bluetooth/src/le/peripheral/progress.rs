//! Absolute completion budget for one published peripheral radio event.

#![forbid(unsafe_code)]

// The maximum LE supervision timeout is 32 seconds. A published event must
// either complete or fail safely within a larger fixed owner-retained budget,
// even when its IRQ edge is lost.
const PERIPHERAL_COMPLETION_BUDGET_MICROS: u64 = 40_000_000;
const PERIPHERAL_STOP_BUDGET_MICROS: u64 = 100_000;

/// Absolute monotonic deadline retained across executor wakes and cancellation.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PeripheralConnectionProgressDeadline {
    started: u64,
    expires: Option<u64>,
}

impl PeripheralConnectionProgressDeadline {
    pub(crate) const fn new(now_micros: u64) -> Self {
        Self::with_budget(now_micros, PERIPHERAL_COMPLETION_BUDGET_MICROS)
    }

    pub(crate) const fn for_stop(now_micros: u64) -> Self {
        Self::with_budget(now_micros, PERIPHERAL_STOP_BUDGET_MICROS)
    }

    const fn with_budget(now_micros: u64, budget_micros: u64) -> Self {
        Self {
            started: now_micros,
            expires: now_micros.checked_add(budget_micros),
        }
    }

    pub(crate) const fn expired(self, now_micros: u64) -> bool {
        match self.expires {
            Some(deadline) => now_micros < self.started || now_micros >= deadline,
            None => true,
        }
    }
}

/// Equivalent generation barrier for the sole reused HCI connection handle.
/// The next connection cannot reach idle admission while any old Host-visible
/// event, queued packet, or Host-owned Controller ACL buffer remains.
pub(crate) const fn retirement_barrier_is_ready(
    host_events_complete: bool,
    controller_queue_empty: bool,
    controller_credits_settled: bool,
) -> bool {
    host_events_complete && controller_queue_empty && controller_credits_settled
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deadline_is_absolute_across_copied_resume_state() {
        let deadline = PeripheralConnectionProgressDeadline::new(20);
        assert!(!deadline.expired(20));
        assert!(!deadline.expired(40_000_019));
        let resumed = deadline;
        assert!(resumed.expired(40_000_020));
    }

    #[test]
    fn clock_discontinuity_and_unrepresentable_deadline_fail_closed() {
        assert!(PeripheralConnectionProgressDeadline::new(20).expired(19));
        assert!(PeripheralConnectionProgressDeadline::new(u64::MAX).expired(u64::MAX));
    }

    #[test]
    fn hardware_stop_has_a_separate_finite_budget() {
        let deadline = PeripheralConnectionProgressDeadline::for_stop(500);
        assert!(!deadline.expired(100_499));
        assert!(deadline.expired(100_500));
    }

    #[test]
    fn handle_reuse_waits_for_every_old_generation_owner() {
        assert!(retirement_barrier_is_ready(true, true, true));
        assert!(!retirement_barrier_is_ready(false, true, true));
        assert!(!retirement_barrier_is_ready(true, false, true));
        assert!(!retirement_barrier_is_ready(true, true, false));
    }
}
