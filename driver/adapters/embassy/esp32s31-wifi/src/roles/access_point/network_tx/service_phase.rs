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
mod tests {
    use super::*;
    use open_esp_radio_esp32s31_wifi_mac::irq::{
        EVENT_COLLISION, EVENT_TX_COMPLETE, EVENT_TX_TIMEOUT,
    };

    #[test]
    fn early_deadline_still_observes_completion_without_aborting() {
        let phase = AggregateServicePhase::Published(100);
        assert_eq!(
            phase.action(WifiTxWake::Deadline, || Ok(99)).unwrap(),
            AggregateServiceAction::Observe(AggregateTxServiceEvent::Pending)
        );
        assert_eq!(
            phase.action(WifiTxWake::Deadline, || Ok(100)).unwrap(),
            AggregateServiceAction::Observe(AggregateTxServiceEvent::ExecutorDeadline)
        );
    }

    #[test]
    fn published_interrupts_do_not_sample_the_clock() {
        for (events, expected) in [
            (EVENT_TX_COMPLETE, AggregateTxServiceEvent::Completion),
            (EVENT_TX_TIMEOUT, AggregateTxServiceEvent::HardwareTimeout),
            (EVENT_COLLISION, AggregateTxServiceEvent::Collision),
        ] {
            assert_eq!(
                AggregateServicePhase::Published(100)
                    .action(WifiTxWake::Interrupt { events }, || panic!(
                        "unneeded clock read"
                    ))
                    .unwrap(),
                AggregateServiceAction::Observe(expected)
            );
        }
    }

    #[test]
    fn every_wake_preserves_abort_until_the_post_request_deadline() {
        let phase = AggregateServicePhase::after_abort(100).unwrap();
        assert_eq!(phase.deadline(), 116);
        for wake in [
            WifiTxWake::Deadline,
            WifiTxWake::Interrupt {
                events: EVENT_TX_TIMEOUT,
            },
            WifiTxWake::Interrupt {
                events: EVENT_TX_COMPLETE,
            },
            WifiTxWake::Interrupt {
                events: EVENT_TX_COMPLETE | EVENT_COLLISION,
            },
            WifiTxWake::Interrupt { events: 0 },
        ] {
            for now in 100..116 {
                assert_eq!(
                    phase.action(wake, || Ok(now)).unwrap(),
                    AggregateServiceAction::Wait
                );
            }
            assert_eq!(
                phase.action(wake, || Ok(116)).unwrap(),
                AggregateServiceAction::FinishAbort
            );
        }
    }

    #[test]
    fn conflicting_published_interrupts_fail_closed() {
        assert!(matches!(
            AggregateServicePhase::Published(100).action(
                WifiTxWake::Interrupt {
                    events: EVENT_TX_COMPLETE | EVENT_TX_TIMEOUT
                },
                || panic!("conflict must not read time")
            ),
            Err(Esp32s31AccessPointDatapathError::Aggregate(
                Esp32s31ApAmpduError::ConflictingInterruptEvents(_)
            ))
        ));
    }

    #[test]
    fn overflow_cannot_turn_into_a_wrapped_or_successful_abort() {
        assert!(AggregateServicePhase::after_abort(u64::MAX - 15).is_err());
        let phase = AggregateServicePhase::after_abort(u64::MAX - 16).unwrap();
        assert_eq!(phase.deadline(), u64::MAX);
        assert_eq!(
            phase
                .action(WifiTxWake::Deadline, || Ok(u64::MAX - 1))
                .unwrap(),
            AggregateServiceAction::Wait
        );
        assert!(
            AggregateServicePhase::ResetRequired
                .action(
                    WifiTxWake::Interrupt {
                        events: EVENT_TX_COMPLETE
                    },
                    || panic!("fault is terminal")
                )
                .is_err()
        );
    }
}
