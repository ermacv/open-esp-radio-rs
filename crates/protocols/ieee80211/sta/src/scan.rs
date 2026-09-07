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
mod tests;
