use super::{StaTxBlockAckSessions, TxBlockAckDialogTokenSequence};
use oer_ieee80211_mac::sequence::SequenceNumber;

#[test]
fn shared_dialog_tokens_reproduce_the_qualified_vendor_modulus() {
    let mut tokens = TxBlockAckDialogTokenSequence::new();
    for expected in 1..=62 {
        assert_eq!(tokens.take().value(), expected);
    }
    assert_eq!(tokens.take().value(), 0);
    assert_eq!(tokens.take().value(), 1);
}

#[test]
fn earliest_alarm_deadline_walks_owned_slots_without_tid_remapping() {
    let mut sessions =
        StaTxBlockAckSessions::new(16, oer_time::Duration::from_micros(100_000), false).unwrap();
    assert_eq!(sessions.earliest_alarm_deadline(), None);

    sessions
        .begin(
            0,
            SequenceNumber::new(0).unwrap(),
            oer_time::Instant::from_micros(75),
        )
        .unwrap();
    sessions
        .begin(
            7,
            SequenceNumber::new(0).unwrap(),
            oer_time::Instant::from_micros(25),
        )
        .unwrap();
    sessions
        .begin(
            5,
            SequenceNumber::new(0).unwrap(),
            oer_time::Instant::from_micros(50),
        )
        .unwrap();

    assert_eq!(
        sessions.earliest_alarm_deadline(),
        Some(oer_time::Instant::from_micros(100_025))
    );
    assert!(sessions.stop(7));
    assert_eq!(
        sessions.earliest_alarm_deadline(),
        Some(oer_time::Instant::from_micros(100_050))
    );
}
