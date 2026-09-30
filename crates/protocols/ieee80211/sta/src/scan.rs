//! Allocation-free candidate-scan values and port.
//!
//! The `StaCandidateScanService` of `oer-ieee80211-sta-service` drives the
//! [`StaCandidateScanBackend`] port through one finite channel plan.

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

/// Finite chip/runtime adapter used by the `StaCandidateScanService` of
/// `oer-ieee80211-sta-service`.
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
