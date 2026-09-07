//! Absolute Embassy deadline for cooperative Bluetooth Controller-time rechecks.
//!
//! The schedule owns one deadline and a nonzero period. Creating or cancelling
//! a wait only borrows that schedule; the deadline advances from its previous
//! absolute value only after the matching `Timer::at` completes. Checked
//! advancement enters a typed exhausted state instead of wrapping the Embassy
//! monotonic timeline.

#![forbid(unsafe_code)]

use core::num::NonZeroU64;

#[cfg(target_arch = "riscv32")]
use core::{
    future::Future,
    pin::Pin,
    task::{Context, Poll, ready},
};

#[cfg(target_arch = "riscv32")]
use crate::session::dtm::DtmControllerTimeRecheck;
use embassy_time::Duration;
#[cfg(target_arch = "riscv32")]
use embassy_time::{Instant, Timer};

use crate::session::dtm::DtmControllerTimeRecheckStatus;

/// Nonzero spacing between successive absolute Controller-time rechecks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DtmRecheckPeriod {
    ticks: NonZeroU64,
}

/// Invalid absolute-recheck period.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmRecheckPeriodError {
    /// A zero period cannot advance an absolute schedule.
    Zero,
}

impl DtmRecheckPeriod {
    /// Construct a period from executor clock ticks.
    pub const fn from_ticks(ticks: u64) -> Result<Self, DtmRecheckPeriodError> {
        let Some(ticks) = NonZeroU64::new(ticks) else {
            return Err(DtmRecheckPeriodError::Zero);
        };
        Ok(Self { ticks })
    }

    /// Construct a period from one Embassy duration.
    pub const fn from_duration(duration: Duration) -> Result<Self, DtmRecheckPeriodError> {
        Self::from_ticks(duration.as_ticks())
    }

    /// Period in executor clock ticks.
    pub const fn as_ticks(self) -> u64 {
        self.ticks.get()
    }

    /// Period as an Embassy duration.
    pub const fn as_duration(self) -> Duration {
        Duration::from_ticks(self.as_ticks())
    }
}

/// One absolute Embassy deadline represented without an implicit unit change.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DtmRecheckDeadline {
    ticks: u64,
}

impl DtmRecheckDeadline {
    /// Construct an absolute deadline from executor clock ticks since boot.
    pub const fn from_ticks(ticks: u64) -> Self {
        Self { ticks }
    }

    /// Absolute executor tick count.
    pub const fn as_ticks(self) -> u64 {
        self.ticks
    }

    #[cfg(target_arch = "riscv32")]
    fn from_instant(instant: Instant) -> Self {
        Self::from_ticks(instant.as_ticks())
    }

    #[cfg(target_arch = "riscv32")]
    fn into_instant(self) -> Instant {
        Instant::from_ticks(self.ticks)
    }
}

/// Current absolute schedule state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmRecheckScheduleState {
    /// One representable absolute deadline remains armed.
    Scheduled(DtmRecheckDeadline),
    /// Adding the nonzero period would wrap the monotonic `u64` timeline.
    TimelineExhausted,
}

struct AbsoluteRecheckSchedule {
    state: DtmRecheckScheduleState,
    period: DtmRecheckPeriod,
}

impl AbsoluteRecheckSchedule {
    const fn new(deadline: DtmRecheckDeadline, period: DtmRecheckPeriod) -> Self {
        Self {
            state: DtmRecheckScheduleState::Scheduled(deadline),
            period,
        }
    }

    const fn state(&self) -> DtmRecheckScheduleState {
        self.state
    }

    const fn status(&self) -> DtmControllerTimeRecheckStatus {
        match self.state {
            DtmRecheckScheduleState::Scheduled(_) => DtmControllerTimeRecheckStatus::Scheduled,
            DtmRecheckScheduleState::TimelineExhausted => {
                DtmControllerTimeRecheckStatus::TimelineExhausted
            }
        }
    }

