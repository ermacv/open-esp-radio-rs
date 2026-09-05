use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothDtmRxResultProjection, BluetoothDtmRxResultProjectionError, BluetoothDtmRxRssi,
};

use super::{BluetoothDtmReceiverSession, BluetoothDtmRxCompletionOutcome};

#[test]
fn initial_state_has_no_accepted_rssi_sample() {
    let state = BluetoothDtmReceiverSession::new();

    assert_eq!(state.received_packet_count(), 0);
    assert_eq!(state.last_rssi(), None);
}

#[test]
fn accepted_word_updates_signed_rssi_and_count_once() {
    let mut state = BluetoothDtmReceiverSession::new();

    assert_eq!(
        state.account_projection(BluetoothDtmRxResultProjection::from_word(0xa500_0000)),
        BluetoothDtmRxCompletionOutcome::Counted {
            received_packet_count: 1,
            rssi: BluetoothDtmRxRssi::from_controller_value(-91),
        }
    );
    assert_eq!(state.received_packet_count(), 1);
    assert_eq!(
        state.last_rssi().map(BluetoothDtmRxRssi::controller_value),
        Some(-91)
    );
}

#[test]
fn accepted_rssi_preserves_the_signed_controller_domain() {
    let mut state = BluetoothDtmReceiverSession::new();

    assert_eq!(
        state.account_projection(BluetoothDtmRxResultProjection::from_word(0xff00_0000)),
        BluetoothDtmRxCompletionOutcome::Counted {
            received_packet_count: 1,
            rssi: BluetoothDtmRxRssi::from_controller_value(-1),
        }
    );
    assert_eq!(
        state.last_rssi().map(BluetoothDtmRxRssi::controller_value),
        Some(-1)
    );
}

#[test]
fn rejected_projection_preserves_state_after_rearm() {
    let mut state = BluetoothDtmReceiverSession::new();
    let accepted = state.account_projection(BluetoothDtmRxResultProjection::from_word(0x3100_0000));
    let rejected = state.account_projection(BluetoothDtmRxResultProjection::from_word(0xff00_0001));

    assert!(matches!(
        accepted,
        BluetoothDtmRxCompletionOutcome::Counted { .. }
    ));
    assert_eq!(
        rejected,
        BluetoothDtmRxCompletionOutcome::NotCounted {
            error: BluetoothDtmRxResultProjectionError::NonzeroLowTwentyFourBits,
        }
    );
    assert_eq!(state.received_packet_count(), 1);
    assert_eq!(
        state.last_rssi().map(BluetoothDtmRxRssi::controller_value),
        Some(49)
    );
}

#[test]
fn count_uses_the_complete_wrapping_u16_transition() {
    let mut state = BluetoothDtmReceiverSession::new();

    for _ in 0..=u16::MAX {
        let outcome = state.account_projection(BluetoothDtmRxResultProjection::from_word(0));
        assert!(matches!(
            outcome,
            BluetoothDtmRxCompletionOutcome::Counted { .. }
        ));
    }

    assert_eq!(state.received_packet_count(), 0);
    assert_eq!(
        state.last_rssi().map(BluetoothDtmRxRssi::controller_value),
        Some(0)
    );
}
