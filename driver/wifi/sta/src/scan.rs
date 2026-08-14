//! Allocation-free candidate-scan orchestration.

use core::future::Future;

/// One channel visit within a finite scan plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaScanChannelContext<C> {
    pub channel: C,
    pub index: u16,
    pub total_channels: u16,
}

impl<C> StaScanChannelContext<C> {
    pub fn is_last(&self) -> bool {
        self.index.checked_add(1) == Some(self.total_channels)
    }
}

/// Result of scan preparation or one channel visit.
///
/// Every edge returns the exact owner consumed by the backend. A failed dwell
/// therefore cannot strand PAC, RX-DMA or observation-table ownership inside
/// an executor future.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaScanStepOutcome<O, E> {
    Completed { owner: O },
    Stopped { owner: O },
    Failed { owner: O, error: E },
}

/// Final candidate decision after all planned channel visits complete.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaScanSelectionOutcome<O, C, E> {
    Selected { owner: O, candidate: C },
    NoCandidate { owner: O },
    Stopped { owner: O },
    Failed { owner: O, error: E },
}

/// Finite chip/runtime adapter used by [`StaCandidateScanService`].
///
/// `begin_scan` owns observation reset and any hardware preparation which must
/// happen exactly once per scan attempt. `scan_channel` owns one channel
/// switch, RX/probe publication and bounded dwell. Candidate selection remains
/// a separate edge so an adapter cannot report a retained `ScanRecord` as a
/// refreshed candidate without actually completing the channel plan.
pub trait StaCandidateScanBackend {
    type Owner;
    type Channel: Copy;
    type Candidate;
    type Error;

    fn begin_scan(
        &mut self,
        owner: Self::Owner,
    ) -> impl Future<Output = StaScanStepOutcome<Self::Owner, Self::Error>> + '_;

    fn scan_channel(
        &mut self,
        owner: Self::Owner,
        context: StaScanChannelContext<Self::Channel>,
    ) -> impl Future<Output = StaScanStepOutcome<Self::Owner, Self::Error>> + '_;

