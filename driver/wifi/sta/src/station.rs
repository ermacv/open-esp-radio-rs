//! Allocation-free station attempt and reconnect orchestration.

use core::future::Future;

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
            match self.backend.run_attempt(owner, context).await {
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
                    owner = match self
                        .backend
                        .wait_backoff(
                            returned,
                            self.policy.disconnect_backoff_millis(),
                            StaBackoffReason::Disconnected,
                        )
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
                    owner = match self
                        .backend
                        .wait_backoff(
                            returned,
                            self.policy.retry_backoff_millis(generation_attempt),
                            StaBackoffReason::AttemptFailed {
                                stage: failure.stage,
                                attempt: generation_attempt,
                            },
                        )
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
mod tests {
    use core::{
        future::Future,
        pin::pin,
        task::{Context, Poll},
    };
    use std::{collections::VecDeque, vec::Vec};

    use super::*;

    #[derive(Clone, Copy)]
    enum Planned {
        Disconnected(StaNextCandidate),
        Stopped,
        Failed(StaLifecycleStage, StaFailureDisposition, u8),
        Faulted(u64),
    }

    struct Backend {
        outcomes: VecDeque<Planned>,
        contexts: Vec<StaAttemptContext>,
        backoffs: Vec<(u32, StaBackoffReason)>,
        stop_in_backoff: bool,
    }

    impl StaLifecycleBackend for Backend {
        type Owner = u32;
        type Error = u8;
        type Fault = u64;

        fn run_attempt(
            &mut self,
            owner: Self::Owner,
            context: StaAttemptContext,
        ) -> impl Future<Output = StaAttemptOutcome<Self::Owner, Self::Error, Self::Fault>> + '_
        {
            async move {
                self.contexts.push(context);
                let owner = owner + 1;
                match self.outcomes.pop_front().expect("planned attempt") {
                    Planned::Disconnected(next_candidate) => StaAttemptOutcome::Disconnected {
                        owner,
                        next_candidate,
                    },
                    Planned::Stopped => StaAttemptOutcome::Stopped { owner },
                    Planned::Failed(stage, disposition, error) => StaAttemptOutcome::Failed {
                        owner,
                        failure: StaAttemptFailure::new(stage, disposition, error),
                    },
                    Planned::Faulted(fault) => StaAttemptOutcome::Faulted { fault },
                }
            }
        }

        fn wait_backoff(
            &mut self,
            owner: Self::Owner,
            delay_millis: u32,
            reason: StaBackoffReason,
        ) -> impl Future<Output = StaBackoffOutcome<Self::Owner>> + '_ {
            async move {
                self.backoffs.push((delay_millis, reason));
                if self.stop_in_backoff {
                    StaBackoffOutcome::Stopped { owner }
                } else {
                    StaBackoffOutcome::Elapsed { owner }
                }
            }
        }
    }

    fn backend(outcomes: impl IntoIterator<Item = Planned>) -> Backend {
        Backend {
            outcomes: outcomes.into_iter().collect(),
            contexts: Vec::new(),
            backoffs: Vec::new(),
            stop_in_backoff: false,
        }
    }

    fn policy(attempt_limit: u16) -> StaReconnectPolicy {
        StaReconnectPolicy::new(attempt_limit, 10, 40, 5).unwrap()
    }

    fn block_on<F: Future>(future: F) -> F::Output {
        let mut future = pin!(future);
        let mut context = Context::from_waker(core::task::Waker::noop());
        loop {
            match future.as_mut().poll(&mut context) {
                Poll::Ready(output) => return output,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    #[test]
    fn retry_policy_is_bounded_and_saturates_exponential_delay() {
        let policy = StaReconnectPolicy::new(4, 10, 25, 5).unwrap();
        assert_eq!(policy.retry_backoff_millis(1), 10);
        assert_eq!(policy.retry_backoff_millis(2), 20);
        assert_eq!(policy.retry_backoff_millis(3), 25);
        assert_eq!(policy.retry_backoff_millis(u16::MAX), 25);
        assert_eq!(
            StaReconnectPolicy::new(0, 1, 1, 1),
            Err(StaReconnectPolicyError::ZeroAttemptLimit)
        );
    }

    #[test]
    fn retry_and_refresh_policy_preserve_one_owner_until_stop() {
        let backend = backend([
            Planned::Failed(
                StaLifecycleStage::Association,
                StaFailureDisposition::RetryCurrentCandidate,
                1,
            ),
            Planned::Failed(
                StaLifecycleStage::Security,
                StaFailureDisposition::RefreshCandidate,
                2,
            ),
            Planned::Stopped,
        ]);
        let mut service = StaLifecycleService::new(backend, policy(4));

        let exit = block_on(service.run(10));

        assert_eq!(
            exit,
            StaLifecycleExit::Stopped {
                owner: 13,
                progress: StaLifecycleProgress {
                    connected_epochs: 0,
                    attempts_started: 3,
                    final_generation_attempt: 3,
                    last_failure_stage: Some(StaLifecycleStage::Security),
                },
            }
        );
        assert_eq!(
            service.backend().contexts,
            [
                StaAttemptContext {
                    generation: 0,
                    attempt: 1,
                    refresh_candidate: true,
                },
                StaAttemptContext {
                    generation: 0,
                    attempt: 2,
                    refresh_candidate: false,
                },
                StaAttemptContext {
                    generation: 0,
                    attempt: 3,
                    refresh_candidate: true,
                },
            ]
        );
        assert_eq!(service.backend().backoffs[0].0, 10);
        assert_eq!(service.backend().backoffs[1].0, 20);
    }

    #[test]
    fn disconnect_starts_a_new_generation_after_nonzero_backoff() {
        let backend = backend([
            Planned::Disconnected(StaNextCandidate::Refresh),
            Planned::Stopped,
        ]);
        let mut service = StaLifecycleService::new(backend, policy(3));

        let exit = block_on(service.run(0));

        assert!(matches!(
            exit,
            StaLifecycleExit::Stopped {
                owner: 2,
                progress: StaLifecycleProgress {
                    connected_epochs: 1,
                    attempts_started: 2,
                    final_generation_attempt: 1,
                    last_failure_stage: None,
                },
            }
        ));
        assert_eq!(
            service.backend().backoffs,
            [(5, StaBackoffReason::Disconnected)]
        );
        assert_eq!(service.backend().contexts[1].generation, 1);
        assert!(service.backend().contexts[1].refresh_candidate);
    }

    #[test]
    fn same_peer_reconnect_does_not_claim_an_unperformed_candidate_refresh() {
        let backend = backend([
            Planned::Disconnected(StaNextCandidate::Reuse),
            Planned::Stopped,
        ]);
        let mut service = StaLifecycleService::new(backend, policy(3));

        let exit = block_on(service.run(7));

        assert!(matches!(exit, StaLifecycleExit::Stopped { owner: 9, .. }));
        assert_eq!(
            service.backend().contexts,
            [
                StaAttemptContext {
                    generation: 0,
                    attempt: 1,
                    refresh_candidate: true,
                },
                StaAttemptContext {
                    generation: 1,
                    attempt: 1,
                    refresh_candidate: false,
                },
            ]
        );
    }

    #[test]
    fn exhaustion_returns_the_last_failure_and_exact_owner_without_extra_wait() {
        let backend = backend([
            Planned::Failed(
                StaLifecycleStage::Authentication,
                StaFailureDisposition::RetryCurrentCandidate,
                7,
            ),
            Planned::Failed(
                StaLifecycleStage::Authentication,
                StaFailureDisposition::RetryCurrentCandidate,
                8,
            ),
        ]);
        let mut service = StaLifecycleService::new(backend, policy(2));

        let exit = block_on(service.run(4));

        assert!(matches!(
            exit,
            StaLifecycleExit::Exhausted {
                owner: 6,
                failure: StaAttemptFailure { error: 8, .. },
                ..
            }
        ));
        assert_eq!(service.backend().backoffs.len(), 1);
    }

    #[test]
    fn terminal_failure_and_backoff_stop_both_return_hardware_ownership() {
        let terminal = backend([Planned::Failed(
            StaLifecycleStage::Hardware,
            StaFailureDisposition::Terminal,
            9,
        )]);
        let mut service = StaLifecycleService::new(terminal, policy(3));
        assert!(matches!(
            block_on(service.run(20)),
            StaLifecycleExit::Terminal {
                owner: 21,
                failure: StaAttemptFailure { error: 9, .. },
                ..
            }
        ));

        let mut stopped = backend([Planned::Failed(
            StaLifecycleStage::Security,
            StaFailureDisposition::RefreshCandidate,
            3,
        )]);
        stopped.stop_in_backoff = true;
        let mut service = StaLifecycleService::new(stopped, policy(3));
        assert!(matches!(
            block_on(service.run(30)),
            StaLifecycleExit::Stopped { owner: 31, .. }
        ));
    }

    #[test]
    fn faulted_frontier_is_returned_without_retry_or_backoff() {
        let backend = backend([Planned::Faulted(0xfeed_beef)]);
        let mut service = StaLifecycleService::new(backend, policy(3));

        assert_eq!(
            block_on(service.run(40)),
            StaLifecycleExit::Faulted {
                fault: 0xfeed_beef,
                progress: StaLifecycleProgress {
                    connected_epochs: 0,
                    attempts_started: 1,
                    final_generation_attempt: 1,
                    last_failure_stage: None,
                },
            }
        );
        assert!(service.backend().backoffs.is_empty());
        assert_eq!(service.backend().contexts.len(), 1);
    }
}
