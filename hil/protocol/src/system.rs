//! Independent SoC deadline fault injection. No RF-stop measurement is implied.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum WatchdogTestMode {
    /// Complete normally, then continue serving commands beyond the budget.
    Complete,
    /// Never return from the synchronous task poll.
    BlockedPoll,
    /// Drop an armed future without completing its physical obligation.
    Cancelled,
    /// Keep executor progress but never acknowledge completion.
    LostCompletion,
    /// Attempt restoration after the hardware deadline.
    LateRestoration,
}

/// Platform reset classification, independent of radio protocol resets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResetReason {
    Other,
    Software,
    MainWatchdog1,
}
/// Observed cause of the boot identified by the enclosing envelope.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BootEvidence {
    pub reset_reason: ResetReason,
}
