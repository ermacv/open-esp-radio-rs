//! Allocation-free station attempt and reconnect orchestration.

use core::{
    future::{Future, poll_fn},
    pin::{Pin, pin},
    task::{Context, Poll},
};

/// Poll a child state machine through a real call boundary.
///
/// The future remains stored in its parent future. Only the CPU stack used by
/// its `poll` implementation is isolated, preventing fat LTO from merging a
/// complete MLME attempt into the outer retry loop.
async fn poll_with_stack_boundary<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let mut output = None;
    poll_fn(|context| poll_pinned_future(future.as_mut(), &mut output, context)).await;
    output.expect("completed stack boundary stores its output")
}

#[inline(never)]
fn poll_pinned_future<F: Future>(
    future: Pin<&mut F>,
    output: &mut Option<F::Output>,
    context: &mut Context<'_>,
) -> Poll<()> {
    type PollFn<F> = for<'future, 'context, 'wake> fn(
        Pin<&'future mut F>,
        &'context mut Context<'wake>,
    ) -> Poll<<F as Future>::Output>;
    let poll: PollFn<F> = F::poll;
    match core::hint::black_box(poll)(future, context) {
        Poll::Pending => Poll::Pending,
        Poll::Ready(value) => {
            *output = Some(value);
            Poll::Ready(())
        }
    }
}

/// Protocol/hardware phase which rejected one station attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaLifecycleStage {
    CandidateSelection,
    Authentication,
    Association,
    Security,
    Connected,
    Hardware,
}

/// Policy classification attached to one failed finite attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaFailureDisposition {
    /// Retry without requiring a new scan/candidate decision.
    RetryCurrentCandidate,
    /// Refresh candidate selection before the next attempt.
    RefreshCandidate,
    /// The hardware owner is still returned, but automatic retry is unsafe.
    Terminal,
}

/// Candidate policy for the attempt after a connected epoch ends.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaNextCandidate {
    Reuse,
    Refresh,
}

/// One owned attempt failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaAttemptFailure<E> {
    pub stage: StaLifecycleStage,
    pub disposition: StaFailureDisposition,
    pub error: E,
}

impl<E> StaAttemptFailure<E> {
    pub const fn new(
        stage: StaLifecycleStage,
        disposition: StaFailureDisposition,
        error: E,
    ) -> Self {
        Self {
            stage,
            disposition,
            error,
        }
    }
}

/// Facts supplied to one backend-owned scan/join/WPA2/connected attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaAttemptContext {
    /// Number of connected epochs which have already returned.
    pub generation: u32,
    /// One-based attempt number within the current generation.
    pub attempt: u16,
    /// Whether this attempt must refresh scan/candidate state first.
    pub refresh_candidate: bool,
}

/// Result of one complete backend attempt.
///
/// Every reusable variant returns the exact owner consumed by the attempt. A
/// backend cannot report progress while retaining DMA, key or protocol
/// ownership in a hidden task-local context. `Faulted` is deliberately a
/// different owner type: it retains an exact non-reusable hardware frontier
/// without pretending that it can re-enter the normal station lifecycle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaAttemptOutcome<O, E, F = core::convert::Infallible> {
    /// One finite protocol phase completed and returned the exact owner at the
    /// next phase frontier. The lifecycle immediately dispatches that phase
    /// without consuming retry budget, incrementing the attempt or waiting.
    Advanced { owner: O },
    /// A connected epoch ended and automatic reconnection may continue.
    Disconnected {
        owner: O,
        next_candidate: StaNextCandidate,
    },
    /// The outer caller requested a normal finite stop.
    Stopped { owner: O },
    /// A finite phase failed while preserving a retryable or terminal owner.
    Failed {
        owner: O,
        failure: StaAttemptFailure<E>,
    },
    /// The attempt retained its hardware at a frontier which cannot be
    /// represented by `O`. No reconnect or backoff transition is legal.
    Faulted { fault: F },
}

/// Why the service is waiting before another attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaBackoffReason {
    Disconnected,
    AttemptFailed {
        stage: StaLifecycleStage,
        attempt: u16,
    },
}

