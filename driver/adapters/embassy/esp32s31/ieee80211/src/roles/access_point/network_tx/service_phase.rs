//! Timing gate for the AP aggregate owner. Only `FinishAbort` permits detach.

use super::{
    AggregateTxServiceEvent, Esp32s31AccessPointDatapathError, Esp32s31ApAmpduError, WifiTxWake,
};

/// Reuse one deadline for publication and abort settling; never retain a timer
/// future alongside the DMA transaction.
#[derive(Clone, Copy)]
pub(super) enum AggregateServicePhase {
    Published(u64),
    AbortSettling(u64),
    ResetRequired,
}

#[derive(Debug, PartialEq)]
pub(super) enum AggregateServiceAction {
    Wait,
    Observe(AggregateTxServiceEvent),
    FinishAbort,
}

impl AggregateServicePhase {
    pub(super) fn deadline(self) -> u64 {
        match self {
            Self::Published(deadline) | Self::AbortSettling(deadline) => deadline,
            Self::ResetRequired => u64::MAX,
        }
    }

    pub(super) fn after_abort(now: u64) -> Result<Self, Esp32s31AccessPointDatapathError> {
        now.checked_add(16).map(Self::AbortSettling).ok_or(
            Esp32s31AccessPointDatapathError::Aggregate(Esp32s31ApAmpduError::DeadlineOverflow),
        )
    }

    pub(super) fn action(
        self,
        wake: WifiTxWake,
        now: impl FnOnce() -> Result<u64, Esp32s31AccessPointDatapathError>,
    ) -> Result<AggregateServiceAction, Esp32s31AccessPointDatapathError> {
        match self {
            Self::ResetRequired => Err(Esp32s31AccessPointDatapathError::Aggregate(
                Esp32s31ApAmpduError::DeadlineOverflow,
            )),
            Self::AbortSettling(deadline) => {
                // The abort already happened. Repeated/conflicting IRQs and
                // late completion cannot bypass the physical settle interval.
                Ok(if now()? < deadline {
                    AggregateServiceAction::Wait
                } else {
                    AggregateServiceAction::FinishAbort
                })
            }
            Self::Published(deadline) => {
                let mut event = AggregateTxServiceEvent::classify(wake).map_err(|error| {
                    Esp32s31AccessPointDatapathError::Aggregate(
                        Esp32s31ApAmpduError::ConflictingInterruptEvents(error.events),
                    )
                })?;
                if event == AggregateTxServiceEvent::ExecutorDeadline && now()? < deadline {
                    // Still observe actual completion; do not manufacture a timeout.
                    event = AggregateTxServiceEvent::Pending;
                }
                Ok(AggregateServiceAction::Observe(event))
            }
        }
    }
}

#[cfg(test)]
mod tests;
