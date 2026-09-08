//! Hardware-independent quiescence decision for one active LE DTM event.

#![forbid(unsafe_code)]

/// Hardware visibility retained by a rejected recurring transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DtmQuiescenceRetryOwnership {
    /// Preparation or HEAD publication was rejected before hardware visibility.
    BeforeHead,
    /// `HEAD` is visible but `RUN` has not succeeded yet.
    HeadPublished,
}

/// Sole quiescence action permitted by the retry owner's visibility.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DtmQuiescenceRetryAction {
    /// Cancel the still CPU-owned recurring transaction.
    CancelBeforeHead,
    /// Retry scheduler start and complete exactly the visible final event.
    FinishPublishedHead,
}

pub(crate) const fn bluetooth_dtm_quiescence_retry_action(
    ownership: DtmQuiescenceRetryOwnership,
) -> DtmQuiescenceRetryAction {
    match ownership {
        DtmQuiescenceRetryOwnership::BeforeHead => DtmQuiescenceRetryAction::CancelBeforeHead,
        DtmQuiescenceRetryOwnership::HeadPublished => DtmQuiescenceRetryAction::FinishPublishedHead,
    }
}

#[cfg(test)]
mod tests;

/// One absolute budget shared by cancellation, stop, head retirement and unlink.
/// Copying the value preserves the original deadline across owned transitions.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DtmQuiescenceDeadline {
    started: u64,
    expires: Option<u64>,
}

impl DtmQuiescenceDeadline {
    pub(crate) const fn new(now_micros: u64) -> Self {
        Self {
            started: now_micros,
            expires: now_micros.checked_add(100_000),
        }
    }
    pub(crate) const fn expired(self, now_micros: u64) -> bool {
        match self.expires {
            Some(deadline) => now_micros < self.started || now_micros >= deadline,
            None => true,
        }
    }
}
