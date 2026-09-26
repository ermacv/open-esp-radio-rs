use super::*;
use crate::registered_route::PhyDomain;
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
    let clients = PhyClientState::without_registration(DEFAULT_PLL_TRACK_PERIOD_MICROS)
        .acquire(PhyModemClient::Wifi, &mut Clock(0))
        .unwrap_or_else(|_| panic!("acquisition failed"))
        .into_owner()
        .unwrap_or_else(|_| panic!("unexpected initial tracking"));
    RegisteredWifiPhy {
        domain: PhyDomain::new(
            RegisteredPhyState::from_wrapper_test_model(PhyState::new(PhyConfig::production())),
            clients,
        ),
    }
}

type PoweredRadio = oer_esp32s31_hal::owner::Radio<(), oer_esp32s31_hal::owner::state::Powered>;

/// A powered radio and a Wi-Fi owner registered in its current epoch.
fn registered_radio(acquire_wifi: bool) -> (RegisteredWifiPhy, PoweredRadio) {
    use oer_esp32s31_hal::owner::PhyInitializationAccess;
    let mut radio =
        oer_esp32s31_hal::owner::Radio::claim_for_validation(()).assume_powered_for_validation();
    let epoch = radio.phy_hal_mut().begin_registration_epoch();
    let mut clients = PhyClientState::for_registration(DEFAULT_PLL_TRACK_PERIOD_MICROS, epoch);
    if acquire_wifi {
        clients = clients
            .acquire(PhyModemClient::Wifi, &mut Clock(0))
            .unwrap_or_else(|_| panic!("acquisition failed"))
            .into_owner()
            .unwrap_or_else(|_| panic!("unexpected initial tracking"));
    }
    let owner = RegisteredWifiPhy {
        domain: PhyDomain::new(
            RegisteredPhyState::from_wrapper_test_model(PhyState::new(PhyConfig::production())),
            clients,
        ),
    };
    (owner, radio)
}

#[test]
fn wifi_release_reunites_only_after_preserving_the_last_client_fact() {
    let (owner, radio) = registered_radio(true);
    let release = owner
        .release_wifi_client(radio)
        .unwrap_or_else(|_| panic!("registered Wi-Fi client must release into its radio"));
    let crate::RegisteredPhyClientReleaseDisposition::Last(idle) = release.into_disposition()
    else {
        panic!("last Wi-Fi release must mint the powered-idle owner");
    };
    assert!(idle.client_snapshot().is_empty());
}

#[test]
fn missing_wifi_release_returns_the_exact_owner_and_radio() {
    let (owner, radio) = registered_radio(false);
    let failure = owner
        .release_wifi_client(radio)
        .err()
        .expect("missing Wi-Fi client must fail");
    assert_eq!(
        failure.error(),
        crate::RegisteredWifiPhyClientReleaseError::Client(
            crate::state::client::PhyClientReleaseError::NotAcquired(PhyModemClient::Wifi)
        )
    );
    let (owner, _radio) = failure.into_parts();
    assert!(owner.client_snapshot().is_empty());
}

#[test]
fn wifi_release_rejects_a_radio_registered_again_without_releasing_the_client() {
    use oer_esp32s31_hal::owner::PhyInitializationAccess;
    let (owner, mut radio) = registered_radio(true);
    radio.phy_hal_mut().begin_registration_epoch();
    let failure = owner
        .release_wifi_client(radio)
        .err()
        .expect("a later registration must invalidate the detached owner");
    assert_eq!(
        failure.error(),
        crate::RegisteredWifiPhyClientReleaseError::EpochMismatch
    );
    let (owner, _radio) = failure.into_parts();
    assert!(owner.client_snapshot().contains(PhyModemClient::Wifi));
}

#[test]
fn wifi_release_rejects_a_model_owner_without_registration() {
    let owner = owner();
    let radio =
        oer_esp32s31_hal::owner::Radio::claim_for_validation(()).assume_powered_for_validation();
    let failure = owner
        .release_wifi_client(radio)
        .err()
        .expect("a host model owner cannot describe any radio");
    assert_eq!(
        failure.error(),
        crate::RegisteredWifiPhyClientReleaseError::EpochMismatch
    );
}