/// Backoff completion also returns the exact resource owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaBackoffOutcome<O> {
    Elapsed { owner: O },
    Stopped { owner: O },
}

/// Finite platform composition consumed by [`StaLifecycleService`].
///
/// `run_attempt` owns the concrete scan/Auth/Assoc/WPA2/connected composition;
/// this trait does not replace those typed phase transitions with callbacks.
/// `wait_backoff` consumes and returns the same owner so cancellation during a
/// timer edge cannot strand hardware in the executor future.
pub trait StaLifecycleBackend {
    type Owner;
    type Error;
    type Fault;

    fn run_attempt(
        &mut self,
        owner: Self::Owner,
        context: StaAttemptContext,
    ) -> impl Future<Output = StaAttemptOutcome<Self::Owner, Self::Error, Self::Fault>> + '_;

    fn wait_backoff(
        &mut self,
        owner: Self::Owner,
        delay_millis: u32,
        reason: StaBackoffReason,
    ) -> impl Future<Output = StaBackoffOutcome<Self::Owner>> + '_;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaReconnectPolicyError {
    ZeroAttemptLimit,
    ZeroRetryBackoff,
    RetryBackoffRange,
    ZeroDisconnectBackoff,
}

/// Bounded retry/backoff policy for one station service.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaReconnectPolicy {
    attempt_limit: u16,
    initial_retry_backoff_millis: u32,
    maximum_retry_backoff_millis: u32,
    disconnect_backoff_millis: u32,
}

impl StaReconnectPolicy {
    pub const fn new(
        attempt_limit: u16,
        initial_retry_backoff_millis: u32,
        maximum_retry_backoff_millis: u32,
        disconnect_backoff_millis: u32,
    ) -> Result<Self, StaReconnectPolicyError> {
        if attempt_limit == 0 {
            return Err(StaReconnectPolicyError::ZeroAttemptLimit);
        }
        if initial_retry_backoff_millis == 0 {
            return Err(StaReconnectPolicyError::ZeroRetryBackoff);
        }
        if maximum_retry_backoff_millis < initial_retry_backoff_millis {
            return Err(StaReconnectPolicyError::RetryBackoffRange);
        }
        if disconnect_backoff_millis == 0 {
            return Err(StaReconnectPolicyError::ZeroDisconnectBackoff);
        }
        Ok(Self {
            attempt_limit,
            initial_retry_backoff_millis,
            maximum_retry_backoff_millis,
            disconnect_backoff_millis,
        })
    }

    pub const fn attempt_limit(self) -> u16 {
        self.attempt_limit
    }

    pub const fn disconnect_backoff_millis(self) -> u32 {
        self.disconnect_backoff_millis
    }

    /// Exponential retry delay capped before integer overflow.
    pub const fn retry_backoff_millis(self, failed_attempt: u16) -> u32 {
        let mut delay = self.initial_retry_backoff_millis;
        let mut remaining = failed_attempt.saturating_sub(1);
        while remaining != 0 && delay < self.maximum_retry_backoff_millis {
            let doubled = delay.saturating_mul(2);
            delay = if doubled < self.maximum_retry_backoff_millis {
                doubled
            } else {
                self.maximum_retry_backoff_millis
            };
            remaining -= 1;
        }
        delay
    }
}

/// Observable bounded progress returned at every terminal service edge.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StaLifecycleProgress {
    pub connected_epochs: u32,
    pub attempts_started: u32,
    pub final_generation_attempt: u16,
    pub last_failure_stage: Option<StaLifecycleStage>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaLifecycleExit<O, E, F = core::convert::Infallible> {
    Stopped {
        owner: O,
        progress: StaLifecycleProgress,
    },
    Exhausted {
        owner: O,
        progress: StaLifecycleProgress,
        failure: StaAttemptFailure<E>,
    },
    Terminal {
        owner: O,
        progress: StaLifecycleProgress,
        failure: StaAttemptFailure<E>,
    },
    /// Exact non-reusable owner returned by a faulted backend transition.
    Faulted {
        fault: F,
        progress: StaLifecycleProgress,
    },
}

