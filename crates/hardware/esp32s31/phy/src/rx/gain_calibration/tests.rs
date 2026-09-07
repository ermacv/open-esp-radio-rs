use super::*;
use crate::calibration::estimator::{PhyDcIqEstimate, PhyDcIqEstimateOutcome};

const RADIO: PhyRxDcCalibrationRequest = PhyRxDcCalibrationRequest {
    shared_radio: false,
    stage: PhyRxDcCalibrationStage::Radio,
    control: 0x800,
    initial: [0x100, 0x100],
    reference_delta: [0, 0],
    gain_index: 0,
    rx_saturation_detected: false,
};

fn outcome(request: PhyRxDcMinimumRequest, i: i32, q: i32, power: i32) -> PhyRxDcMinimumOutcome {
    PhyRxDcMinimumOutcome {
        request,
        estimate: PhyDcIqEstimate { i, q, power },
        attempts: 1,
        readiness_activity_edges: 0,
    }
}

#[test]
fn correction_matches_sign_and_large_low_fallbacks() {
    assert_eq!(rx_dc_calibration_correction(0, 0, 2, 0), 0);
    assert_eq!(rx_dc_calibration_correction(1, 0, 2, 0), 1);
    assert_eq!(rx_dc_calibration_correction(-1, 0, 2, 0), -1);
    assert_eq!(rx_dc_calibration_correction(0, 64, 2, 3), 8);
    assert_eq!(rx_dc_calibration_correction(24, 0, 2, 3), 3);
}

#[test]
fn gain_tables_and_reference_mode_match_vendor_rodata_and_calls() {
    assert_eq!(
        WIFI_CALIBRATION_GAIN,
        [0x40, 0x41, 0x43, 0x6e, 0x78, 0x79, 0x7b, 0x7f]
    );
    assert_eq!(
        SHARED_CALIBRATION_GAIN,
        [
            0x40, 0x41, 0x42, 0x43, 0x6e, 0x78, 0x79, 0x7b, 0x027f, 0x017f, 0x007f,
        ]
    );
    assert_eq!(bank_count(PhyRxGainDcBank::Wifi), 8);
    assert_eq!(bank_count(PhyRxGainDcBank::Shared), 11);
    assert_eq!(shared_mixer_dgain(0), 0);
    assert_eq!(shared_mixer_dgain(8), 0);
    assert_eq!(shared_mixer_dgain(9), 4);
    assert_eq!(shared_mixer_dgain(10), 7);
    assert_eq!(
        PhyRxGainDcTransition::reference_minimum_request(PhyRxGainDcBank::Shared, false).mode,
        0
    );
}

#[test]
fn shared_gain_adds_vendor_mixer_dgain_command_before_calibration() {
    let mut transition = PhyRxGainDcTransition::new(PhyRxGainDcParameters {
        crystal_selector: 0,
        pbus_rx_path_value: 0xbf,
        rx_saturation_detected: false,
    });
    transition.step = DcStep::SetGain {
        bank: PhyRxGainDcBank::Shared,
        index: 9,
        value: [0x100; 2],
        transaction: 2,
    };

    let PhyRxGainDcAction::ForcePbus { bank, transaction } = transition.action() else {
        panic!("missing final generic set-rx-gain command");
    };
    assert_eq!(bank, PhyRxGainDcBank::Shared);
    assert_eq!(transaction, PhyPbusForceTest::new(0, 2, 0xbf));
    transition
        .advance(PhyRxGainDcCompletion::PbusCompleted { bank, transaction })
        .unwrap();

    let PhyRxGainDcAction::ForcePbus { bank, transaction } = transition.action() else {
        panic!("missing shared mixer-dgain command");
    };
    assert_eq!(bank, PhyRxGainDcBank::Shared);
    assert_eq!(transaction, PhyPbusForceTest::new(0, 2, 0xbc));
    transition
        .advance(PhyRxGainDcCompletion::PbusCompleted { bank, transaction })
        .unwrap();
    assert!(matches!(
        transition.step,
        DcStep::CalibrateBaseband {
            bank: PhyRxGainDcBank::Shared,
            index: 9,
            ..
        }
    ));
}

#[test]
fn later_gain_searches_restart_from_the_first_bank_pair() {
    let mut transition = PhyRxGainDcTransition::new(PhyRxGainDcParameters {
        crystal_selector: 0,
        pbus_rx_path_value: 0,
        rx_saturation_detected: false,
    });
    transition.wifi_index_dc[0] = [0x101, 0x102];
    transition.wifi_index_dc[1] = [0x111, 0x112];
    transition.shared_index_dc[0] = [0x121, 0x122];
    transition.shared_index_dc[1] = [0x131, 0x132];

    assert_eq!(
        transition.previous(PhyRxGainDcBank::Wifi, 2),
        [0x101, 0x102]
    );
    assert_eq!(
        transition.previous(PhyRxGainDcBank::Shared, 2),
        [0x121, 0x122]
    );
}

