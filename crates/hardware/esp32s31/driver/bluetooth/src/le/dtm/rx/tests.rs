use oer_esp32s31_bluetooth_memory::{DtmRxResultProjection, DtmRxResultProjectionError, DtmRxRssi};

use super::{DtmReceiverSession, DtmRxCompletionOutcome};

#[test]
fn initial_state_has_no_accepted_rssi_sample() {
    let state = DtmReceiverSession::new();

    assert_eq!(state.received_packet_count(), 0);
    assert_eq!(state.last_rssi(), None);
}

#[test]
fn accepted_word_updates_signed_rssi_and_count_once() {
    let mut state = DtmReceiverSession::new();

    assert_eq!(
        state.account_projection(DtmRxResultProjection::from_word(0xa500_0000)),
        DtmRxCompletionOutcome::Counted {
            received_packet_count: 1,
            rssi: DtmRxRssi::from_controller_value(-91),
        }
    );
    assert_eq!(state.received_packet_count(), 1);
    assert_eq!(
        state.last_rssi().map(DtmRxRssi::controller_value),
        Some(-91)
    );
}

#[test]
fn accepted_rssi_preserves_the_signed_controller_domain() {
    let mut state = DtmReceiverSession::new();

    assert_eq!(
        state.account_projection(DtmRxResultProjection::from_word(0xff00_0000)),
        DtmRxCompletionOutcome::Counted {
            received_packet_count: 1,
            rssi: DtmRxRssi::from_controller_value(-1),
        }
    );
    assert_eq!(state.last_rssi().map(DtmRxRssi::controller_value), Some(-1));
}

#[test]
fn rejected_projection_preserves_state_after_rearm() {
    let mut state = DtmReceiverSession::new();
    let accepted = state.account_projection(DtmRxResultProjection::from_word(0x3100_0000));
    let rejected = state.account_projection(DtmRxResultProjection::from_word(0xff00_0001));

    assert!(matches!(accepted, DtmRxCompletionOutcome::Counted { .. }));
    assert_eq!(
        rejected,
        DtmRxCompletionOutcome::NotCounted {
            error: DtmRxResultProjectionError::NonzeroLowTwentyFourBits,
        }
    );
    assert_eq!(state.received_packet_count(), 1);
    assert_eq!(state.last_rssi().map(DtmRxRssi::controller_value), Some(49));
}

#[test]
fn count_uses_the_complete_wrapping_u16_transition() {
    let mut state = DtmReceiverSession::new();

    for _ in 0..=u16::MAX {
        let outcome = state.account_projection(DtmRxResultProjection::from_word(0));
        assert!(matches!(outcome, DtmRxCompletionOutcome::Counted { .. }));
    }

    assert_eq!(state.received_packet_count(), 0);
    assert_eq!(state.last_rssi().map(DtmRxRssi::controller_value), Some(0));
}
