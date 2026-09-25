//! Retained deadlines for peripheral event completion and finite owner transitions.

#![forbid(unsafe_code)]

// Stop, software unlink and one Controller-time acquisition each have their
// own finite budget. Waiting for Host credits is not one of these operations.
const PERIPHERAL_OPERATION_BUDGET_MICROS: u64 = 100_000;

/// A platform-clock upper bound anchored once, never at a later RUN or retry.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PeripheralConnectionProgressDeadline {
    started: u64,
    budget_micros: u64,
    final_poll_used: bool,
}

impl PeripheralConnectionProgressDeadline {
    /// Pair the received sequence sample with platform time and its actual
    /// sequencer end (reservation end plus sequence lead). The clocks have different epochs: only the raw duration
    /// is converted. Sample-delivery latency makes this an upper bound, not a
    /// measurement of the exact physical end or a protocol-deadline proof.
    pub(crate) const fn from_sequence(
        observed_micros: u64,
        sampled_ticks: u32,
        end_ticks: u32,
        scale: crate::controller_time::BluetoothControllerTimeScale,
    ) -> Self {
        let delta = end_ticks.wrapping_sub(sampled_ticks);
        let budget = if delta as i32 > 0 {
            let duration = scale.project_raw_ticks(delta);
            duration.whole_micros as u64 + (duration.remainder_ticks != 0) as u64
        } else {
            0
        };
        Self::with_budget(observed_micros, budget)
    }

    pub(crate) const fn for_stop(now_micros: u64) -> Self {
        Self::for_operation(now_micros)
    }

    pub(crate) const fn for_operation(now_micros: u64) -> Self {
        Self::with_budget(now_micros, PERIPHERAL_OPERATION_BUDGET_MICROS)
    }

    const fn with_budget(now_micros: u64, budget_micros: u64) -> Self {
        Self {
            started: now_micros,
            budget_micros,
            final_poll_used: false,
        }
    }

    pub(crate) const fn expired(self, now_micros: u64) -> bool {
        match self.started.checked_add(self.budget_micros) {
            Some(deadline) => now_micros < self.started || now_micros >= deadline,
            None => true,
        }
    }

    /// At ordinary expiry allow exactly one final readiness observation. A
    /// durable completion already delivered at the deadline wins over timeout;
    /// repeated empty/spurious wakes cannot renew this opportunity. Clock
    /// regression or arithmetic exhaustion has no such recovery allowance.
    pub(crate) fn expired_after_final_poll(&mut self, now_micros: u64) -> bool {
        if now_micros < self.started || self.started.checked_add(self.budget_micros).is_none() {
            return true;
        }
        if !self.expired(now_micros) {
            return false;
        }
        if self.final_poll_used {
            true
        } else {
            self.final_poll_used = true;
            false
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

    fn event(observed: u64, sample: u32, end: u32) -> PeripheralConnectionProgressDeadline {
        PeripheralConnectionProgressDeadline::from_sequence(
            observed,
            sample,
            end,
            crate::controller_hal::reviewed_standalone_time_scale(),
        )
    }

    #[test]
    fn delayed_publication_and_cancellation_do_not_restart_the_event_budget() {
        let sequence = event(1_000_000, 20_000, 24_000);
        assert!(!sequence.expired(1_001_999));
        let resumed = sequence;
        assert!(resumed.expired(1_002_000));
        assert!(resumed.expired(1_100_000));
    }

    #[test]
    fn raw_wrap_and_fractional_tick_rounding_do_not_assume_equal_clock_epochs() {
        let deadline = event(5_000_000_000, u32::MAX - 99, 101);
        assert!(!deadline.expired(5_000_000_100));
        assert!(deadline.expired(5_000_000_101));
    }

    #[test]
    fn one_final_poll_preserves_completion_without_extending_repeated_waits() {
        let mut deadline = event(100, 0, 2_000);
        assert!(!deadline.expired_after_final_poll(1_099));
        assert!(!deadline.expired_after_final_poll(1_100));
        let mut resumed = deadline;
        assert!(resumed.expired_after_final_poll(1_100));
        assert!(resumed.expired_after_final_poll(1_101));
    }

    #[test]
    fn host_backpressure_does_not_spend_a_future_controller_time_budget() {
        let old_event = event(100, 0, 2_000);
        assert!(old_event.expired(50_000_000));
        let acquisition = PeripheralConnectionProgressDeadline::for_operation(50_000_000);
        assert!(!acquisition.expired(50_099_999));
        assert!(acquisition.expired(50_100_000));
    }

    #[test]
    fn clock_failure_or_unrepresentable_deadline_never_grants_an_extra_poll() {
        assert!(event(20, 0, 200).expired_after_final_poll(19));
        assert!(event(u64::MAX, 0, 200).expired_after_final_poll(u64::MAX));
        assert!(event(20, 200, 100).expired(20));
    }

    #[test]
    fn stop_and_unlink_have_independent_absolute_budgets() {
        let stop = PeripheralConnectionProgressDeadline::for_stop(500);
        assert!(stop.expired(100_500));
        let unlink = PeripheralConnectionProgressDeadline::for_operation(100_400);
        assert!(!unlink.expired(200_399));
        assert!(unlink.expired(200_400));
    }

    #[test]
    fn handle_reuse_waits_for_every_old_generation_owner() {
        assert!(retirement_barrier_is_ready(true, true, true));
        assert!(!retirement_barrier_is_ready(false, true, true));
        assert!(!retirement_barrier_is_ready(true, false, true));
        assert!(!retirement_barrier_is_ready(true, true, false));
    }
}
