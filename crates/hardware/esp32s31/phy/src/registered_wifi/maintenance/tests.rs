use super::*;
use crate::{
    PhyConfig, PhyState,
    state::client::{DEFAULT_PLL_TRACK_PERIOD_MICROS, PhyClientState, PhyModemClient},
};
struct Clock(u64);
impl PhyPllTrackClock for Clock {
    fn now_micros(&mut self) -> u64 {
        self.0
    }
}
fn owner() -> RegisteredWifiPhy {
    let clients = PhyClientState::for_registered_epoch(DEFAULT_PLL_TRACK_PERIOD_MICROS)
        .acquire(PhyModemClient::Wifi, &mut Clock(0))
        .unwrap_or_else(|_| panic!("acquisition failed"))
        .into_owner()
        .unwrap_or_else(|_| panic!("unexpected initial tracking"));
    RegisteredWifiPhy {
        registered: RegisteredPhyState::from_wrapper_test_model(PhyState::new(
            PhyConfig::production(),
        )),
        clients,
    }
}
#[test]
fn early_maintenance_preserves_owner_and_deadline() {
    let owner = owner();
    let before = owner.client_snapshot();
    match owner
        .evaluate(
            WifiPhyMaintenanceRequest::Track,
            &mut Clock(DEFAULT_PLL_TRACK_PERIOD_MICROS),
        )
        .unwrap_or_else(|_| panic!("evaluation failed"))
    {
        Evaluation::Idle(owner) => assert_eq!(owner.client_snapshot(), before),
        Evaluation::Pending { .. } => panic!("early maintenance must not touch hardware"),
    }
}
#[test]
fn elapsed_deadline_retains_wifi_until_tracking_completes() {
    match owner()
        .evaluate(
            WifiPhyMaintenanceRequest::Track,
            &mut Clock(DEFAULT_PLL_TRACK_PERIOD_MICROS + 1),
        )
        .unwrap_or_else(|_| panic!("evaluation failed"))
    {
        Evaluation::Idle(_) => panic!("due tracking skipped"),
        Evaluation::Pending {
            registered,
            pending,
        } => {
            assert_eq!(registered.state().config(), &PhyConfig::production());
            let pending = pending
                .into_owner()
                .err()
                .expect("unfinished tracking returned its owner");
            let poisoned = pending.fail();
            assert!(poisoned.request().wifi());
            assert!(!poisoned.request().bluetooth_ieee802154());
            assert!(poisoned.snapshot().contains(PhyModemClient::Wifi));
        }
    }
}

#[test]
fn explicit_calibration_preserves_registered_policy_and_real_deadline() {
    let owner = owner();
    let before = owner.registered.tracking_policy();
    let requested = WifiPhyMaintenanceRequest::Calibrate.policy(&owner.registered);
    assert_eq!(requested.calibration_tracking_threshold, Some(0));
    assert_eq!(
        WifiPhyMaintenanceRequest::Track.policy(&owner.registered),
        before
    );
    assert_eq!(owner.registered.tracking_policy(), before);
    let snapshot = owner.client_snapshot();
    match owner
        .evaluate(
            WifiPhyMaintenanceRequest::Calibrate,
            &mut Clock(DEFAULT_PLL_TRACK_PERIOD_MICROS),
        )
        .unwrap_or_else(|_| panic!("evaluation failed"))
    {
        Evaluation::Idle(owner) => assert_eq!(owner.client_snapshot(), snapshot),
        Evaluation::Pending { .. } => panic!("explicit calibration must respect the real deadline"),
    }
}
