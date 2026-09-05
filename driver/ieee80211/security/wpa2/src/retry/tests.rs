use super::*;
use crate::state::Wpa2TxMessage;

fn transmit() -> Wpa2Transmit {
    Wpa2Transmit {
        message: Wpa2TxMessage::PairwiseMessage3,
        replay_counter: 8,
        retransmission: false,
    }
}

#[test]
fn each_alarm_edge_emits_at_most_one_bounded_retransmission() {
    let mut retry = Wpa2Retry::new(Wpa2RetryConfig {
        first_interval_us: 100_000,
        subsequent_interval_us: 1_000_000,
        attempts: 2,
    })
    .unwrap();
    let first = retry.arm(transmit(), 10).unwrap();
    assert_eq!(first.deadline_us, 100_010);

    let Wpa2RetryAction::Transmit {
        frame,
        next_alarm: second,
    } = retry.on_alarm(first, first.deadline_us).unwrap()
    else {
        panic!("first alarm must retransmit and rearm")
    };
    assert!(frame.retransmission);
    assert_eq!(second.deadline_us, 1_100_010);

    assert!(matches!(
        retry.on_alarm(second, second.deadline_us).unwrap(),
        Wpa2RetryAction::Transmit { .. }
    ));
    let exhausted = Wpa2RetryAlarm {
        generation: second.generation,
        deadline_us: second.deadline_us + 1_000_000,
    };
    assert_eq!(
        retry.on_alarm(exhausted, exhausted.deadline_us).unwrap(),
        Wpa2RetryAction::Exhausted
    );
    assert!(!retry.is_armed());
    assert_eq!(
        retry.on_alarm(exhausted, exhausted.deadline_us).unwrap(),
        Wpa2RetryAction::Stale
    );
}

#[test]
fn cancel_invalidates_an_already_programmed_alarm() {
    let mut retry = Wpa2Retry::new(Wpa2RetryConfig {
        first_interval_us: 1,
        subsequent_interval_us: 1,
        attempts: 1,
    })
    .unwrap();
    let alarm = retry.arm(transmit(), 0).unwrap();
    retry.cancel();
    assert_eq!(retry.on_alarm(alarm, 1).unwrap(), Wpa2RetryAction::Stale);
}

#[test]
fn acknowledged_initial_frame_uses_the_subsequent_response_window() {
    let mut retry = Wpa2Retry::new(Wpa2RetryConfig {
        first_interval_us: 100_000,
        subsequent_interval_us: 1_000_000,
        attempts: 3,
    })
    .unwrap();
    retry.arm(transmit(), 10).unwrap();
    assert_eq!(
        retry.defer_first_after_ack(20).unwrap(),
        Some(Wpa2RetryAlarm {
            generation: 1,
            deadline_us: 1_000_020,
        })
    );
}
