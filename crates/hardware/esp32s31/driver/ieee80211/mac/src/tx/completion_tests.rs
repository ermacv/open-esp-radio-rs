use super::*;

fn ordinary(status: u8, detail: u8) -> Result<CompletionObservation, CompletionFault> {
    decode(CompletionRaw::ordinary(
        MacTxCompletionObservation::new_model(status, detail),
    ))
}

#[test]
fn rts_handler_preserves_all_detail_branches() {
    for detail in 0..=u8::MAX {
        let expected = match detail {
            1 | 3..=5 | 0xa0..=0xad => Ok(CompletionObservation::Observed(TxStatus::Collision)),
            0xc0 => Err(CompletionFault::SecurityKeyError),
            _ => Ok(CompletionObservation::NoEffect),
        };
        assert_eq!(ordinary(1, detail), expected, "RTS detail {detail:#x}");
    }
}

#[test]
fn handler_arguments_are_independent_of_the_hardware_trigger_flag() {
    assert_eq!(
        decode(CompletionRaw::rts_error(0xc0, 1)),
        Ok(CompletionObservation::NoEffect)
    );
    assert_eq!(
        decode(CompletionRaw::rts_error(0xc0, u32::MAX)),
        Ok(CompletionObservation::NoEffect)
    );
    assert_eq!(
        decode(CompletionRaw::rts_error(0xc0, 0)),
        Err(CompletionFault::SecurityKeyError)
    );
    for trigger in [false, true] {
        let raw = MacTxCompletionObservation::new_model(1, 0xc0).with_trigger_flow_model(trigger);
        assert_eq!(
            decode(CompletionRaw::ordinary(raw)),
            Err(CompletionFault::SecurityKeyError)
        );
    }
}

#[test]
fn tx_error_uses_its_selector_for_collisions_and_key_errors() {
    for selector in [0, 1, u32::MAX] {
        for detail in 0..=u8::MAX {
            let expected = match detail {
                0 => Ok(CompletionObservation::Observed(TxStatus::CtsTimeout)),
                1 | 3..=5 if selector == 0 => {
                    Ok(CompletionObservation::Observed(TxStatus::Collision))
                }
                0xc0 if selector != 0 => Ok(CompletionObservation::NoEffect),
                0xc0 => Err(CompletionFault::SecurityKeyError),
                _ => Ok(CompletionObservation::Observed(TxStatus::AckTimeout)),
            };
            assert_eq!(decode(CompletionRaw::tx_error(detail, selector)), expected);
        }
    }
}

#[test]
fn top_level_dispatch_rejects_only_statuses_outside_the_library_branches() {
    assert_eq!(
        ordinary(0, 0xff),
        Ok(CompletionObservation::Observed(TxStatus::Success))
    );
    assert_eq!(
        ordinary(2, 0xff),
        Ok(CompletionObservation::Observed(TxStatus::CtsTimeout))
    );
    assert_eq!(
        ordinary(5, 0xff),
        Ok(CompletionObservation::Observed(TxStatus::AckTimeout))
    );
    for status in core::iter::once(3).chain(6..=u8::MAX) {
        assert_eq!(
            ordinary(status, 0),
            Err(CompletionFault::InvalidStatus { status })
        );
    }
}

#[test]
fn no_effect_and_faults_never_construct_completed_metadata() {
    assert_eq!(
        TxCompletion::new_model(TxCookie(1), 1, 2),
        Ok(CompletionEvent::NoEffect)
    );
    assert_eq!(
        TxCompletion::new_model(TxCookie(1), 1, 0xc0),
        Err(CompletionFault::SecurityKeyError)
    );
    assert_eq!(
        TxCompletion::new_model(TxCookie(1), 4, 0xc0),
        Err(CompletionFault::SecurityKeyError)
    );
}

#[test]
fn virtual_queue_ten_resolves_the_owning_descriptor_instead_of_indexing_queue_ten() {
    for queue in 0..5 {
        let descriptor_word = 0xf0ff_ff0f | (u32::from(queue) << 4);
        assert_eq!(
            CompletionQueue::from_vendor_argument(10, descriptor_word)
                .unwrap()
                .index(),
            queue
        );
        assert_eq!(
            CompletionQueue::from_vendor_argument(queue, u32::MAX)
                .unwrap()
                .index(),
            queue
        );
    }
    assert_eq!(
        CompletionQueue::from_vendor_argument(10, 0xf0),
        Err(CompletionQueueError {
            argument: 10,
            resolved: 15
        })
    );
    assert_eq!(
        CompletionQueue::from_vendor_argument(6, 0),
        Err(CompletionQueueError {
            argument: 6,
            resolved: 6
        })
    );
}
