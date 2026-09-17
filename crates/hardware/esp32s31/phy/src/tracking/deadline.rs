//! Wall-clock guard for an admitted PHY transaction.
//!
//! The composition supplies an absolute deadline in the tracking clock domain.
//! It must reserve time for protocol restoration separately. This guard does
//! not grant RF access, choose connection events or prove that a blocking
//! hardware poll fits the window. A poll which overruns is rejected on return;
//! it cannot be preempted by an executor timer.

use core::num::NonZeroU64;

/// Exclusive end of a maintenance execution window, in monotonic microseconds.
///
/// Construct this when the window starts, not when deferred work finally gets
/// polled. Reusing it never renews the window. Successful execution must finish
/// strictly before its end; reaching the deadline requires a poisoned owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TrackingDeadline {
    started_at_micros: u64,
    expires_at_micros: u64,
}

impl TrackingDeadline {
    /// Return `None` if the absolute end cannot be represented.
    pub const fn new(started_at_micros: u64, budget_micros: NonZeroU64) -> Option<Self> {
        match started_at_micros.checked_add(budget_micros.get()) {
            Some(expires_at_micros) => Some(Self {
                started_at_micros,
                expires_at_micros,
            }),
            None => None,
        }
    }

    pub const fn started_at_micros(self) -> u64 {
        self.started_at_micros
    }

    pub const fn expires_at_micros(self) -> u64 {
        self.expires_at_micros
    }

    /// Check an entry/restoration sample without renewing the absolute window.
    /// A sequence of execution samples additionally needs to reject reversal
    /// against the preceding sample, as the target executor's guard does.
    pub fn check(self, now_micros: Option<u64>) -> Result<(), TrackingDeadlineError> {
        Guard {
            deadline: self,
            previous_micros: self.started_at_micros,
        }
        .sample(now_micros)
        .map(|_| ())
    }
}

/// Timing failure; none of these errors authorizes resuming the PHY client.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrackingDeadlineError {
    ClockUnavailable,
    ClockWentBackwards {
        previous_micros: u64,
        observed_micros: u64,
    },
    Expired {
        deadline_micros: u64,
        observed_micros: u64,
    },
    /// The timer and monotonic clock do not agree on the requested interval.
    WakeBeforeDeadline {
        deadline_micros: u64,
        observed_micros: u64,
    },
}

struct Guard {
    deadline: TrackingDeadline,
    previous_micros: u64,
}

impl Guard {
    fn sample(&mut self, now: Option<u64>) -> Result<u64, TrackingDeadlineError> {
        let now = now.ok_or(TrackingDeadlineError::ClockUnavailable)?;
        if now < self.previous_micros {
            return Err(TrackingDeadlineError::ClockWentBackwards {
                previous_micros: self.previous_micros,
                observed_micros: now,
            });
        }
        self.previous_micros = now;
        self.deadline
            .expires_at_micros
            .checked_sub(now)
            .filter(|left| *left != 0)
            .ok_or(TrackingDeadlineError::Expired {
                deadline_micros: self.deadline.expires_at_micros,
                observed_micros: now,
            })
    }
}

/// Only borrow the in-flight hardware owner through `work`. On error, the
/// caller must consume that owner into its poisoned state after this function
/// has dropped the child future. In particular, never wrap a future returning
/// an independently runnable owner here.
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) async fn run<F: core::future::Future, A: core::future::Future<Output = ()>>(
    deadline: TrackingDeadline,
    mut now: impl FnMut() -> Option<u64>,
    alarm: impl FnOnce(u64) -> A,
    work: F,
) -> Result<F::Output, TrackingDeadlineError> {
    use core::{future::poll_fn, pin::pin, task::Poll};

    let mut guard = Guard {
        deadline,
        previous_micros: deadline.started_at_micros,
    };
    let remaining = guard.sample(now())?;
    let mut alarm = pin!(alarm(remaining));
    let mut work = pin!(work);
    poll_fn(|cx| {
        // An elapsed deadline wins ties with completion, before another MMIO
        // poll. Arm the independent wake before a child can suspend indefinitely.
        guard.sample(now())?;
        let elapsed = alarm.as_mut().poll(cx).is_ready();
        guard.sample(now())?;
        if elapsed {
            return Poll::Ready(Err(TrackingDeadlineError::WakeBeforeDeadline {
                deadline_micros: deadline.expires_at_micros,
                observed_micros: guard.previous_micros,
            }));
        }
        let result = work.as_mut().poll(cx);
        // A synchronous calibration can use the entire poll. Its successful
        // semantic result is insufficient to publish a runnable client late.
        guard.sample(now())?;
        result.map(Ok)
    })
    .await
}

#[cfg(test)]
mod tests;