/// Outer allocation-free station lifecycle owner.
pub struct StaLifecycleService<B> {
    backend: B,
    policy: StaReconnectPolicy,
}

impl<B> StaLifecycleService<B>
where
    B: StaLifecycleBackend,
{
    pub const fn new(backend: B, policy: StaReconnectPolicy) -> Self {
        Self { backend, policy }
    }

    pub const fn backend(&self) -> &B {
        &self.backend
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    pub fn into_backend(self) -> B {
        self.backend
    }

    /// Run until caller stop, retry exhaustion or a terminal failure.
    pub async fn run(&mut self, owner: B::Owner) -> StaLifecycleExit<B::Owner, B::Error, B::Fault> {
        let mut owner = owner;
        let mut progress = StaLifecycleProgress::default();
        let mut generation_attempt = 1_u16;
        // A cold service never accepts a caller-proven candidate. Candidate
        // reuse is legal only after this lifecycle itself returns a connected
        // epoch with an explicit `StaNextCandidate::Reuse` disposition.
        let mut refresh_candidate = true;
        loop {
            progress.attempts_started = progress.attempts_started.saturating_add(1);
            progress.final_generation_attempt = generation_attempt;
            let context = StaAttemptContext {
                generation: progress.connected_epochs,
                attempt: generation_attempt,
                refresh_candidate,
            };
            let outcome = loop {
                match poll_with_stack_boundary(self.backend.run_attempt(owner, context)).await {
                    StaAttemptOutcome::Advanced { owner: advanced } => owner = advanced,
                    outcome => break outcome,
                }
            };
            match outcome {
                StaAttemptOutcome::Advanced { .. } => {
                    unreachable!("advanced phases are consumed by the inner attempt loop")
                }
                StaAttemptOutcome::Stopped { owner } => {
                    return StaLifecycleExit::Stopped { owner, progress };
                }
                StaAttemptOutcome::Disconnected {
                    owner: returned,
                    next_candidate,
                } => {
                    progress.connected_epochs = progress.connected_epochs.saturating_add(1);
                    progress.last_failure_stage = None;
                    generation_attempt = 1;
                    refresh_candidate = next_candidate == StaNextCandidate::Refresh;
                    owner = match poll_with_stack_boundary(self.backend.wait_backoff(
                        returned,
                        self.policy.disconnect_backoff_millis(),
                        StaBackoffReason::Disconnected,
                    ))
                    .await
                    {
                        StaBackoffOutcome::Elapsed { owner } => owner,
                        StaBackoffOutcome::Stopped { owner } => {
                            return StaLifecycleExit::Stopped { owner, progress };
                        }
                    };
                }
                StaAttemptOutcome::Failed {
                    owner: returned,
                    failure,
                } => {
                    progress.last_failure_stage = Some(failure.stage);
                    if failure.disposition == StaFailureDisposition::Terminal {
                        return StaLifecycleExit::Terminal {
                            owner: returned,
                            progress,
                            failure,
                        };
                    }
                    if generation_attempt >= self.policy.attempt_limit() {
                        return StaLifecycleExit::Exhausted {
                            owner: returned,
                            progress,
                            failure,
                        };
                    }
                    refresh_candidate =
                        failure.disposition == StaFailureDisposition::RefreshCandidate;
                    owner = match poll_with_stack_boundary(self.backend.wait_backoff(
                        returned,
                        self.policy.retry_backoff_millis(generation_attempt),
                        StaBackoffReason::AttemptFailed {
                            stage: failure.stage,
                            attempt: generation_attempt,
                        },
                    ))
                    .await
                    {
                        StaBackoffOutcome::Elapsed { owner } => owner,
                        StaBackoffOutcome::Stopped { owner } => {
                            return StaLifecycleExit::Stopped { owner, progress };
                        }
                    };
                    generation_attempt += 1;
                }
                StaAttemptOutcome::Faulted { fault } => {
                    return StaLifecycleExit::Faulted { fault, progress };
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
