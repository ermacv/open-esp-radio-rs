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

#[test]
fn selected_observation_does_not_acknowledge_periodic_calibration() {
    use crate::tracking::{
        maintenance::Operation,
        parameters::{PhyParamTrackingAction as A, PhyParamTrackingCompletion as C},
    };
    let owner = owner();
    let before = owner.client_snapshot();
    let Evaluation::Pending { mut pending, .. } = owner
        .evaluate(
            WifiPhyMaintenanceRequest::Operation(Operation::Temperature),
            &mut Clock(100),
        )
        .unwrap_or_else(|_| panic!("selection failed"))
    else {
        panic!("explicit observation was gated by the periodic deadline")
    };
    assert_eq!(pending.snapshot(), before);
    pending.advance(C::EnteredCritical).unwrap();
    assert_eq!(pending.action(), A::TemperatureRead);
    // Host-model completion cannot mint a registered hardware epoch.
    pending.advance(C::TemperatureRead).unwrap();
    pending.advance(C::ExitedCritical).unwrap();
    let clients = pending
        .into_owner()
        .unwrap_or_else(|_| panic!("complete model retained owner"));
    assert_eq!(clients.snapshot(), before);
}

#[test]
fn automatic_selection_rechecks_sample_age_after_waiting_for_admission() {
    use crate::tracking::maintenance::Operation;
    let mut owner = owner();
    let mut state = PhyState::new(PhyConfig::production());
    state.apply_observed_temperature_outcome(
        crate::analog::temperature::PhyTemperatureOutcome {
            temperature: 50,
            sensor_index: 2,
            next_dac: 15,
        },
        Some(100),
        Some(110),
    );
    owner.registered = RegisteredPhyState::from_wrapper_test_model(state);
    let before = owner.client_snapshot();
    let Evaluation::Idle(owner) = owner
        .evaluate(
            WifiPhyMaintenanceRequest::ObservedOperation {
                operation: Operation::CommonCalibration,
                maximum_age_micros: 1000,
            },
            &mut Clock(1101),
        )
        .unwrap_or_else(|_| panic!("clock failed"))
    else {
        panic!("stale admission started hardware")
    };
    assert_eq!(owner.client_snapshot(), before);
}

#[test]
fn explicit_rfpll_forces_measurement_without_enabling_periodic_work() {
    use crate::tracking::{
        parameters::{PhyParamTrackingAction as A, PhyParamTrackingCompletion as C},
        rfpll::thermal,
    };
    let owner = owner();
    let before = owner.client_snapshot();
    let policy = owner.registered.tracking_policy();
    assert!(!policy.rfpll_cap_tracking_enabled);
    let selected = WifiPhyMaintenanceRequest::MeasureRfpll.policy(&owner.registered);
    assert_eq!(selected.rfpll_cap_tracking_threshold, Some(0));
    assert!(!selected.rfpll_cap_tracking_enabled);
    assert_eq!(owner.registered.tracking_policy(), policy);
    let Evaluation::Pending {
        registered: _,
        mut pending,
    } = owner
        .evaluate(WifiPhyMaintenanceRequest::MeasureRfpll, &mut Clock(100))
        .unwrap_or_else(|_| panic!("selection failed"))
    else {
        panic!("explicit measurement waited for periodic deadline")
    };
    assert_eq!(pending.snapshot(), before);
    pending.advance(C::EnteredCritical).unwrap();
    assert!(matches!(pending.action(), A::RfpllCapTrack { .. }));
    // Ordinary host fixture: this does not manufacture registered access.
    let mut state = PhyState::new(PhyConfig::production());
    let child = pending.begin_rfpll_cap_tracking(&mut state).unwrap();
    assert_eq!(child.action(), thermal::Action::SelectSoftwareControl);
    assert!(
        child.commit().is_err(),
        "measurement requires physical completions"
    );
    assert!(
        pending.into_owner().is_err(),
        "unfinished measurement released owner"
    );
}

#[test]
fn observed_rfpll_requires_a_completed_recent_acquisition_without_changing_policy() {
    use crate::tracking::{
        maintenance::Operation,
        parameters::{PhyParamTrackingAction as A, PhyParamTrackingCompletion as C},
    };
    for (acquisition, now, expected) in [
        (None, 1100, false),
        (Some((100, 110)), 1100, true),
        (Some((100, 110)), 1101, false),
        (Some((100, 110)), 109, false),
        (Some((110, 100)), 1100, false),
    ] {
        let mut owner = owner();
        let mut state = PhyState::new(PhyConfig::production());
        state.apply_observed_temperature_outcome(
            crate::analog::temperature::PhyTemperatureOutcome {
                temperature: 50,
                sensor_index: 2,
                next_dac: 15,
            },
            acquisition.map(|(start, _)| start),
            acquisition.map(|(_, end)| end),
        );
        owner.registered = RegisteredPhyState::from_wrapper_test_model(state);
        let request = WifiPhyMaintenanceRequest::ObservedOperation {
            operation: Operation::Rfpll,
            maximum_age_micros: 1000,
        };
        assert!(!request.policy(&owner.registered).rfpll_cap_tracking_enabled);
        assert_eq!(
            request
                .policy(&owner.registered)
                .rfpll_cap_tracking_threshold,
            None
        );
        let before = owner.client_snapshot();
        match owner
            .evaluate(request, &mut Clock(now))
            .unwrap_or_else(|_| panic!("clock rejected"))
        {
            Evaluation::Idle(owner) => {
                assert!(!expected);
                assert_eq!(owner.client_snapshot(), before);
            }
            Evaluation::Pending { mut pending, .. } => {
                assert!(expected);
                assert_eq!(pending.snapshot(), before);
                pending.advance(C::EnteredCritical).unwrap();
                assert!(matches!(pending.action(), A::RfpllCapTrack { .. }));
            }
        }
    }
}
