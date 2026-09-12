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

#[test]
fn independent_wifi_radio_measurement_waits_for_low_level_publication() {
    let parameters = PhyRxGainDcParameters {
        crystal_selector: 0,
        pbus_rx_path_value: 0,
        rx_saturation_detected: false,
    };
    let mut transition = PhyRxGainDcTransition::new(parameters);
    transition.store_calibration(
        PhyRxGainDcBank::Wifi,
        0,
        PhyRxDcCalibrationOutcome {
            request: RADIO,
            configuration: [0x123, 0x145],
            iterations: 1,
            converged: true,
            readiness_activity_edges: 0,
        },
    );
    let transaction = PhyPbusForceTest::new(1, 2, 0);
    assert_eq!(
        transition.action(),
        PhyRxGainDcAction::ForcePbus {
            bank: PhyRxGainDcBank::Wifi,
            transaction
        }
    );
    let mut failed = transition;
    failed
        .advance(PhyRxGainDcCompletion::PbusTimedOut {
            bank: PhyRxGainDcBank::Wifi,
            transaction,
        })
        .unwrap();
    assert!(matches!(
        failed.step,
        DcStep::ClockOff(_, DcTerminal::Failed(PhyRxGainDcFailure::Pbus { .. }))
    ));
    assert!(
        transition
            .advance(PhyRxGainDcCompletion::PbusCompleted {
                bank: PhyRxGainDcBank::Shared,
                transaction
            })
            .is_err()
    );
    transition
        .advance(PhyRxGainDcCompletion::PbusCompleted {
            bank: PhyRxGainDcBank::Wifi,
            transaction,
        })
        .unwrap();
    assert!(matches!(transition.step, DcStep::CalibrateWifiRadio(_)));
}