#[test]
fn converged_radio_measurement_restores_field_after_final_pbus_values() {
    let mut transition = PhyRxDcCalibrationTransition::new(RADIO);
    transition
        .advance(PhyRxDcCalibrationCompletion::ControlRestorePrepared)
        .unwrap();
    transition
        .advance(PhyRxDcCalibrationCompletion::PbusRead {
            selector: 1,
            path: 2,
            value: 3,
        })
        .unwrap();
    for selector in [2, 3] {
        let PhyRxDcCalibrationAction::ForcePbus(transaction) = transition.action() else {
            panic!("missing setup PBus action");
        };
        assert_eq!(transaction.selector(), selector);
        transition
            .advance(PhyRxDcCalibrationCompletion::PbusForceCompleted(
                transaction,
            ))
            .unwrap();
    }
    let PhyRxDcCalibrationAction::DelayMicros {
        measurement,
        micros,
    } = transition.action()
    else {
        panic!("missing timer action");
    };
    transition
        .advance(PhyRxDcCalibrationCompletion::DelayElapsed {
            measurement,
            micros,
        })
        .unwrap();
    let request = transition.minimum_request(false);
    transition.accept_measurement(false, outcome(request, 1, -1, 20));
    for selector in [2, 3] {
        let PhyRxDcCalibrationAction::ForcePbus(transaction) = transition.action() else {
            panic!("missing cleanup PBus action");
        };
        assert_eq!(transaction.selector(), selector);
        transition
            .advance(PhyRxDcCalibrationCompletion::PbusForceCompleted(
                transaction,
            ))
            .unwrap();
    }
    assert!(matches!(
        transition.action(),
        PhyRxDcCalibrationAction::RestoreControl
    ));
    transition
        .advance(PhyRxDcCalibrationCompletion::ControlRestored)
        .unwrap();
    let PhyRxDcCalibrationAction::Complete(outcome) = transition.action() else {
        panic!("calibration did not complete");
    };
    assert!(outcome.converged);
    assert_eq!(outcome.iterations, 1);
    assert_eq!(outcome.configuration, RADIO.initial);
}

#[test]
fn child_failure_uses_initial_configuration_for_cleanup() {
    let mut transition = PhyRxDcCalibrationTransition::new(RADIO);
    transition.step = Step::Minimum {
        high: false,
        transition: PhyRxDcMinimumTransition::new(transition.minimum_request(false)),
    };
    transition.fail(PhyRxDcCalibrationFailure::Minimum(
        PhyRxDcMinimumFailure::DcIq(
            crate::calibration::estimator::PhyDcIqFailure::ReadinessTimedOut {
                request: crate::calibration::estimator::PhyDcIqEstimateRequest {
                    iteration: 0,
                    chain: 1,
                    control: RADIO.control,
                    mode: 0,
                },
                readiness_activity_edges: 0,
            },
        ),
    ));
    assert_eq!(
        transition.action(),
        PhyRxDcCalibrationAction::ForcePbus(PhyPbusForceTest::new(2, 2, RADIO.initial[0]))
    );
}

#[test]
fn minimum_outcome_shape_is_owned_not_pointer_backed() {
    let request = PhyRxDcMinimumRequest {
        measurement: 0,
        control: RADIO.control,
        mode: 0,
        rx_saturation_detected: false,
    };
    let child = PhyDcIqEstimateOutcome {
        request: crate::calibration::estimator::PhyDcIqEstimateRequest {
            iteration: 0,
            chain: 1,
            control: RADIO.control,
            mode: 0,
        },
        estimate: PhyDcIqEstimate {
            i: 1,
            q: 2,
            power: 3,
        },
        readiness_activity_edges: 0,
    };
    assert_eq!(request.measurement, child.request.iteration);
}

#[test]
fn cleanup_work_mode_pulse_uses_the_vendor_two_microsecond_delay() {
    let mut transition = PhyRxGainDcTransition::new(PhyRxGainDcParameters {
        crystal_selector: 0,
        pbus_rx_path_value: 0,
        rx_saturation_detected: false,
    });
    transition.step = DcStep::WorkModePulseDelay(DcTerminal::Complete);
    assert_eq!(
        transition.action(),
        PhyRxGainDcAction::DelayMicros {
            phase: PhyRxGainDcDelayPhase::PbusWorkModePulse,
            micros: 2,
        }
    );
    assert_eq!(
        transition.advance(PhyRxGainDcCompletion::DelayElapsed {
            phase: PhyRxGainDcDelayPhase::PbusWorkModePulse,
            micros: 1,
        }),
        Err(PhyRxGainDcTransitionError::WrongCompletion)
    );
}
