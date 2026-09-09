use crate::{PhyConfig, state::client::DEFAULT_PLL_TRACK_PERIOD_MICROS};

use oer_esp32s31_hal::owner::Radio;

use super::*;

#[derive(Debug)]
struct TestPlatform;

struct FixedClock(u64);

impl PhyPllTrackClock for FixedClock {
    fn now_micros(&mut self) -> u64 {
        self.0
    }
}

fn registered_radio() -> RegisteredPhyRadio<TestPlatform> {
    let radio = Radio::claim_for_validation(TestPlatform);
    let radio = radio.assume_powered_for_validation();
    RegisteredPhyRadio {
        radio,
        phy: RegisteredPhyState::from_wrapper_test_model(PhyState::new(PhyConfig::production())),
        clients: PhyClientState::for_registered_epoch(DEFAULT_PLL_TRACK_PERIOD_MICROS),
    }
}

fn settle_acquire(
    owner: RegisteredPhyRadio<TestPlatform>,
    client: PhyModemClient,
    now_micros: u64,
) -> RegisteredPhyRadio<TestPlatform> {
    let acquired = match owner.acquire_client(client, &mut FixedClock(now_micros)) {
        Ok(acquired) => acquired,
        Err(_) => panic!("fresh acquisition must succeed"),
    };
    match acquired.into_owner() {
        Ok(owner) => owner,
        Err(_) => panic!("fresh timestamp must not request tracking"),
    }
}