#[test]
fn every_fallible_dc_child_rejects_without_changing_accumulated_results() {
    use crate::calibration::estimator::{PhyDcIqAction, PhyDcIqCompletion};
    let pll = RfpllFrequencyTransition::channel(RfpllFrequencyRequest {
        crystal_selector: 0,
        frequency_code: 0x9b4,
        offset: 0,
    });
    let RfpllFrequencyAction::StartChannelSwitch {
        frequency_index,
        crystal_selector,
    } = pll.action()
    else {
        panic!("channel start");
    };
    let pll_valid = PhyRxGainDcCompletion::Rfpll(RfpllFrequencyCompletion::ChannelSwitchStarted {
        frequency_index,
        crystal_selector,
    });
    let pll_invalid = PhyRxGainDcCompletion::Rfpll(RfpllFrequencyCompletion::DelayElapsed(0));
    let i2c = MaskedI2cWriteTransition::new(analog_registers::SHARED_RX_GAIN_CALIBRATION_ENABLE, 0);
    let MaskedI2cWriteAction::ReadByte { address } = i2c.action() else {
        panic!("I2C read");
    };
    let i2c_valid = PhyRxGainDcCompletion::I2c(MaskedI2cWriteCompletion::I2cReadCompleted {
        address,
        value: 0x5a,
    });
    let i2c_invalid =
        PhyRxGainDcCompletion::I2c(MaskedI2cWriteCompletion::I2cWriteCompleted { address });
    let minimum = PhyRxDcMinimumTransition::new(PhyRxGainDcTransition::reference_minimum_request(
        PhyRxGainDcBank::Shared,
        false,
    ));
    let PhyRxDcMinimumAction::DcIq(PhyDcIqAction::Configure(request)) = minimum.action() else {
        panic!("estimator configure");
    };
    let minimum_valid = PhyRxGainDcCompletion::Minimum(PhyRxDcMinimumCompletion::DcIq(
        PhyDcIqCompletion::Configured(request),
    ));
    let minimum_invalid = PhyRxGainDcCompletion::Minimum(PhyRxDcMinimumCompletion::DcIq(
        PhyDcIqCompletion::ReadinessTimedOut(request),
    ));
    let calibration = PhyRxDcCalibrationTransition::new(RADIO);
    let calibration_valid =
        PhyRxGainDcCompletion::Calibration(PhyRxDcCalibrationCompletion::ControlRestorePrepared);
    let calibration_invalid =
        PhyRxGainDcCompletion::Calibration(PhyRxDcCalibrationCompletion::ControlRestored);
    let mut nested = calibration;
    nested.step = Step::Minimum {
        high: false,
        transition: minimum,
    };
    let nested_valid = PhyRxGainDcCompletion::Calibration(PhyRxDcCalibrationCompletion::Minimum(
        PhyRxDcMinimumCompletion::DcIq(PhyDcIqCompletion::Configured(request)),
    ));
    let nested_invalid = PhyRxGainDcCompletion::Calibration(PhyRxDcCalibrationCompletion::Minimum(
        PhyRxDcMinimumCompletion::DcIq(PhyDcIqCompletion::ReadinessTimedOut(request)),
    ));
    for (step, valid, invalid) in [
        (
            DcStep::FineCalibration {
                index: 2,
                transition: nested,
            },
            nested_valid,
            nested_invalid,
        ),
        (
            DcStep::CalibrateBaseband {
                bank: PhyRxGainDcBank::Wifi,
                index: 1,
                transition: nested,
            },
            nested_valid,
            nested_invalid,
        ),
        (
            DcStep::CalibrateWifiRadio(nested),
            nested_valid,
            nested_invalid,
        ),
        (DcStep::Rfpll(pll), pll_valid, pll_invalid),
        (DcStep::WifiRfpll(pll), pll_valid, pll_invalid),
        (DcStep::SharedI2c(i2c), i2c_valid, i2c_invalid),
        (DcStep::SharedRestoreI2c(i2c), i2c_valid, i2c_invalid),
        (
            DcStep::ReferenceMinimum {
                bank: PhyRxGainDcBank::Shared,
                high: false,
                transition: minimum,
            },
            minimum_valid,
            minimum_invalid,
        ),
        (
            DcStep::FineCalibration {
                index: 2,
                transition: calibration,
            },
            calibration_valid,
            calibration_invalid,
        ),
        (
            DcStep::CalibrateBaseband {
                bank: PhyRxGainDcBank::Wifi,
                index: 1,
                transition: calibration,
            },
            calibration_valid,
            calibration_invalid,
        ),
        (
            DcStep::CalibrateWifiRadio(calibration),
            calibration_valid,
            calibration_invalid,
        ),
    ] {
        let mut dc = PhyRxGainDcTransition::new(PhyRxGainDcParameters {
            crystal_selector: 0,
            pbus_rx_path_value: 0xbf,
            rx_saturation_detected: false,
        });
        dc.step = step;
        dc.wifi_index_dc = [[0x101, 0x102]; 8];
        dc.shared_index_dc = [[0x121, 0x122]; 11];
        dc.rxbb_dc_adjustments = [[3, 7]; 6];
        assert_eq!(
            dc.lower_external(),
            PhyRxGainDcExternalBinding::lower(dc.action())
        );
        let before = dc;
        assert_eq!(
            dc.advance(invalid),
            Err(PhyRxGainDcTransitionError::WrongCompletion)
        );
        assert_eq!(dc, before);
        dc.advance(valid).unwrap();
        assert_eq!(
            dc.lower_external(),
            PhyRxGainDcExternalBinding::lower(dc.action())
        );
        let accepted = dc;
        // Replaying a formerly valid completion must preserve the advanced
        // nested cursor as well as the already accumulated coefficient banks.
        assert_eq!(
            dc.advance(valid),
            Err(PhyRxGainDcTransitionError::WrongCompletion)
        );
        assert_eq!(dc, accepted);
    }
}

