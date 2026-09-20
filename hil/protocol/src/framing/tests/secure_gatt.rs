use super::*;
use crate::{
    BluetoothNumericChallenge, BluetoothNumericDecision, BluetoothSecureGattEvidence, RejectReason,
};

#[test]
fn reset_read_failure_request_preserves_epoch_and_boot_binding() {
    let expected = Envelope::new(
        77,
        9,
        0,
        8,
        Command::FailBluetoothGattResetRead { epoch: u32::MAX },
    );
    assert_eq!(expected.validate_target(78), Err(RejectReason::BootId));
    let mut encoder = FrameEncoder::new();
    let bytes = encoder.encode(&expected).unwrap();
    let mut decoder = FrameDecoder::new();
    let mut observed = None;
    decoder.feed::<Command>(bytes, |frame| observed = Some(frame.unwrap()));
    assert_eq!(observed, Some(expected));
}

#[test]
fn reset_reader_control_is_boot_bound_and_preserves_epoch_and_operation() {
    for release in [false, true] {
        let expected = Envelope::new(
            77,
            9,
            0,
            8,
            Command::BluetoothGattResetReadGate {
                epoch: u32::MAX,
                release,
            },
        );
        assert_eq!(expected.validate_target(78), Err(RejectReason::BootId));
        let mut encoder = FrameEncoder::new();
        let bytes = encoder.encode(&expected).unwrap();
        let mut decoder = FrameDecoder::new();
        let mut observed = None;
        decoder.feed::<Command>(bytes, |frame| observed = Some(frame.unwrap()));
        assert_eq!(observed, Some(expected));
    }
}

#[test]
fn secure_restart_is_boot_bound_and_preserves_requested_epoch() {
    let expected = Envelope::new(
        77,
        9,
        0,
        8,
        Command::RestartBluetoothGatt { epoch: u32::MAX },
    );
    assert_eq!(expected.validate_target(77), Ok(()));
    assert_eq!(expected.validate_target(78), Err(RejectReason::BootId));
    let mut encoder = FrameEncoder::new();
    let bytes = encoder.encode(&expected).unwrap();
    let mut decoder = FrameDecoder::new();
    let mut observed = None;
    decoder.feed::<Command>(bytes, |frame| observed = Some(frame.unwrap()));
    assert_eq!(observed, Some(expected));
}

#[test]
fn secure_decision_is_boot_bound_and_round_trips_without_key_material() {
    let decision = BluetoothNumericDecision {
        challenge: BluetoothNumericChallenge {
            id: u64::MAX,
            number: 999_999,
        },
        accept: true,
    };
    let expected = Envelope::new(77, 9, 0, 8, Command::ConfirmBluetoothGatt(decision));
    assert_eq!(expected.validate_target(77), Ok(()));
    assert_eq!(expected.validate_target(78), Err(RejectReason::BootId));
    let unknown = Envelope::new(0, 9, 0, 8, expected.body.clone());
    assert_eq!(unknown.validate_target(77), Err(RejectReason::BootId));
    let mut encoder = FrameEncoder::new();
    let bytes = encoder.encode(&expected).unwrap();
    let mut decoder = FrameDecoder::new();
    let mut observed = None;
    decoder.feed::<Command>(bytes, |frame| observed = Some(frame.unwrap()));
    assert_eq!(observed, Some(expected));
}
#[test]
fn secure_evidence_retains_independent_ui_bond_and_delivery_counts() {
    let expected = Envelope::new(
        u64::MAX,
        u32::MAX,
        0,
        u32::MAX,
        Event::BluetoothSecureGatt(BluetoothSecureGattEvidence {
            advertising_start_rejection: Some(
                crate::BluetoothAdvertisingStartRejection::TimingWindow,
            ),
            application_failure: Some(crate::BluetoothGattApplicationFailure::HciStatus(u8::MAX)),
            reset_read_gate: crate::BluetoothGattResetReadGate::ReaderHeld,
            bond_load_fault_armed: true,
            bond_load_failures: u32::MAX,
            shutdown: Some(crate::BluetoothGattShutdown {
                cause: crate::BluetoothGattStopCause::InjectedBondLoadFailure,
                reset: crate::BluetoothGattResetOutcome::Completed,
            }),
            epoch: u32::MAX,
            restarting: true,
            cold_releases: u32::MAX,
            old_hci_closed: true,
            comparison: Some(BluetoothNumericChallenge {
                id: u64::MAX,
                number: 999_999,
            }),
            comparisons: u32::MAX,
            accepted: u32::MAX,
            declined: u32::MAX,
            bonds_stored: u32::MAX,
            bonds_resumed: u32::MAX,
            pairing_failures: u32::MAX,
            rejected: u32::MAX,
            notifications_queued: u32::MAX,
            application_stopped: true,
            traffic: crate::BluetoothGattEvidence {
                address: Some([255; 6]),
                connections: u32::MAX,
                disconnections: u32::MAX,
                reads: u32::MAX,
                writes: u32::MAX,
                advertising_starts: u32::MAX,
                value: 255,
                last_disconnect_reason: Some(255),
                cpu0_stack: Some(crate::StackWatermark {
                    capacity_bytes: u32::MAX,
                    free_bytes: u32::MAX,
                    used_bytes: u32::MAX,
                    minimum_free_bytes: u32::MAX,
                }),
                ..Default::default()
            },
        }),
    );
    let mut encoder = FrameEncoder::new();
    let bytes = encoder.encode(&expected).unwrap();
    let mut decoder = FrameDecoder::new();
    let mut observed = None;
    decoder.feed::<Event>(bytes, |frame| observed = Some(frame.unwrap()));
    assert_eq!(observed, Some(expected));
}

#[test]
fn bond_load_fault_is_boot_bound_and_round_trips() {
    let expected = Envelope::new(
        77,
        9,
        0,
        8,
        Command::FailNextBluetoothGattBondLoad { epoch: u32::MAX },
    );
    assert_eq!(expected.validate_target(78), Err(RejectReason::BootId));
    let mut encoder = FrameEncoder::new();
    let bytes = encoder.encode(&expected).unwrap();
    let mut decoder = FrameDecoder::new();
    let mut observed = None;
    decoder.feed::<Command>(bytes, |frame| observed = Some(frame.unwrap()));
    assert_eq!(observed, Some(expected));
}
