//! Hardware-independent quiescence decision for one active LE DTM event.

#![forbid(unsafe_code)]

/// Hardware visibility retained by a rejected recurring transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BluetoothDtmQuiescenceRetryOwnership {
    /// Preparation or HEAD publication was rejected before hardware visibility.
    BeforeHead,
    /// `HEAD` is visible but `RUN` has not succeeded yet.
    HeadPublished,
}

/// Sole quiescence action permitted by the retry owner's visibility.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BluetoothDtmQuiescenceRetryAction {
    /// Cancel the still CPU-owned recurring transaction.
    CancelBeforeHead,
    /// Retry scheduler start and complete exactly the visible final event.
    FinishPublishedHead,
}

pub(crate) const fn bluetooth_dtm_quiescence_retry_action(
    ownership: BluetoothDtmQuiescenceRetryOwnership,
) -> BluetoothDtmQuiescenceRetryAction {
    match ownership {
        BluetoothDtmQuiescenceRetryOwnership::BeforeHead => {
            BluetoothDtmQuiescenceRetryAction::CancelBeforeHead
        }
        BluetoothDtmQuiescenceRetryOwnership::HeadPublished => {
            BluetoothDtmQuiescenceRetryAction::FinishPublishedHead
        }
    }
}

#[cfg(test)]
mod tests;
