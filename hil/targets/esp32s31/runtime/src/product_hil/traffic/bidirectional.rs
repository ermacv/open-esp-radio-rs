#![forbid(unsafe_code)]

use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use open_esp_radio_hil_protocol::{
    Direction, RadioEvidence, RxDeliveryEvidence, TransportEvidence, TxAggregateTimingEvidence,
};

use crate::console::{ActiveSession, complete_session};

use super::SessionChannel;

pub(in crate::product_hil) type BidirectionalSessionChannel =
    Channel<CriticalSectionRawMutex, ActiveSession, 1>;
pub(in crate::product_hil) type BidirectionalResultChannel =
    Channel<CriticalSectionRawMutex, OpenRadioBidirectionalResult, 2>;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(in crate::product_hil) enum OpenRadioBidirectionalDirection {
    Rx,
    Tx,
}

#[derive(Clone, Copy)]
pub(in crate::product_hil) struct OpenRadioBidirectionalResult {
    session_id: u64,
    direction: OpenRadioBidirectionalDirection,
    evidence: TransportEvidence,
    radio: Option<RadioEvidence>,
    tx_timing: Option<TxAggregateTimingEvidence>,
    rx_delivery: Option<RxDeliveryEvidence>,
    passed: bool,
}

impl OpenRadioBidirectionalResult {
    pub(in crate::product_hil) const fn new(
        session_id: u64,
        direction: OpenRadioBidirectionalDirection,
        evidence: TransportEvidence,
        radio: Option<RadioEvidence>,
        tx_timing: Option<TxAggregateTimingEvidence>,
        rx_delivery: Option<RxDeliveryEvidence>,
        passed: bool,
    ) -> Self {
        Self {
            session_id,
            direction,
            evidence,
            radio,
            tx_timing,
            rx_delivery,
            passed,
        }
    }
}

pub(in crate::product_hil) async fn run_open_radio_bidirectional_session_coordinator(
    input: &'static SessionChannel,
    rx_sessions: &'static BidirectionalSessionChannel,
    tx_sessions: &'static BidirectionalSessionChannel,
    results: &'static BidirectionalResultChannel,
) -> ! {
    loop {
        let session = input.receive().await;
        match session.config.direction {
            Direction::Rx => {
                rx_sessions.send(session).await;
                complete_single_direction(
                    session.session_id,
                    OpenRadioBidirectionalDirection::Rx,
                    results.receive().await,
                )
                .await;
            }
            Direction::Tx => {
                tx_sessions.send(session).await;
                complete_single_direction(
                    session.session_id,
                    OpenRadioBidirectionalDirection::Tx,
                    results.receive().await,
                )
                .await;
            }
            Direction::Bidirectional => {
                rx_sessions.send(session).await;
                tx_sessions.send(session).await;
                let first = results.receive().await;
                let second = results.receive().await;
                let valid_pair = first.session_id == session.session_id
                    && second.session_id == session.session_id
                    && first.direction != second.direction;
                let evidence = TransportEvidence {
                    rx_bytes: first
                        .evidence
                        .rx_bytes
                        .saturating_add(second.evidence.rx_bytes),
                    tx_bytes: first
                        .evidence
                        .tx_bytes
                        .saturating_add(second.evidence.tx_bytes),
                    rx_units: first
                        .evidence
                        .rx_units
                        .saturating_add(second.evidence.rx_units),
                    tx_units: first
                        .evidence
                        .tx_units
                        .saturating_add(second.evidence.tx_units),
                    elapsed_micros: first
                        .evidence
                        .elapsed_micros
                        .max(second.evidence.elapsed_micros),
                    transport_errors: first
                        .evidence
                        .transport_errors
                        .saturating_add(second.evidence.transport_errors)
                        .saturating_add(u32::from(!valid_pair)),
                };
                complete_session(
                    session.session_id,
                    evidence,
                    merge_radio(first.radio, second.radio),
                    first.tx_timing.or(second.tx_timing),
                    if first.direction == OpenRadioBidirectionalDirection::Rx {
                        first.rx_delivery
                    } else {
                        second.rx_delivery
                    },
                    valid_pair && first.passed && second.passed,
                )
                .await;
            }
        }
    }
}

async fn complete_single_direction(
    session_id: u64,
    expected_direction: OpenRadioBidirectionalDirection,
    result: OpenRadioBidirectionalResult,
) {
    let valid = result.session_id == session_id && result.direction == expected_direction;
    let mut evidence = result.evidence;
    evidence.transport_errors = evidence.transport_errors.saturating_add(u32::from(!valid));
    complete_session(
        session_id,
        evidence,
        result.radio,
        result.tx_timing,
        result.rx_delivery,
        valid && result.passed,
    )
    .await;
}

pub(in crate::product_hil) async fn complete_open_radio_bidirectional_direction(
    results: &'static BidirectionalResultChannel,
    result: OpenRadioBidirectionalResult,
) {
    results.send(result).await;
}

fn merge_radio(
    first: Option<RadioEvidence>,
    second: Option<RadioEvidence>,
) -> Option<RadioEvidence> {
    match (first, second) {
        (None, None) => None,
        (first, second) => Some(RadioEvidence {
            rx: first
                .and_then(|value| value.rx)
                .or_else(|| second.and_then(|value| value.rx)),
            tx: first
                .and_then(|value| value.tx)
                .or_else(|| second.and_then(|value| value.tx)),
        }),
    }
}