    fn begin_wait(&mut self) -> Option<AbsoluteRecheckWaitLease<'_>> {
        let DtmRecheckScheduleState::Scheduled(deadline) = self.state else {
            return None;
        };
        Some(AbsoluteRecheckWaitLease {
            schedule: self,
            deadline,
        })
    }

    fn advance_after_completed_wait(&mut self) {
        let DtmRecheckScheduleState::Scheduled(deadline) = self.state else {
            return;
        };
        self.state = match deadline.as_ticks().checked_add(self.period.as_ticks()) {
            Some(next) => DtmRecheckScheduleState::Scheduled(DtmRecheckDeadline::from_ticks(next)),
            None => DtmRecheckScheduleState::TimelineExhausted,
        };
    }
}

struct AbsoluteRecheckWaitLease<'schedule> {
    schedule: &'schedule mut AbsoluteRecheckSchedule,
    deadline: DtmRecheckDeadline,
}

impl AbsoluteRecheckWaitLease<'_> {
    const fn deadline(&self) -> DtmRecheckDeadline {
        self.deadline
    }

    fn complete(self) {
        self.schedule.advance_after_completed_wait();
    }
}

/// Failure to anchor the first absolute recheck after the current instant.
#[cfg(target_arch = "riscv32")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmRecheckStartError {
    /// Current time plus one period is not representable without wrapping.
    TimelineExhausted,
}

/// Concrete Embassy provider for periodic absolute Controller-time rechecks.
#[cfg(target_arch = "riscv32")]
#[must_use = "retain the provider so cancellation cannot discard its absolute deadline"]
pub struct DtmAbsoluteRecheck {
    schedule: AbsoluteRecheckSchedule,
}

#[cfg(target_arch = "riscv32")]
impl DtmAbsoluteRecheck {
    /// Anchor a schedule at an explicitly supplied first absolute deadline.
    pub fn starting_at(first_deadline: Instant, period: DtmRecheckPeriod) -> Self {
        Self {
            schedule: AbsoluteRecheckSchedule::new(
                DtmRecheckDeadline::from_instant(first_deadline),
                period,
            ),
        }
    }

    /// Anchor the first deadline exactly one period after the current instant.
    pub fn after_period(period: DtmRecheckPeriod) -> Result<Self, DtmRecheckStartError> {
        let Some(first_deadline) = Instant::now().checked_add(period.as_duration()) else {
            return Err(DtmRecheckStartError::TimelineExhausted);
        };
        Ok(Self::starting_at(first_deadline, period))
    }

    /// Current absolute deadline or typed timeline exhaustion.
    pub const fn state(&self) -> DtmRecheckScheduleState {
        self.schedule.state()
    }
}

/// One cancellation-safe borrowed wait over a provider-owned schedule.
#[cfg(target_arch = "riscv32")]
#[must_use = "await the deadline or drop the future without advancing it"]
pub struct DtmAbsoluteRecheckWait<'provider> {
    lease: Option<AbsoluteRecheckWaitLease<'provider>>,
    timer: Option<Timer>,
}

#[cfg(target_arch = "riscv32")]
impl Future for DtmAbsoluteRecheckWait<'_> {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let Some(timer) = self.timer.as_mut() else {
            return Poll::Pending;
        };
        ready!(Pin::new(timer).poll(context));
        self.timer = None;
        self.lease
            .take()
            .expect("a representable Timer retains its schedule lease")
            .complete();
        Poll::Ready(())
    }
}

#[cfg(target_arch = "riscv32")]
impl DtmControllerTimeRecheck for DtmAbsoluteRecheck {
    type Recheck<'borrow>
        = DtmAbsoluteRecheckWait<'borrow>
    where
        Self: 'borrow;

    fn status(&self) -> DtmControllerTimeRecheckStatus {
        self.schedule.status()
    }

    fn wait_until_absolute_recheck(&mut self) -> Self::Recheck<'_> {
        let lease = self.schedule.begin_wait();
        let timer = lease
            .as_ref()
            .map(|lease| Timer::at(lease.deadline().into_instant()));
        DtmAbsoluteRecheckWait { lease, timer }
    }
}

#[cfg(test)]
mod tests;
