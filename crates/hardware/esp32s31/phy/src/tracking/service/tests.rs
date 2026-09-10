use super::*;
use crate::tracking::temperature::{Acquisition, Observation};
use crate::{PhyConfig, PhyState, RegisteredPhyState, state::client::PhyClientState};

fn snapshot() -> Inspection {
    let registered =
        RegisteredPhyState::from_wrapper_test_model(PhyState::new(PhyConfig::production()));
    let mut result = Inspection::registered(
        &registered,
        PhyClientState::for_registered_epoch(1000).snapshot(),
        0,
    )
    .unwrap();
    // Pure service input: keep the real decision shapes, with an explicitly
    // active test class and a dated sensor observation.
    result.wifi = Some(super::super::inspection::ClassInspection {
        power: super::super::power::PhyTxPowerTrackingDecision {
            bounded_temperature: 0,
            threshold: 2,
            gain_base: 0,
            recomputed: false,
            update_required: false,
        },
        calibration: None,
    });
    result.temperature = Observation {
        value: 20,
        acquisition: Acquisition::Window {
            started: 100,
            completed: 110,
        },
    };
    result
}
fn config() -> Config {
    Config::new(NonZeroU64::new(1000).unwrap())
}

#[test]
fn unknown_or_stale_temperature_requires_observation_before_any_calibration() {
    let mut input = snapshot();
    input.wifi.as_mut().unwrap().power.update_required = true;
    input.temperature.acquisition = Acquisition::Undated;
    assert_eq!(
        config().inspect(input, 200, false),
        Demand::Run(Operation::Temperature)
    );
    input.temperature.acquisition = Acquisition::Window {
        started: 100,
        completed: 110,
    };
    assert_eq!(
        config().inspect(input, 1101, false),
        Demand::Run(Operation::Temperature)
    );
    assert_eq!(
        config().inspect(input, 1100, false),
        Demand::Run(Operation::WifiPower)
    );
}
#[test]
fn unchanged_temperature_parks_until_deadline_and_external_event_requests_observation() {
    let input = snapshot();
    assert_eq!(config().inspect(input, 200, false), Demand::At(1101));
    assert_eq!(
        config().inspect(input, 200, true),
        Demand::Run(Operation::Temperature)
    );
    assert_eq!(
        config().inspect(input, 109, false),
        Demand::Suspended(Suspension::Clock)
    );
}
#[test]
fn impossible_observation_cadence_suspends_instead_of_spinning() {
    let mut input = snapshot();
    input.temperature.acquisition = Acquisition::Window {
        started: 100,
        completed: 1100,
    };
    assert_eq!(
        config().inspect(input, 1200, true),
        Demand::Suspended(Suspension::SampleOverrun)
    );
    input.bluetooth_ieee802154 = input.wifi;
    assert_eq!(
        config().inspect(input, 1200, false),
        Demand::Suspended(Suspension::SharedRadio)
    );
}
