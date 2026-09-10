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