#[test]
fn early_maintenance_preserves_owner_and_deadline() {
    let owner = owner();
    let before = owner.client_snapshot();
    match owner
        .evaluate(
            WifiPhyMaintenanceRequest::Track,
            &mut Clock(DEFAULT_PLL_TRACK_PERIOD_MICROS),
            true,
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
            true,
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
    let before = owner.domain.registered.tracking_policy();
    let requested = WifiPhyMaintenanceRequest::Calibrate.policy(&owner.domain.registered);
    assert_eq!(requested.calibration_tracking_threshold, Some(0));
    assert_eq!(
        WifiPhyMaintenanceRequest::Track.policy(&owner.domain.registered),
        before
    );
    assert_eq!(owner.domain.registered.tracking_policy(), before);
    let snapshot = owner.client_snapshot();
    match owner
        .evaluate(
            WifiPhyMaintenanceRequest::Calibrate,
            &mut Clock(DEFAULT_PLL_TRACK_PERIOD_MICROS),
            true,
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
            true,
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
    owner.domain.registered = RegisteredPhyState::from_wrapper_test_model(state);
    let before = owner.client_snapshot();
    let Evaluation::Idle(owner) = owner
        .evaluate(
            WifiPhyMaintenanceRequest::ObservedOperation {
                operation: Operation::CommonCalibration,
                maximum_age_micros: 1000,
            },
            &mut Clock(1101),
            true,
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
    let mut owner = owner();
    let mut state = PhyState::new(PhyConfig::production());
    state.apply_observed_temperature_outcome(
        crate::analog::temperature::PhyTemperatureOutcome {
            temperature: 20,
            sensor_index: 2,
            next_dac: 15,
        },
        Some(90),
        Some(95),
    );
    owner.domain.registered = RegisteredPhyState::from_wrapper_test_model(state);
    let request = WifiPhyMaintenanceRequest::MeasureRfpll {
        maximum_age_micros: 1000,
    };
    let before = owner.client_snapshot();
    let policy = owner.domain.registered.tracking_policy();
    assert!(policy.rfpll_cap_tracking_enabled);
    let selected = request.policy(&owner.domain.registered);
    assert_eq!(selected.rfpll_cap_tracking_threshold, Some(0));
    assert!(selected.rfpll_cap_tracking_enabled);
    assert_eq!(owner.domain.registered.tracking_policy(), policy);
    let Evaluation::Pending {
        registered: _,
        mut pending,
    } = owner
        .evaluate(request, &mut Clock(100), true)
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
    assert_eq!(
        child.action(),
        thermal::Action::SetGrantProtect { enabled: true }
    );
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
    for forced in [false, true] {
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
            owner.domain.registered = RegisteredPhyState::from_wrapper_test_model(state);
            let request = if forced {
                WifiPhyMaintenanceRequest::MeasureRfpll {
                    maximum_age_micros: 1000,
                }
            } else {
                WifiPhyMaintenanceRequest::ObservedOperation {
                    operation: Operation::Rfpll,
                    maximum_age_micros: 1000,
                }
            };
            assert!(
                request
                    .policy(&owner.domain.registered)
                    .rfpll_cap_tracking_enabled
            );
            assert_eq!(
                request
                    .policy(&owner.domain.registered)
                    .rfpll_cap_tracking_threshold,
                forced.then_some(0)
            );
            let before = owner.client_snapshot();
            match owner
                .evaluate(request, &mut Clock(now), true)
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
}

#[test]
fn stale_registration_is_rejected_before_any_selection() {
    let owner = owner();
    let before = owner.client_snapshot();
    let Err((owner, rejection)) = owner.evaluate(
        WifiPhyMaintenanceRequest::Calibrate,
        &mut Clock(DEFAULT_PLL_TRACK_PERIOD_MICROS + 1),
        false,
    ) else {
        panic!("a stale registration must not select maintenance work");
    };
    assert_eq!(rejection, Rejection::EpochMismatch);
    assert_eq!(owner.client_snapshot(), before);
}