#[test]
fn registered_client_acquire_release_never_separates_radio_and_phy_state() {
    let owner = registered_radio();
    assert!(owner.client_snapshot().is_empty());

    let owner = settle_acquire(owner, PhyModemClient::Ieee802154, 0);
    assert!(owner.client_snapshot().contains(PhyModemClient::Ieee802154));

    let failure = match owner.acquire_client(PhyModemClient::Ieee802154, &mut FixedClock(0)) {
        Ok(_) => panic!("duplicate client acquisition must fail"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.error(),
        PhyClientAcquireError::AlreadyAcquired(PhyModemClient::Ieee802154)
    );
    let owner = failure.into_owner();

    let released = match owner.release_client(PhyModemClient::Ieee802154) {
        Ok(released) => released,
        Err(_) => panic!("owned client must release"),
    };
    assert!(released.is_last());
    assert!(released.into_owner().client_snapshot().is_empty());
}

#[test]
fn periodic_request_retains_complete_registered_epoch_until_poison_or_success() {
    let owner = settle_acquire(registered_radio(), PhyModemClient::Ieee802154, 0);
    let evaluation = match owner.evaluate_periodic_tracking(&mut FixedClock(1)) {
        Ok(evaluation) => evaluation,
        Err(_) => panic!("monotonic callback must evaluate"),
    };
    let pending = match evaluation.into_owner() {
        Ok(_) => panic!("an active periodic class must retain the owner"),
        Err(pending) => pending,
    };
    assert!(!pending.request().wifi());
    assert!(pending.request().bluetooth_ieee802154());
    assert!(
        pending
            .client_snapshot()
            .contains(PhyModemClient::Ieee802154)
    );

    let tracking = pending.begin_tracking();
    assert_eq!(tracking.action(), PhyParamTrackingAction::EnterCritical);
    let poisoned = tracking.fail();
    assert!(poisoned.request().bluetooth_ieee802154());
    assert!(
        poisoned
            .client_snapshot()
            .contains(PhyModemClient::Ieee802154)
    );
}

#[test]
fn unrelated_wakes_do_not_start_registered_phy_tracking() {
    let mut owner = settle_acquire(registered_radio(), PhyModemClient::Wifi, 0);
    let deadline = owner
        .client_snapshot()
        .next_tracking_deadline_micros()
        .unwrap()
        .unwrap();
    for now in [0, 1, deadline - 1] {
        let evaluation = owner
            .evaluate_due_tracking(&mut FixedClock(now))
            .unwrap_or_else(|_| panic!("monotonic evaluation failed"));
        assert!(evaluation.request().is_none());
        owner = evaluation
            .into_owner()
            .unwrap_or_else(|_| panic!("early wake issued tracking"));
        assert_eq!(
            owner.client_snapshot().next_tracking_deadline_micros(),
            Ok(Some(deadline))
        );
    }
    let evaluation = owner
        .evaluate_due_tracking(&mut FixedClock(deadline))
        .unwrap_or_else(|_| panic!("deadline evaluation failed"));
    let pending = match evaluation.into_owner() {
        Ok(_) => panic!("due tracking must retain its radio epoch"),
        Err(pending) => pending,
    };
    assert!(pending.request().wifi());
    let poisoned = pending.begin_tracking().fail();
    assert!(poisoned.client_snapshot().contains(PhyModemClient::Wifi));
}

struct TrackingTimer {
    now: u64,
    deadlines: std::vec::Vec<u64>,
    pending: bool,
}

impl PhyPllTrackClock for TrackingTimer {
    fn now_micros(&mut self) -> u64 {
        self.now
    }
}

impl crate::state::client::PhyTrackingTimer for TrackingTimer {
    async fn wait_until_micros(&mut self, deadline: u64) {
        self.deadlines.push(deadline);
        if self.pending {
            core::future::pending::<()>().await;
        }
        self.now = deadline;
    }
}

#[test]
fn cancelled_tracking_wait_retains_owner_and_empty_client_set_never_arms() {
    use core::{
        future::Future,
        task::{Context, Poll, Waker},
    };
    let mut context = Context::from_waker(Waker::noop());
    let mut timer = TrackingTimer {
        now: 0,
        deadlines: std::vec::Vec::new(),
        pending: true,
    };
    let owner = settle_acquire(registered_radio(), PhyModemClient::Wifi, 0);
    let before = owner.client_snapshot();
    {
        let mut future = std::pin::pin!(owner.wait_for_tracking_demand(&mut timer));
        assert!(future.as_mut().poll(&mut context).is_pending());
        assert!(future.as_mut().poll(&mut context).is_pending());
    }
    assert_eq!(timer.deadlines, [DEFAULT_PLL_TRACK_PERIOD_MICROS + 1]);
    assert_eq!(owner.client_snapshot(), before);
    let released = owner
        .release_client(PhyModemClient::Wifi)
        .unwrap_or_else(|_| panic!("cancelled wait must retain the owner"));
    assert!(released.into_owner().client_snapshot().is_empty());
    timer.deadlines.clear();
    {
        let owner = registered_radio();
        let mut future = std::pin::pin!(owner.wait_for_tracking_demand(&mut timer));
        let Poll::Ready(Ok(evaluation)) = future.as_mut().poll(&mut context) else {
            panic!("empty client set must return without waiting");
        };
        assert!(evaluation.is_none());
    }
    assert!(timer.deadlines.is_empty());
}

#[test]
fn registered_tracking_timer_wake_reports_demand_without_starting_work() {
    use core::{
        future::Future,
        task::{Context, Poll, Waker},
    };
    let mut timer = TrackingTimer {
        now: 0,
        deadlines: std::vec::Vec::new(),
        pending: false,
    };
    let owner = settle_acquire(registered_radio(), PhyModemClient::Wifi, 0);
    let before = owner.client_snapshot();
    {
        let mut future = std::pin::pin!(owner.wait_for_tracking_demand(&mut timer));
        let mut context = Context::from_waker(Waker::noop());
        let Poll::Ready(Ok(Some(demand))) = future.as_mut().poll(&mut context) else {
            panic!("timer completion must report demand");
        };
        assert!(demand.request().wifi());
    }
    assert_eq!(owner.client_snapshot(), before);
}

#[test]
fn wifi_acquisition_after_cold_calibration_retains_owner_until_initial_tracking() {
    // Calibration can finish on either side of the first tracking deadline.
    for now in [
        DEFAULT_PLL_TRACK_PERIOD_MICROS,
        DEFAULT_PLL_TRACK_PERIOD_MICROS + 1,
    ] {
        let acquired = registered_radio()
            .acquire_client(PhyModemClient::Wifi, &mut FixedClock(now))
            .unwrap_or_else(|_| panic!("fresh Wi-Fi acquisition failed"));
        assert!(acquired.was_empty());
        match acquired.into_owner() {
            Ok(owner) => {
                assert_eq!(now, DEFAULT_PLL_TRACK_PERIOD_MICROS);
                assert!(owner.client_snapshot().contains(PhyModemClient::Wifi));
            }
            Err(pending) => {
                assert_eq!(now, DEFAULT_PLL_TRACK_PERIOD_MICROS + 1);
                assert!(pending.request().wifi());
                assert!(!pending.request().bluetooth_ieee802154());
                let tracking = pending.begin_tracking();
                assert_eq!(tracking.action(), PhyParamTrackingAction::EnterCritical);
                // An unsettled hardware request must never return a ready radio.
                let poisoned = tracking.fail();
                assert!(poisoned.client_snapshot().contains(PhyModemClient::Wifi));
            }
        }
    }
}
