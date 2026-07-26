use core::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use crate::{
    adapter::{blocking_probe, clear_task_delay_snapshot},
    allocation::{allocation_probe, AllocationProbe, AllocationSnapshot},
    critical::{critical_section_probe, CriticalSectionProbe, CriticalSectionSnapshot},
    diagnostics::{BlockingCall, BlockingCallProbe},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StrictPolicy {
    pub deny_allocations: bool,
    pub deny_other_core_stalls: bool,
    pub max_interrupt_nesting: usize,
}

impl StrictPolicy {
    pub const fn heap_free_single_owner() -> Self {
        Self {
            deny_allocations: true,
            deny_other_core_stalls: true,
            max_interrupt_nesting: 8,
        }
    }
}

impl Default for StrictPolicy {
    fn default() -> Self {
        Self::heap_free_single_owner()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StrictViolation {
    BlockingCall {
        call: u32,
        event: u32,
        argument: usize,
    },
    Allocation {
        calls: usize,
        requested_bytes: usize,
    },
    AllocationFailure,
    CriticalSectionStillActive(usize),
    UnmatchedInterruptRestore(usize),
    InterruptNestingExceeded(usize),
    OtherCoreStall(usize),
    WrongWifiHart(usize),
}

/// A checkpoint over the runtime probes. Install the allocator and critical
/// section wrappers before relying on their corresponding strict checks.
pub struct StrictAudit<'a> {
    policy: StrictPolicy,
    blocking: &'a BlockingCallProbe,
    allocation: &'a AllocationProbe,
    critical: &'a CriticalSectionProbe,
    allocation_baseline: AllocationSnapshot,
    critical_baseline: CriticalSectionSnapshot,
}

impl StrictAudit<'static> {
    pub fn global(policy: StrictPolicy) -> Self {
        Self::new(
            policy,
            blocking_probe(),
            allocation_probe(),
            critical_section_probe(),
        )
    }
}

impl<'a> StrictAudit<'a> {
    pub fn new(
        policy: StrictPolicy,
        blocking: &'a BlockingCallProbe,
        allocation: &'a AllocationProbe,
        critical: &'a CriticalSectionProbe,
    ) -> Self {
        blocking.clear();
        clear_task_delay_snapshot();
        critical.clear();
        Self {
            policy,
            blocking,
            allocation,
            critical,
            allocation_baseline: allocation.snapshot(),
            critical_baseline: critical.snapshot(),
        }
    }

    pub fn check(&self) -> Result<(), StrictViolation> {
        let (call, event, argument) = self.blocking.raw();
        if call != BlockingCall::None as u32 {
            return Err(StrictViolation::BlockingCall {
                call,
                event,
                argument,
            });
        }

        let allocation = self.allocation.snapshot();
        let allocations = allocation
            .allocations
            .wrapping_sub(self.allocation_baseline.allocations);
        let reallocations = allocation
            .reallocations
            .wrapping_sub(self.allocation_baseline.reallocations);
        let frees = allocation
            .frees
            .wrapping_sub(self.allocation_baseline.frees);
        if allocation.failures != self.allocation_baseline.failures {
            return Err(StrictViolation::AllocationFailure);
        }
        let heap_calls = allocations.wrapping_add(reallocations).wrapping_add(frees);
        if self.policy.deny_allocations && heap_calls != 0 {
            return Err(StrictViolation::Allocation {
                calls: heap_calls,
                requested_bytes: allocation
                    .requested_bytes
                    .wrapping_sub(self.allocation_baseline.requested_bytes),
            });
        }

        let critical = self.critical.snapshot();
        if critical.active_interrupt_sections != 0 {
            return Err(StrictViolation::CriticalSectionStillActive(
                critical.active_interrupt_sections,
            ));
        }
        let unmatched = critical
            .unmatched_restores
            .wrapping_sub(self.critical_baseline.unmatched_restores);
        if unmatched != 0 {
            return Err(StrictViolation::UnmatchedInterruptRestore(unmatched));
        }
        if critical.max_interrupt_nesting > self.policy.max_interrupt_nesting {
            return Err(StrictViolation::InterruptNestingExceeded(
                critical.max_interrupt_nesting,
            ));
        }
        let stalls = critical
            .other_core_stalls
            .wrapping_sub(self.critical_baseline.other_core_stalls);
        if self.policy.deny_other_core_stalls && stalls != 0 {
            return Err(StrictViolation::OtherCoreStall(stalls));
        }
        let wrong_hart = critical
            .wrong_hart_entries
            .wrapping_sub(self.critical_baseline.wrong_hart_entries);
        if wrong_hart != 0 {
            return Err(StrictViolation::WrongWifiHart(wrong_hart));
        }
        Ok(())
    }
}

/// Fails closed as soon as a wrapped runtime poll reaches a forbidden path.
pub struct AuditedFuture<'a, F> {
    inner: F,
    audit: StrictAudit<'a>,
}

impl<'a, F> AuditedFuture<'a, F> {
    pub const fn new(inner: F, audit: StrictAudit<'a>) -> Self {
        Self { inner, audit }
    }

    pub fn inner(&self) -> &F {
        &self.inner
    }
}

impl<F: Future + Unpin> Future for AuditedFuture<'_, F> {
    type Output = Result<F::Output, StrictViolation>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let output = Pin::new(&mut self.inner).poll(cx);
        if let Err(violation) = self.audit.check() {
            return Poll::Ready(Err(violation));
        }
        output.map(Ok)
    }
}

#[cfg(test)]
mod tests {
    use super::{StrictAudit, StrictPolicy, StrictViolation};
    use crate::{
        allocation::AllocationProbe,
        critical::CriticalSectionProbe,
        diagnostics::{BlockingCall, BlockingCallProbe},
    };

    #[test]
    fn audit_reports_a_forbidden_call_with_context() {
        let blocking = BlockingCallProbe::new();
        let allocation = AllocationProbe::new();
        let critical = CriticalSectionProbe::new();
        let audit = StrictAudit::new(
            StrictPolicy::heap_free_single_owner(),
            &blocking,
            &allocation,
            &critical,
        );
        blocking.record(BlockingCall::TaskDelay, 8, 10);

        assert_eq!(
            audit.check(),
            Err(StrictViolation::BlockingCall {
                call: BlockingCall::TaskDelay as u32,
                event: 8,
                argument: 10,
            })
        );
    }
}