    fn select_candidate(
        &mut self,
        owner: Self::Owner,
    ) -> StaScanSelectionOutcome<Self::Owner, Self::Candidate, Self::Error>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaScanPlanError {
    Empty,
    TooManyChannels,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StaScanProgress {
    pub channels_planned: u16,
    pub channels_started: u16,
    pub channels_completed: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaCandidateScanExit<O, C, E> {
    Selected {
        owner: O,
        candidate: C,
        progress: StaScanProgress,
    },
    NoCandidate {
        owner: O,
        progress: StaScanProgress,
    },
    Stopped {
        owner: O,
        progress: StaScanProgress,
    },
    Failed {
        owner: O,
        error: E,
        progress: StaScanProgress,
    },
    InvalidPlan {
        owner: O,
        error: StaScanPlanError,
        progress: StaScanProgress,
    },
}

/// Runs exactly one finite candidate scan.
///
/// Retry/backoff belongs to `StaLifecycleService`; this service deliberately
/// performs no implicit repetition. Cold and running hardware adapters may
/// therefore use different owner variants while sharing the same closed scan
/// order and progress contract.
pub struct StaCandidateScanService<B> {
    backend: B,
}

impl<B> StaCandidateScanService<B>
where
    B: StaCandidateScanBackend,
{
    pub const fn new(backend: B) -> Self {
        Self { backend }
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

    pub async fn run(
        &mut self,
        mut owner: B::Owner,
        channels: &[B::Channel],
    ) -> StaCandidateScanExit<B::Owner, B::Candidate, B::Error> {
        let channels_planned = match u16::try_from(channels.len()) {
            Ok(0) => {
                return StaCandidateScanExit::InvalidPlan {
                    owner,
                    error: StaScanPlanError::Empty,
                    progress: StaScanProgress::default(),
                };
            }
            Ok(count) => count,
            Err(_) => {
                return StaCandidateScanExit::InvalidPlan {
                    owner,
                    error: StaScanPlanError::TooManyChannels,
                    progress: StaScanProgress::default(),
                };
            }
        };
        let mut progress = StaScanProgress {
            channels_planned,
            ..StaScanProgress::default()
        };

        owner = match self.backend.begin_scan(owner).await {
            StaScanStepOutcome::Completed { owner } => owner,
            StaScanStepOutcome::Stopped { owner } => {
                return StaCandidateScanExit::Stopped { owner, progress };
            }
            StaScanStepOutcome::Failed { owner, error } => {
                return StaCandidateScanExit::Failed {
                    owner,
                    error,
                    progress,
                };
            }
        };

        for (index, channel) in channels.iter().copied().enumerate() {
            let index = u16::try_from(index).expect("validated channel plan fits in u16");
            progress.channels_started = progress.channels_started.saturating_add(1);
            owner = match self
                .backend
                .scan_channel(
                    owner,
                    StaScanChannelContext {
                        channel,
                        index,
                        total_channels: channels_planned,
                    },
                )
                .await
            {
                StaScanStepOutcome::Completed { owner } => {
                    progress.channels_completed = progress.channels_completed.saturating_add(1);
                    owner
                }
                StaScanStepOutcome::Stopped { owner } => {
                    return StaCandidateScanExit::Stopped { owner, progress };
                }
                StaScanStepOutcome::Failed { owner, error } => {
                    return StaCandidateScanExit::Failed {
                        owner,
                        error,
                        progress,
                    };
                }
            };
        }

        match self.backend.select_candidate(owner) {
            StaScanSelectionOutcome::Selected { owner, candidate } => {
                StaCandidateScanExit::Selected {
                    owner,
                    candidate,
                    progress,
                }
            }
            StaScanSelectionOutcome::NoCandidate { owner } => {
                StaCandidateScanExit::NoCandidate { owner, progress }
            }
            StaScanSelectionOutcome::Stopped { owner } => {
                StaCandidateScanExit::Stopped { owner, progress }
            }
            StaScanSelectionOutcome::Failed { owner, error } => StaCandidateScanExit::Failed {
                owner,
                error,
                progress,
            },
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
    use std::vec::Vec;

    use super::*;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Error {
        Begin,
        Channel(u8),
        Selection,
    }

    struct Backend {
        events: Vec<&'static str>,
        contexts: Vec<StaScanChannelContext<u8>>,
        begin: Result<(), Error>,
        fail_channel: Option<u8>,
        selection: Result<Option<u8>, Error>,
        stop_at_begin: bool,
    }

    impl StaCandidateScanBackend for Backend {
        type Owner = u32;
        type Channel = u8;
        type Candidate = u8;
        type Error = Error;

        async fn begin_scan(
            &mut self,
            owner: Self::Owner,
        ) -> StaScanStepOutcome<Self::Owner, Self::Error> {
            self.events.push("begin");
            let owner = owner + 1;
            if self.stop_at_begin {
                StaScanStepOutcome::Stopped { owner }
            } else {
                match self.begin {
                    Ok(()) => StaScanStepOutcome::Completed { owner },
                    Err(error) => StaScanStepOutcome::Failed { owner, error },
                }
            }
        }

        async fn scan_channel(
            &mut self,
            owner: Self::Owner,
            context: StaScanChannelContext<Self::Channel>,
        ) -> StaScanStepOutcome<Self::Owner, Self::Error> {
            self.events.push("channel");
            self.contexts.push(context);
            let owner = owner + 1;
            if self.fail_channel == Some(context.channel) {
                StaScanStepOutcome::Failed {
                    owner,
                    error: Error::Channel(context.channel),
                }
            } else {
                StaScanStepOutcome::Completed { owner }
            }
        }

        fn select_candidate(
            &mut self,
            owner: Self::Owner,
        ) -> StaScanSelectionOutcome<Self::Owner, Self::Candidate, Self::Error> {
            self.events.push("select");
            let owner = owner + 1;
            match self.selection {
                Ok(Some(candidate)) => StaScanSelectionOutcome::Selected { owner, candidate },
                Ok(None) => StaScanSelectionOutcome::NoCandidate { owner },
                Err(error) => StaScanSelectionOutcome::Failed { owner, error },
            }
        }
    }

    fn backend(selection: Result<Option<u8>, Error>) -> Backend {
        Backend {
            events: Vec::new(),
            contexts: Vec::new(),
            begin: Ok(()),
            fail_channel: None,
            selection,
            stop_at_begin: false,
        }
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
    fn selected_candidate_follows_the_complete_ordered_channel_plan() {
        let backend = backend(Ok(Some(42)));
        let mut service = StaCandidateScanService::new(backend);

        let exit = block_on(service.run(10, &[1, 6, 11]));

        assert_eq!(
            exit,
            StaCandidateScanExit::Selected {
                owner: 15,
                candidate: 42,
                progress: StaScanProgress {
                    channels_planned: 3,
                    channels_started: 3,
                    channels_completed: 3,
                },
            }
        );
        assert_eq!(
            service.backend().contexts,
            [
                StaScanChannelContext {
                    channel: 1,
                    index: 0,
                    total_channels: 3,
                },
                StaScanChannelContext {
                    channel: 6,
                    index: 1,
                    total_channels: 3,
                },
                StaScanChannelContext {
                    channel: 11,
                    index: 2,
                    total_channels: 3,
                },
            ]
        );
        assert!(service.backend().contexts[2].is_last());
    }

    #[test]
    fn empty_plan_fails_before_the_backend_and_preserves_the_owner() {
        let backend = backend(Ok(Some(1)));
        let mut service = StaCandidateScanService::new(backend);

        assert_eq!(
            block_on(service.run(7, &[])),
            StaCandidateScanExit::InvalidPlan {
                owner: 7,
                error: StaScanPlanError::Empty,
                progress: StaScanProgress::default(),
            }
        );
        assert!(service.backend().events.is_empty());
    }

    #[test]
    fn channel_failure_returns_the_exact_owner_and_stops_the_plan() {
        let mut backend = backend(Ok(Some(1)));
        backend.fail_channel = Some(6);
        let mut service = StaCandidateScanService::new(backend);

        assert_eq!(
            block_on(service.run(20, &[1, 6, 11])),
            StaCandidateScanExit::Failed {
                owner: 23,
                error: Error::Channel(6),
                progress: StaScanProgress {
                    channels_planned: 3,
                    channels_started: 2,
                    channels_completed: 1,
                },
            }
        );
        assert_eq!(service.backend().contexts.len(), 2);
        assert_eq!(service.backend().events, ["begin", "channel", "channel"]);
    }

    #[test]
    fn no_candidate_is_distinct_from_hardware_or_selection_failure() {
        let mut absent = StaCandidateScanService::new(backend(Ok(None)));
        assert!(matches!(
            block_on(absent.run(0, &[1])),
            StaCandidateScanExit::NoCandidate {
                owner: 3,
                progress: StaScanProgress {
                    channels_completed: 1,
                    ..
                },
            }
        ));

        let mut failed = StaCandidateScanService::new(backend(Err(Error::Selection)));
        assert!(matches!(
            block_on(failed.run(0, &[1])),
            StaCandidateScanExit::Failed {
                owner: 3,
                error: Error::Selection,
                ..
            }
        ));
    }

    #[test]
    fn stop_during_preparation_returns_before_any_channel_is_started() {
        let mut backend = backend(Ok(Some(1)));
        backend.stop_at_begin = true;
        let mut service = StaCandidateScanService::new(backend);

        assert!(matches!(
            block_on(service.run(90, &[1, 6])),
            StaCandidateScanExit::Stopped {
                owner: 91,
                progress: StaScanProgress {
                    channels_planned: 2,
                    channels_started: 0,
                    channels_completed: 0,
                },
            }
        ));
        assert_eq!(service.backend().events, ["begin"]);
    }

    #[test]
    fn begin_failure_is_not_reclassified_as_no_candidate() {
        let mut backend = backend(Ok(Some(1)));
        backend.begin = Err(Error::Begin);
        let mut service = StaCandidateScanService::new(backend);

        assert!(matches!(
            block_on(service.run(4, &[1])),
            StaCandidateScanExit::Failed {
                owner: 5,
                error: Error::Begin,
                ..
            }
        ));
    }
}
