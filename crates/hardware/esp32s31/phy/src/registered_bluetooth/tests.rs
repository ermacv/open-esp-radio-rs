use super::RegisteredBluetoothPhy;
use crate::{
    PhyConfig, PhyState, RegisteredPhyState,
    state::client::{
        DEFAULT_PLL_TRACK_PERIOD_MICROS, PhyClientState, PhyModemClient, PhyPllTrackClock,
    },
    tracking::schedule::Schedule,
};

struct FixedClock(u64);

impl PhyPllTrackClock for FixedClock {
    fn now_micros(&mut self) -> u64 {
        self.0
    }
}

fn registered_phy() -> RegisteredBluetoothPhy {
    RegisteredBluetoothPhy {
        registered: RegisteredPhyState::from_wrapper_test_model(PhyState::new(
            PhyConfig::production(),
        )),
        clients: PhyClientState::for_registered_epoch(DEFAULT_PLL_TRACK_PERIOD_MICROS),
    }
}

fn settled_bluetooth(now_micros: u64) -> super::RegisteredBluetoothPhyClient {
    registered_phy()
        .acquire_phy_client(&mut FixedClock(now_micros))
        .unwrap_or_else(|_| panic!("fresh Bluetooth client acquisition must succeed"))
        .into_owner()
        .unwrap_or_else(|_| panic!("test acquisition must not require tracking"))
}

#[test]
fn bluetooth_client_acquisition_retains_registration_and_exact_role() {
    let acquisition = registered_phy()
        .acquire_phy_client(&mut FixedClock(0))
        .unwrap_or_else(|_| panic!("fresh Bluetooth client acquisition must succeed"));
    assert!(acquisition.request().is_none());

    let owner = acquisition
        .into_owner()
        .unwrap_or_else(|_| panic!("fresh timestamp must not require tracking"));
    assert!(owner.phy_state().phy_registered());
    assert!(owner.client_snapshot().contains(PhyModemClient::Bluetooth));
    assert!(!owner.client_snapshot().contains(PhyModemClient::Wifi));
    assert!(!owner.client_snapshot().contains(PhyModemClient::Ieee802154));
}

#[test]
fn due_initial_tracking_cannot_be_skipped_or_recovered_after_fail_stop() {
    let acquisition = registered_phy()
        .acquire_phy_client(&mut FixedClock(
            DEFAULT_PLL_TRACK_PERIOD_MICROS.saturating_add(1),
        ))
        .unwrap_or_else(|_| panic!("Bluetooth client acquisition must succeed"));
    assert!(acquisition.request().is_some());

    let pending = match acquisition.into_owner() {
        Ok(_) => panic!("due tracking must retain pending ownership"),
        Err(pending) => pending,
    };
    let poisoned = pending.fail();
    assert!(poisoned.phy_state().phy_registered());
    assert!(
        poisoned
            .client_snapshot()
            .contains(PhyModemClient::Bluetooth)
    );
}

#[test]
fn due_initial_tracking_uses_registered_epoch_policy() {
    let acquisition = registered_phy()
        .acquire_phy_client(&mut FixedClock(
            DEFAULT_PLL_TRACK_PERIOD_MICROS.saturating_add(1),
        ))
        .unwrap_or_else(|_| panic!("Bluetooth client acquisition must succeed"));
    let pending = match acquisition.into_owner() {
        Ok(_) => panic!("due tracking must retain the registered epoch"),
        Err(pending) => pending,
    };
    let mut tracking = pending.begin_tracking();

    assert_eq!(
        tracking.action(),
        crate::tracking::parameters::PhyParamTrackingAction::EnterCritical
    );
    tracking
        .pending
        .advance(crate::tracking::parameters::PhyParamTrackingCompletion::EnteredCritical)
        .unwrap();
    assert_eq!(
        tracking.action(),
        crate::tracking::parameters::PhyParamTrackingAction::RfpllCapTrack {
            diagnostics: crate::tracking::parameters::PhyTrackingDiagnostics::Disabled,
        }
    );
}

#[test]
fn bluetooth_inspection_is_read_only_and_selects_only_the_shared_class() {
    let owner = settled_bluetooth(0);
    let deadline = owner
        .client_snapshot()
        .next_tracking_deadline_micros()
        .unwrap()
        .unwrap();
    let inspection = owner.inspect_tracking(deadline).unwrap();
    assert!(matches!(inspection.schedule, Schedule::Due(_)));
    assert!(inspection.wifi.is_none());
    assert!(inspection.wifi_i2c.is_none());
    assert!(inspection.bluetooth_ieee802154.is_some());
    assert_eq!(
        owner
            .client_snapshot()
            .next_tracking_deadline_micros()
            .unwrap(),
        Some(deadline)
    );
}

#[test]
fn early_wake_preserves_deadline_and_due_wake_retains_bluetooth_request() {
    let mut owner = settled_bluetooth(0);
    let deadline = owner
        .client_snapshot()
        .next_tracking_deadline_micros()
        .unwrap()
        .unwrap();
    for now in [0, deadline - 1] {
        let evaluation = owner
            .evaluate_due_tracking(&mut FixedClock(now))
            .unwrap_or_else(|_| panic!("monotonic evaluation must succeed"));
        assert!(evaluation.request().is_none());
        owner = evaluation
            .into_owner()
            .unwrap_or_else(|_| panic!("early wake must not issue tracking"));
        assert_eq!(
            owner
                .client_snapshot()
                .next_tracking_deadline_micros()
                .unwrap(),
            Some(deadline)
        );
    }

    let evaluation = owner
        .evaluate_due_tracking(&mut FixedClock(deadline))
        .unwrap_or_else(|_| panic!("due evaluation must succeed"));
    let pending = match evaluation.into_owner() {
        Ok(_) => panic!("due Bluetooth tracking must retain the registered owner"),
        Err(pending) => pending,
    };
    assert!(!pending.request().wifi());
    assert!(pending.request().bluetooth_ieee802154());
}

#[test]
fn periodic_callback_issues_only_the_bluetooth_shared_class() {
    let owner = settled_bluetooth(0);
    let evaluation = owner
        .evaluate_periodic_tracking(&mut FixedClock(1))
        .unwrap_or_else(|_| panic!("periodic evaluation must succeed"));
    let pending = match evaluation.into_owner() {
        Ok(_) => panic!("an active periodic Bluetooth class must issue tracking"),
        Err(pending) => pending,
    };
    assert!(!pending.request().wifi());
    assert!(pending.request().bluetooth_ieee802154());
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
fn cancelled_bluetooth_tracking_wait_does_not_transfer_or_advance_the_owner() {
    use core::{
        future::Future,
        task::{Context, Waker},
    };
    let owner = settled_bluetooth(0);
    let before = owner.client_snapshot();
    let mut timer = TrackingTimer {
        now: 0,
        deadlines: std::vec::Vec::new(),
        pending: true,
    };
    let mut context = Context::from_waker(Waker::noop());
    {
        let mut future = std::pin::pin!(owner.wait_for_tracking_demand(&mut timer));
        assert!(future.as_mut().poll(&mut context).is_pending());
        assert!(future.as_mut().poll(&mut context).is_pending());
    }
    assert_eq!(timer.deadlines, [DEFAULT_PLL_TRACK_PERIOD_MICROS + 1]);
    assert_eq!(owner.client_snapshot(), before);
}