#[test]
fn borrowed_minimum_matches_stepwise_parent_and_preserves_cleanup() {
    use crate::calibration::estimator::{
        PhyDcIqAccumulatorSnapshot, PhyDcIqAction, PhyDcIqCompletion, PhyDcIqReadinessSnapshot,
    };
    for timeout in [false, true] {
        for high in [false, true] {
            let minimum = PhyRxDcMinimumTransition::new(
                PhyRxGainDcTransition::reference_minimum_request(PhyRxGainDcBank::Shared, high),
            );
            let mut calibration = PhyRxDcCalibrationTransition::new(RADIO);
            calibration.step = Step::Minimum {
                high,
                transition: minimum,
            };
            for step in [
                DcStep::ReferenceMinimum {
                    bank: PhyRxGainDcBank::Shared,
                    high,
                    transition: minimum,
                },
                DcStep::ReferenceMinimum {
                    bank: PhyRxGainDcBank::Wifi,
                    high,
                    transition: minimum,
                },
                DcStep::FineCalibration {
                    index: 2,
                    transition: calibration,
                },
                DcStep::CalibrateBaseband {
                    bank: PhyRxGainDcBank::Wifi,
                    index: 1,
                    transition: calibration,
                },
                DcStep::CalibrateWifiRadio(calibration),
            ] {
                let mut fused = PhyRxGainDcTransition::new(PhyRxGainDcParameters {
                    crystal_selector: 0,
                    pbus_rx_path_value: 0xbf,
                    rx_saturation_detected: false,
                });
                fused.step = step;
                fused.wifi_index_dc = [[0x101, 0x102]; 8];
                fused.shared_index_dc = [[0x121, 0x122]; 11];
                let mut stepwise = fused;
                for _ in 0..1000 {
                    let before = fused;
                    assert!(fused.finish_minimum().is_err());
                    assert_eq!(fused, before, "early commit must not change any state");
                    let action = fused.minimum_mut().unwrap().action();
                    assert_eq!(stepwise.minimum_mut().unwrap().action(), action);
                    let PhyRxDcMinimumAction::DcIq(action) = action else {
                        panic!("pending estimator")
                    };
                    let completion = PhyRxDcMinimumCompletion::DcIq(match action {
                        PhyDcIqAction::Configure(request) => PhyDcIqCompletion::Configured(request),
                        PhyDcIqAction::SetEnable {
                            request,
                            phase,
                            enabled,
                        } => PhyDcIqCompletion::EnableSet {
                            request,
                            phase,
                            enabled,
                        },
                        PhyDcIqAction::DelayMicros {
                            request,
                            phase,
                            micros,
                        } => PhyDcIqCompletion::DelayElapsed {
                            request,
                            phase,
                            micros,
                        },
                        PhyDcIqAction::AwaitReadinessEdge { request, .. } if timeout => {
                            PhyDcIqCompletion::ReadinessTimedOut(request)
                        }
                        PhyDcIqAction::AwaitReadinessEdge { request, .. } => {
                            PhyDcIqCompletion::ReadinessObserved {
                                request,
                                snapshot: PhyDcIqReadinessSnapshot {
                                    ready: true,
                                    activity: false,
                                },
                            }
                        }
                        PhyDcIqAction::ReadAccumulators(request) => {
                            PhyDcIqCompletion::AccumulatorsRead {
                                request,
                                snapshot: PhyDcIqAccumulatorSnapshot {
                                    i: 0,
                                    q: 0,
                                    power: 0,
                                },
                            }
                        }
                        other => panic!("unexpected action {other:?}"),
                    });
                    let parent_completion = match step {
                        DcStep::ReferenceMinimum { .. } => {
                            PhyRxGainDcCompletion::Minimum(completion)
                        }
                        _ => PhyRxGainDcCompletion::Calibration(
                            PhyRxDcCalibrationCompletion::Minimum(completion),
                        ),
                    };
                    stepwise.advance(parent_completion).unwrap();
                    fused.minimum_mut().unwrap().advance(completion).unwrap();
                    if fused.minimum_mut().unwrap().terminal().is_some() {
                        fused.finish_minimum().unwrap();
                        assert_eq!(
                            fused, stepwise,
                            "terminal publication and cleanup must match"
                        );
                        assert!(fused.minimum_mut().is_none());
                        let terminal = fused;
                        assert!(fused.finish_minimum().is_err());
                        assert_eq!(fused, terminal, "replayed commit must preserve cleanup");
                        break;
                    }
                    assert_eq!(
                        fused, stepwise,
                        "intermediate actions and retained products must match"
                    );
                }
                assert!(fused.minimum_mut().is_none(), "bounded minimum must finish");
            }
        }
    }
}
