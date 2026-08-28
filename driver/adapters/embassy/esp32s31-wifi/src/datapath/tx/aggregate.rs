//! Role-neutral aggregate-TX scheduler and transaction facts.

use open_esp_radio_esp32s31_wifi_mac::irq::{EVENT_COLLISION, EVENT_TX_COMPLETE, EVENT_TX_TIMEOUT};

use crate::datapath::WifiTxWake;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AggregateTxServiceEvent {
    Pending,
    Completion,
    HardwareTimeout,
    Collision,
    ExecutorDeadline,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AggregateTxServiceEventError {
    pub(crate) events: u32,
}

impl AggregateTxServiceEvent {
    pub(crate) fn classify(wake: WifiTxWake) -> Result<Self, AggregateTxServiceEventError> {
        let WifiTxWake::Interrupt { events } = wake else {
            return Ok(Self::ExecutorDeadline);
        };
        let tx_events = events & (EVENT_TX_COMPLETE | EVENT_TX_TIMEOUT | EVENT_COLLISION);
        if tx_events.count_ones() > 1 {
            return Err(AggregateTxServiceEventError { events: tx_events });
        }
        Ok(match tx_events {
            EVENT_TX_COMPLETE => Self::Completion,
            EVENT_TX_TIMEOUT => Self::HardwareTimeout,
            EVENT_COLLISION => Self::Collision,
            _ => Self::Pending,
        })
    }
}

#[cfg(test)]
mod tests {
    use open_esp_radio_esp32s31_wifi_mac::irq::{
        EVENT_COLLISION, EVENT_TX_COMPLETE, EVENT_TX_TIMEOUT,
    };

    use super::*;

    #[test]
    fn unrelated_irq_bits_do_not_forge_a_tx_event() {
        assert_eq!(
            AggregateTxServiceEvent::classify(WifiTxWake::Interrupt {
                events: 0x8000_0000,
            }),
            Ok(AggregateTxServiceEvent::Pending)
        );
    }

    #[test]
    fn deadline_and_each_hardware_reason_remain_distinct() {
        assert_eq!(
            AggregateTxServiceEvent::classify(WifiTxWake::Deadline),
            Ok(AggregateTxServiceEvent::ExecutorDeadline)
        );
        assert_eq!(
            AggregateTxServiceEvent::classify(WifiTxWake::Interrupt {
                events: EVENT_TX_COMPLETE,
            }),
            Ok(AggregateTxServiceEvent::Completion)
        );
        assert_eq!(
            AggregateTxServiceEvent::classify(WifiTxWake::Interrupt {
                events: EVENT_TX_TIMEOUT,
            }),
            Ok(AggregateTxServiceEvent::HardwareTimeout)
        );
        assert_eq!(
            AggregateTxServiceEvent::classify(WifiTxWake::Interrupt {
                events: EVENT_COLLISION,
            }),
            Ok(AggregateTxServiceEvent::Collision)
        );
    }

    #[test]
    fn conflicting_terminal_events_fail_closed() {
        let events = EVENT_TX_COMPLETE | EVENT_COLLISION;
        assert_eq!(
            AggregateTxServiceEvent::classify(WifiTxWake::Interrupt { events }),
            Err(AggregateTxServiceEventError { events })
        );
    }
}
