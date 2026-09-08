use super::{
    DtmQuiescenceRetryAction, DtmQuiescenceRetryOwnership, bluetooth_dtm_quiescence_retry_action,
};

#[test]
fn retry_visibility_selects_cancel_or_one_final_event() {
    use DtmQuiescenceRetryAction::{CancelBeforeHead, FinishPublishedHead};

    use DtmQuiescenceRetryOwnership::{BeforeHead, HeadPublished};

    let cases = [
        (BeforeHead, CancelBeforeHead),
        (HeadPublished, FinishPublishedHead),
    ];

    for (ownership, expected) in cases {
        assert_eq!(bluetooth_dtm_quiescence_retry_action(ownership), expected);
    }
}

#[test]
fn quiescence_deadline_survives_retries_and_expires_at_boundary() {
    let deadline = super::DtmQuiescenceDeadline::new(20);
    assert!(!deadline.expired(20));
    assert!(!deadline.expired(100_019));
    let retained = deadline;
    assert!(retained.expired(100_020));
    assert!(retained.expired(100_021));
}

#[test]
fn quiescence_clock_discontinuity_fails_closed() {
    assert!(super::DtmQuiescenceDeadline::new(20).expired(19));
    assert!(super::DtmQuiescenceDeadline::new(u64::MAX).expired(u64::MAX));
}
