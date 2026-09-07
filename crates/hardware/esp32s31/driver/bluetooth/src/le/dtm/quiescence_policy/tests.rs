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
