use oer_ieee80211_mac::block_ack::{BlockAckAction, OperationalTxBlockAck, TxBlockAckResponse};
use oer_ieee80211_mac::sequence::SequenceNumber;
use oer_time::{Duration, Instant};

use super::*;

/// The Espressif station's TIDs and a modulo-63 token sequence.
const POLICY: StaTxBlockAckPolicy = StaTxBlockAckPolicy {
    tids: &[0, 7, 5],
    first_dialog_token: 1,
    next_dialog_token,
};

const fn next_dialog_token(current: u8) -> u8 {
    if current >= 62 { 0 } else { current + 1 }
}

fn originator(window: u16, amsdu_tids: u8) -> StaTxBlockAckOriginator {
    StaTxBlockAckOriginator::new(
        POLICY,
        StaTxBlockAckConfig {
            window,
            negotiation_timeout: Duration::from_micros(100_000),
            amsdu_tids,
        },
    )
    .unwrap()
}

fn sequence(value: u16) -> SequenceNumber {
    SequenceNumber::new(value).unwrap()
}

fn response(dialog_token: u8, tid: u8, status: u16, window: u16) -> BlockAckAction {
    BlockAckAction::AddbaResponse {
        dialog_token,
        status,
        tid,
        immediate: true,
        amsdu: true,
        window,
        timeout_tu: 7,
    }
}

#[test]
fn the_policy_s_tids_share_one_token_sequence_and_route_responses() {
    let mut originator = originator(32, 1);
    let at = Instant::from_micros(0);
    let tid0 = originator.begin(0, sequence(0x100), at).unwrap();
    let tid7 = originator.begin(7, sequence(0x200), at).unwrap();
    let tid5 = originator.begin(5, sequence(0x300), at).unwrap();
    assert_eq!(
        [tid0.dialog_token, tid7.dialog_token, tid5.dialog_token],
        [1, 2, 3]
    );
    assert_eq!(originator.alarm(7), Some(tid7.alarm));
    assert_eq!(
        originator.on_response_action(response(tid7.dialog_token, 7, 0, 16)),
        Ok(StaTxBlockAckResponseDisposition::Matched(
            StaTxBlockAckResponse {
                tid: 7,
                response: TxBlockAckResponse::Operational(OperationalTxBlockAck {
                    tid: 7,
                    window: 16,
                    timeout_tu: 7,
                    starting_sequence: sequence(0x200),
                    amsdu: false,
                }),
            }
        ))
    );
    assert_eq!(originator.alarm(7), None);
    // TID 0 may carry A-MSDUs, as the configuration allows.
    assert!(matches!(
        originator.on_response_action(response(tid0.dialog_token, 0, 0, 16)),
        Ok(StaTxBlockAckResponseDisposition::Matched(
            StaTxBlockAckResponse {
                response: TxBlockAckResponse::Operational(OperationalTxBlockAck {
                    amsdu: true,
                    ..
                }),
                ..
            }
        ))
    ));
    assert_eq!(
        originator.expire_next(Instant::from_micros(100_000)),
        Some(5)
    );
    assert_eq!(originator.expire_next(Instant::from_micros(100_000)), None);
    assert!(originator.operational(7).is_some());
}

#[test]
fn the_earliest_deadline_follows_the_live_negotiations() {
    let mut originator = originator(16, 0);
    assert_eq!(originator.earliest_alarm_deadline(), None);
    originator
        .begin(0, sequence(0), Instant::from_micros(75))
        .unwrap();
    originator
        .begin(7, sequence(0), Instant::from_micros(25))
        .unwrap();
    originator
        .begin(5, sequence(0), Instant::from_micros(50))
        .unwrap();
    assert_eq!(
        originator.earliest_alarm_deadline(),
        Some(Instant::from_micros(100_025))
    );
    assert!(originator.stop(7));
    assert_eq!(
        originator.earliest_alarm_deadline(),
        Some(Instant::from_micros(100_050))
    );
}

#[test]
fn an_unowned_tid_and_a_stale_token_change_nothing() {
    let mut originator = originator(32, 0);
    assert_eq!(
        originator.begin(3, sequence(0), Instant::from_micros(0)),
        Err(StaTxBlockAckError::UnsupportedTid(3))
    );
    assert_eq!(
        originator.on_response(&[3, 1, 42, 0, 0, 0, 0, 0, 0]),
        Ok(StaTxBlockAckResponseDisposition::StaleDialogToken(42))
    );
    assert!(!originator.stop(3));
}

#[test]
fn attempts_requeue_until_the_peer_answers_or_they_run_out() {
    let mut originator = originator(32, 0);
    assert!(!originator.has_pending());
    originator.queue_initial(2);
    assert_eq!(originator.take_pending(), Some(0));
    assert_eq!(originator.take_pending(), Some(7));
    assert_eq!(originator.take_pending(), Some(5));
    assert_eq!(originator.take_pending(), None);

    // TID 0's request did not leave: its second attempt is queued.
    originator.transmit_failed(0);
    assert_eq!(originator.take_pending(), Some(0));
    let tid0 = originator
        .begin(0, sequence(0), Instant::from_micros(0))
        .unwrap();
    // Its second negotiation times out with no attempt left.
    assert_eq!(
        originator.expire_next(Instant::from_micros(100_000)),
        Some(0)
    );
    assert!(!originator.has_pending());
    assert_ne!(tid0.dialog_token, 0);

    // TID 7 is refused: the peer's answer ends its attempts.
    let tid7 = originator
        .begin(7, sequence(0), Instant::from_micros(0))
        .unwrap();
    assert_eq!(
        originator.on_response_action(response(tid7.dialog_token, 7, 37, 0)),
        Ok(StaTxBlockAckResponseDisposition::Matched(
            StaTxBlockAckResponse {
                tid: 7,
                response: TxBlockAckResponse::Rejected(37),
            }
        ))
    );
    assert!(!originator.has_pending());

    // Leaving ends everything and counts what was live.
    originator
        .begin(5, sequence(0), Instant::from_micros(0))
        .unwrap();
    assert_eq!(originator.stop_all(), 1);
    assert_eq!(originator.earliest_alarm_deadline(), None);
}

#[test]
fn a_policy_names_each_tid_once() {
    const TWICE: StaTxBlockAckPolicy = StaTxBlockAckPolicy {
        tids: &[0, 0],
        ..POLICY
    };
    assert!(matches!(
        StaTxBlockAckOriginator::new(
            TWICE,
            StaTxBlockAckConfig {
                window: 8,
                negotiation_timeout: Duration::from_micros(1),
                amsdu_tids: 0,
            },
        ),
        Err(StaTxBlockAckError::UnsupportedTid(0))
    ));
}
