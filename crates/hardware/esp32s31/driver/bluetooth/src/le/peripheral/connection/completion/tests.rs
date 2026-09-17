use core::cell::Cell;

use oer_bluetooth_ll::connection::LePeripheralConnectionEventPeerActivity;

use super::{PeripheralConnectionCaptureCompletion, classify_peripheral_connection_capture};

#[test]
fn absent_connection_capture_is_a_missed_event_without_normalization() {
    let called = Cell::new(false);

    let result = classify_peripheral_connection_capture(None::<()>, false, |_| {
        called.set(true);
        Some(())
    });

    let PeripheralConnectionCaptureCompletion::Complete {
        activity,
        packet_start,
    } = result
    else {
        panic!("an absent capture completes without timestamp normalization");
    };
    assert_eq!(activity, LePeripheralConnectionEventPeerActivity::Missed);
    assert_eq!(packet_start, None);
    assert!(!called.get());
}

#[test]
fn available_connection_capture_is_observed_after_one_normalization() {
    let calls = Cell::new(0);

    let result = classify_peripheral_connection_capture(Some(()), false, |_| {
        calls.set(calls.get() + 1);
        Some(37_u32)
    });

    let PeripheralConnectionCaptureCompletion::Complete {
        activity,
        packet_start,
    } = result
    else {
        panic!("a normalized available capture completes as observed");
    };
    assert_eq!(activity, LePeripheralConnectionEventPeerActivity::Observed);
    assert_eq!(packet_start, Some(37));
    assert_eq!(calls.get(), 1);
}

#[test]
fn available_connection_capture_without_normalization_remains_uncompleted() {
    let calls = Cell::new(0);

    let result = classify_peripheral_connection_capture(Some(()), false, |_| {
        calls.set(calls.get() + 1);
        None::<()>
    });

    assert!(matches!(
        result,
        PeripheralConnectionCaptureCompletion::NormalizationUnavailable
    ));
    assert_eq!(calls.get(), 1);
}

#[test]
fn accepted_packet_establishes_without_inventing_a_missing_capture() {
    let result = classify_peripheral_connection_capture(None::<()>, true, |_| -> Option<u32> {
        panic!("no captured timestamp exists to normalize");
    });
    let PeripheralConnectionCaptureCompletion::Complete {
        activity,
        packet_start,
    } = result
    else {
        panic!("accepted RX is peer activity even without an anchor capture");
    };
    assert_eq!(activity, LePeripheralConnectionEventPeerActivity::Observed);
    assert_eq!(packet_start, None);
}

#[test]
fn only_valid_plaintext_receive_inside_completed_window_releases_recovery() {
    use crate::scheduler::SchedulerRawWindow;
    use oer_bluetooth_ll::security::LePeripheralEncryptionReceiveMode::*;
    use oer_esp32s31_bluetooth_memory::PeripheralConnectionReceiveTime;
    for start in [100, u32::MAX - 100] {
        let window =
            SchedulerRawWindow::from_projected_scheduler_window(start, start.wrapping_add(200))
                .unwrap();
        for (delta, expected) in [
            (u32::MAX, false),
            (0, true),
            (100, true),
            (199, true),
            (200, false),
        ] {
            let receive =
                PeripheralConnectionReceiveTime::from_controller_ticks(start.wrapping_add(delta));
            assert_eq!(
                super::plaintext_receive_releases_recovery(window, receive, Plaintext, Plaintext),
                expected
            );
            for protected in [
                EncryptedStartResponse,
                Encrypted,
                UnencryptedPauseResponse,
                RestartEncryptionRequest,
                Blocked,
            ] {
                assert!(!super::plaintext_receive_releases_recovery(
                    window, receive, protected, Plaintext
                ));
                assert!(!super::plaintext_receive_releases_recovery(
                    window, receive, Plaintext, protected
                ));
            }
        }
        // The prior event's valid timestamp cannot release a later pause.
        let next = SchedulerRawWindow::from_projected_scheduler_window(
            start.wrapping_add(15_000),
            start.wrapping_add(15_200),
        )
        .unwrap();
        let prior = PeripheralConnectionReceiveTime::from_controller_ticks(start.wrapping_add(100));
        let fresh =
            PeripheralConnectionReceiveTime::from_controller_ticks(start.wrapping_add(15_100));
        assert!(!super::plaintext_receive_releases_recovery(
            next, prior, Plaintext, Plaintext
        ));
        assert!(super::plaintext_receive_releases_recovery(
            next, fresh, Plaintext, Plaintext
        ));
    }
}
