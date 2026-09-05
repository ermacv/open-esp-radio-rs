use super::{StaTxBlockAckSessions, TxBlockAckDialogTokenSequence};

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
    let mut sessions = StaTxBlockAckSessions::new(16, 100_000, false).unwrap();
    assert_eq!(sessions.earliest_alarm_deadline(), None);

    sessions.begin(0, 0, 75).unwrap();
    sessions.begin(7, 0, 25).unwrap();
    sessions.begin(5, 0, 50).unwrap();

    assert_eq!(sessions.earliest_alarm_deadline(), Some(100_025));
    assert!(sessions.stop(7));
    assert_eq!(sessions.earliest_alarm_deadline(), Some(100_050));
}
