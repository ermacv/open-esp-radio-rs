//! Exercise the production request channel, including cancellation and epoch end.
#[allow(
    dead_code,
    reason = "host tests include the request module without its target-only supervisor consumer"
)]
#[path = "../src/supervisor/station/pause_request.rs"]
mod pause_request;
use pause_request::{PauseError, PauseOperation, PauseReport, Requests};
use std::{
    future::Future,
    task::{Context, Poll, Waker},
};

#[test]
fn cancellation_cannot_reuse_an_inflight_request_or_deliver_a_stale_completion() {
    let requests = Requests::new();
    let mut cx = Context::from_waker(Waker::noop());
    let mut unavailable = std::pin::pin!(pause_request::station_pause_round_trip(
        PauseOperation::Access
    ));
    assert_eq!(
        unavailable.as_mut().poll(&mut cx),
        Poll::Ready(Err(PauseError::Unavailable))
    );
    let availability = requests.open();
    {
        let mut first = std::pin::pin!(requests.request(PauseOperation::Access));
        assert!(first.as_mut().poll(&mut cx).is_pending());
        let mut second = std::pin::pin!(requests.request(PauseOperation::Access));
        assert_eq!(
            second.as_mut().poll(&mut cx),
            Poll::Ready(Err(PauseError::Busy))
        );
        let mut server = std::pin::pin!(requests.wait());
        assert!(server.as_mut().poll(&mut cx).is_ready());
        // Cancel after the server accepted the request.
    }
    let mut busy = std::pin::pin!(requests.request(PauseOperation::Access));
    assert_eq!(
        busy.as_mut().poll(&mut cx),
        Poll::Ready(Err(PauseError::Busy))
    );
    requests.finish(Ok(PauseReport {
        timings: None,
        tracking: None,
        elapsed_micros: 17,
    }));
    let mut next = std::pin::pin!(requests.request(PauseOperation::Access));
    assert!(
        next.as_mut().poll(&mut cx).is_pending(),
        "old completion cannot finish a new request"
    );
    let mut server = std::pin::pin!(requests.wait());
    assert!(server.as_mut().poll(&mut cx).is_ready());
    drop(availability);
    assert_eq!(
        next.as_mut().poll(&mut cx),
        Poll::Ready(Err(PauseError::Interrupted))
    );
}

#[test]
fn hardware_failure_is_returned_without_becoming_success_or_leaking_into_next_epoch() {
    let requests = Requests::new();
    let mut cx = Context::from_waker(Waker::noop());
    for reason in [
        PauseError::MacStop,
        PauseError::RxBusy,
        PauseError::RxPause,
        PauseError::IrqPause,
        PauseError::RxResume,
        PauseError::IrqResume,
        PauseError::RegisterReclaim,
        PauseError::PhyAdmission,
        PauseError::PhyRelease,
        PauseError::RegisterRepublish,
        PauseError::PhyTracking,
        PauseError::MacRestoration,
        PauseError::ReceivePolicyChanged,
    ] {
        let availability = requests.open();
        let mut request = std::pin::pin!(requests.request(PauseOperation::Access));
        assert!(request.as_mut().poll(&mut cx).is_pending());
        let mut server = std::pin::pin!(requests.wait());
        assert!(server.as_mut().poll(&mut cx).is_ready());
        requests.finish(Err(reason));
        assert_eq!(request.as_mut().poll(&mut cx), Poll::Ready(Err(reason)));
        drop(availability);
    }
}

#[test]
fn maintenance_request_is_not_downgraded() {
    for operation in [PauseOperation::Tracking, PauseOperation::Calibration] {
        let requests = Requests::new();
        let _availability = requests.open();
        let mut cx = Context::from_waker(Waker::noop());
        let mut request = std::pin::pin!(requests.request(operation));
        assert!(request.as_mut().poll(&mut cx).is_pending());
        let mut received = std::pin::pin!(requests.wait());
        assert_eq!(received.as_mut().poll(&mut cx), Poll::Ready(operation));
        requests.finish(Err(PauseError::PhyTracking));
        assert_eq!(
            request.as_mut().poll(&mut cx),
            Poll::Ready(Err(PauseError::PhyTracking))
        );
    }
}

#[test]
fn automatic_observation_notifications_survive_an_inflight_operation_and_epoch_close_disables_service()
 {
    use oer_esp32s31_phy::tracking::{maintenance::Operation, service::Config};
    let requests = Requests::new();
    let epoch = requests.open();
    let config = Config::new(core::num::NonZeroU64::new(1000).unwrap());
    requests.automatic.configure(Some(config));
    assert_eq!(requests.automatic.snapshot(), (Some(config), true));
    assert!(requests.automatic.try_begin(config, Operation::Temperature));
    assert_eq!(requests.automatic.snapshot(), (Some(config), false));
    requests.automatic.configure(None);
    assert_eq!(requests.automatic.snapshot(), (None, false));
    assert_eq!(
        requests.automatic.status(),
        pause_request::TrackingStatus::Pending(Operation::Temperature)
    );
    requests.automatic.notify();
    requests.automatic.completed(
        Operation::Temperature,
        5,
        Some(
            oer_esp32s31_phy::tracking::parameters::PhyParamTrackingOutcome {
                clients: oer_esp32s31_phy::tracking::parameters::PhyParamTrackRequest::new(
                    true, false,
                ),
                tracking_inhibited: false,
                calibration: Default::default(),
            },
        ),
    );
    assert_eq!(requests.automatic.snapshot(), (None, true));
    assert_eq!(requests.automatic.measurements().operations[0], 1);
    assert!(!requests.automatic.measurements().invalid);
    drop(epoch);
    assert_eq!(requests.automatic.snapshot(), (None, false));
    assert_eq!(requests.automatic.measurements().operations[0], 1);
}

#[test]
fn automatic_selection_checks_current_configuration_and_epoch_end_releases_pending() {
    use oer_esp32s31_phy::tracking::{maintenance::Operation, service::Config};
    let requests = Requests::new();
    let epoch = requests.open();
    let config = Config::new(core::num::NonZeroU64::new(1000).unwrap());
    requests.automatic.configure(Some(config));
    requests.automatic.configure(None);
    assert!(!requests.automatic.try_begin(config, Operation::Temperature));
    requests.automatic.configure(Some(config));
    assert!(requests.automatic.try_begin(config, Operation::Temperature));
    drop(epoch); // Stop wins before physical maintenance starts.
    assert_eq!(
        requests.automatic.status(),
        pause_request::TrackingStatus::Disabled
    );
    let _next_epoch = requests.open();
    requests.automatic.configure(Some(config));
    assert!(requests.automatic.try_begin(config, Operation::Temperature));
    requests
        .automatic
        .report(pause_request::TrackingStatus::Failed(
            PauseError::PhyAdmission,
        ));
    assert_eq!(requests.automatic.snapshot(), (None, false));
    assert!(!requests.automatic.try_begin(config, Operation::Temperature));
    requests.automatic.configure(None);
    assert_eq!(
        requests.automatic.status(),
        pause_request::TrackingStatus::Failed(PauseError::PhyAdmission)
    );
    assert!(requests.automatic.measurements().failed);
}
