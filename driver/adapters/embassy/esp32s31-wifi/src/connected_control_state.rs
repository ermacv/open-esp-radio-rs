//! Executor-independent state owned by one connected STA epoch.
//!
//! Delivery, wakeups and reorder-command publication remain in the Embassy
//! adapter. This module owns only association-scoped protocol state and
//! deadlines, so those resources cannot accidentally become mailbox state.

use open_esp_radio_esp32s31_wifi_mac::{
    rx_ampdu::{StaRxBlockAckActivation, StaRxBlockAckSessions},
    tx_ampdu::{STA_TX_BLOCK_ACK_TIDS, StaTxBlockAckSessions},
};
use open_esp_radio_ieee80211::station_power_save::StaPowerManagement;
use open_esp_radio_wifi_sta::{
    link_monitor::StaBeaconMonitor,
    power_save::{StaDozePermit, StaPowerSavePlanner},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectedControlTxKind {
    RxAddbaResponse { tid: u8 },
    TxAddbaRequest { tid: u8 },
    PowerManagement(StaPowerManagement),
}

pub(crate) enum ControlInFlight {
    RxAddba(StaRxBlockAckActivation),
    TxAddba { tid: u8 },
    PowerManagement(StaPowerManagement),
}

impl ControlInFlight {
    pub(crate) fn kind(&self) -> ConnectedControlTxKind {
        match self {
            Self::RxAddba(activation) => ConnectedControlTxKind::RxAddbaResponse {
                tid: activation.negotiated().tid,
            },
            Self::TxAddba { tid } => ConnectedControlTxKind::TxAddbaRequest { tid: *tid },
            Self::PowerManagement(mode) => ConnectedControlTxKind::PowerManagement(*mode),
        }
    }
}

/// Complete protocol state for one association.
///
/// Fields are crate-visible while the transition code is being separated
/// from its Embassy adapter. No executor-owned resource is allowed here.
pub(crate) struct ConnectedControlState {
    pub(crate) peer: [u8; 6],
    pub(crate) he_enabled: bool,
    pub(crate) rx_block_ack: StaRxBlockAckSessions,
    pub(crate) tx_block_ack: StaTxBlockAckSessions,
    pub(crate) initial_tx_block_ack: [bool; 3],
    pub(crate) in_flight: Option<ControlInFlight>,
    pub(crate) beacon_monitor: Option<StaBeaconMonitor>,
    pub(crate) beacon_lost: bool,
    pub(crate) power_save: Option<StaPowerSavePlanner>,
    pub(crate) pending_doze_permit: Option<StaDozePermit>,
}

impl ConnectedControlState {
    pub(crate) fn new(
        peer: [u8; 6],
        he_enabled: bool,
        tx_block_ack: StaTxBlockAckSessions,
    ) -> Self {
        Self {
            peer,
            he_enabled,
            rx_block_ack: StaRxBlockAckSessions::new(),
            tx_block_ack,
            initial_tx_block_ack: [false; 3],
            in_flight: None,
            beacon_monitor: None,
            beacon_lost: false,
            power_save: None,
            pending_doze_permit: None,
        }
    }

    pub(crate) fn has_immediate_work(&self, mailbox_has_event: bool) -> bool {
        self.in_flight.is_some()
            || mailbox_has_event
            || self.initial_tx_block_ack.into_iter().any(|pending| pending)
    }

    pub(crate) fn next_alarm_deadline(&self) -> Option<u64> {
        let block_ack = STA_TX_BLOCK_ACK_TIDS
            .into_iter()
            .filter_map(|tid| self.tx_block_ack.alarm(tid))
            .map(|alarm| alarm.deadline_us)
            .min();
        match (
            block_ack,
            self.beacon_monitor
                .as_ref()
                .and_then(StaBeaconMonitor::deadline_micros),
        ) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (Some(deadline), None) | (None, Some(deadline)) => Some(deadline),
            (None, None) => None,
        }
    }

    pub(crate) fn has_pending_traffic(
        &self,
        network_tx_pending: bool,
        mailbox_has_event: bool,
    ) -> bool {
        network_tx_pending
            || mailbox_has_event
            || self.initial_tx_block_ack.into_iter().any(|pending| pending)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> ConnectedControlState {
        ConnectedControlState::new(
            [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
            true,
            StaTxBlockAckSessions::new(32, 100_000, true).unwrap(),
        )
    }

    #[test]
    fn protocol_readiness_combines_owned_state_with_external_mailbox_state() {
        let mut state = state();
        assert!(!state.has_immediate_work(false));
        assert!(state.has_immediate_work(true));
        assert!(!state.has_pending_traffic(false, false));
        assert!(state.has_pending_traffic(false, true));

        state.initial_tx_block_ack[1] = true;
        assert!(state.has_immediate_work(false));
        assert!(state.has_pending_traffic(false, false));
    }

    #[test]
    fn protocol_deadline_is_computed_without_an_executor_timer() {
        let mut state = state();
        assert_eq!(state.next_alarm_deadline(), None);

        state.tx_block_ack.begin(7, 23, 50).unwrap();
        assert_eq!(state.next_alarm_deadline(), Some(100_050));
    }
}
