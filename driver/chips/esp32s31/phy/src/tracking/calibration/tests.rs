use super::*;
use std::vec::Vec;

const PARAMETERS: PhyCalibrationTrackingParameters = PhyCalibrationTrackingParameters {
    current_temperature: 50,
    common_reference_temperature: 20,
    wifi_reference_temperature: 20,
    bluetooth_ieee802154_reference_temperature: 20,
    threshold_override: None,
    current_channel: 11,
    channel_bandwidth: 1,
    crystal_selector: 0x31,
};

const RX_GAIN_OUTCOME: crate::rx::gain::PhyRxGainInitOutcome =
    crate::rx::gain::PhyRxGainInitOutcome {
        dc: Some(crate::rx::gain_calibration::PhyRxGainDcOutcome {
            wifi_index_dc: [[0; 2]; 8],
            wifi_dc_base: [0; 2],
            shared_index_dc: [[0; 2]; 11],
            rxbb_dc_adjustments: [[0; 2]; 6],
        }),
        generated_tables: true,
        wifi_last_index: 69,
        shared_last_index: 75,
    };

const fn channel_outcome(
    channel: u16,
    cbw: u8,
    temperature: i16,
) -> crate::channel::PhyChipChannelOutcome {
    crate::channel::PhyChipChannelOutcome {
        channel,
        frequency_mhz: 2_407 + channel * 5,
        cbw,
        init_complete: cbw != 0,
        temperature: crate::analog::temperature::PhyTemperatureOutcome {
            temperature,
            sensor_index: 2,
            next_dac: 15,
        },
    }
}

const fn tx_dc_pwdet_outcome(
    class: PhyCalibrationTrackClass,
) -> crate::tx::dc_power_detector::PhyTxDcPwdetOutcome {
    let value = match class {
        PhyCalibrationTrackClass::Wifi => 0x111,
        PhyCalibrationTrackClass::BluetoothIeee802154 => 0x222,
    };
    crate::tx::dc_power_detector::PhyTxDcPwdetOutcome {
        dco: [[value; 4]; 3],
        total_measurements: 144,
    }
}

fn completion(action: PhyCalibrationTrackingAction) -> PhyCalibrationTrackingCompletion {
    match action {
        PhyCalibrationTrackingAction::ClearPbus => {
            PhyCalibrationTrackingCompletion::PbusClearCompleted(
                PhyCalibrationPbusClearCompletion {
                    outcome: crate::analog::pbus::PhyPbusClearOutcome::Cleared,
                },
            )
        }
        PhyCalibrationTrackingAction::CalibrateDcode => {
            PhyCalibrationTrackingCompletion::DcodeCompleted(PhyCalibrationDcodeCompletion {
                result: Ok(crate::analog::dcode::PhyDcodeOutcome { codes: [7; 8] }),
            })
        }
        PhyCalibrationTrackingAction::RecalibrateRxGain => {
            PhyCalibrationTrackingCompletion::RxGainRecalibrated(PhyCalibrationRxGainCompletion {
                result: Ok(RX_GAIN_OUTCOME),
            })
        }
        PhyCalibrationTrackingAction::RestoreChipChannel { channel, cbw } => {
            PhyCalibrationTrackingCompletion::ChipChannelRestored(PhyCalibrationChannelCompletion {
                result: Ok(channel_outcome(
                    channel,
                    cbw,
                    PARAMETERS.current_temperature,
                )),
            })
        }
        PhyCalibrationTrackingAction::SetHardwareFrequencyControl { enabled } => {
            PhyCalibrationTrackingCompletion::HardwareFrequencyControlSet { enabled }
        }
        PhyCalibrationTrackingAction::ForceTxRxOff { enabled } => {
            PhyCalibrationTrackingCompletion::ForceTxRxCompleted(
                PhyCalibrationForceTxRxCompletion { enabled },
            )
        }
        PhyCalibrationTrackingAction::ConfigureBasebandChannel { cbw } => {
            PhyCalibrationTrackingCompletion::BasebandChannelConfigured { cbw }
        }
        PhyCalibrationTrackingAction::CalibrateTxDcPwdet { class } => {
            PhyCalibrationTrackingCompletion::TxDcPwdetCalibrated(
                PhyCalibrationTxDcPwdetCompletion {
                    class,
                    result: Ok(tx_dc_pwdet_outcome(class)),
                },
            )
        }
        PhyCalibrationTrackingAction::PublishWifiTxGain { channel } => {
            PhyCalibrationTrackingCompletion::TxGainPublished(PhyCalibrationTxGainCompletion {
                class: PhyCalibrationTrackClass::Wifi,
                channel: Some(channel),
            })
        }
        PhyCalibrationTrackingAction::PublishBluetoothIeee802154TxGain => {
            PhyCalibrationTrackingCompletion::TxGainPublished(PhyCalibrationTxGainCompletion {
                class: PhyCalibrationTrackClass::BluetoothIeee802154,
                channel: None,
            })
        }
        PhyCalibrationTrackingAction::EnableMacBaseband => {
            PhyCalibrationTrackingCompletion::MacBasebandEnabled
        }
        PhyCalibrationTrackingAction::RestoreTxGainCompensation => {
            PhyCalibrationTrackingCompletion::TxGainCompensationRestored
        }
        PhyCalibrationTrackingAction::Complete(_) | PhyCalibrationTrackingAction::Failed(_) => {
            panic!("terminal action")
        }
    }
}

fn run(
    request: PhyCalibrationTrackingRequest,
    parameters: PhyCalibrationTrackingParameters,
) -> Vec<PhyCalibrationTrackingAction> {
    let mut transition = PhyCalibrationTrackingTransition::new(request, parameters);
    let mut actions = Vec::new();
    for _ in 0..24 {
        let action = transition.action();
        actions.push(action);
        if matches!(
            action,
            PhyCalibrationTrackingAction::Complete(_) | PhyCalibrationTrackingAction::Failed(_)
        ) {
            return actions;
        }
        transition.advance(completion(action)).unwrap();
    }
    panic!("calibration tracking exceeded its finite path")
}

#[test]
fn wifi_inclusive_threshold_runs_common_then_wifi_and_restores_every_guard() {
    let actions = run(
        PhyCalibrationTrackingRequest {
            class: PhyCalibrationTrackClass::Wifi,
        },
        PARAMETERS,
    );
    assert_eq!(actions[0], PhyCalibrationTrackingAction::ClearPbus);
    assert_eq!(actions[1], PhyCalibrationTrackingAction::CalibrateDcode);
    assert!(
        actions.contains(&PhyCalibrationTrackingAction::RestoreChipChannel {
            channel: 11,
            cbw: 1,
        })
    );
    assert!(
        actions.contains(&PhyCalibrationTrackingAction::CalibrateTxDcPwdet {
            class: PhyCalibrationTrackClass::Wifi,
        })
    );
    assert_eq!(
        actions[actions.len() - 2],
        PhyCalibrationTrackingAction::RestoreTxGainCompensation
    );
    let PhyCalibrationTrackingAction::Complete(outcome) = actions[actions.len() - 1] else {
        panic!("missing terminal outcome")
    };
    assert!(outcome.common_updated);
    assert!(outcome.class_updated);
    assert_eq!(
        outcome.dcode,
        Some(crate::analog::dcode::PhyDcodeOutcome { codes: [7; 8] })
    );
    assert_eq!(outcome.rx_gain, Some(RX_GAIN_OUTCOME));
    assert_eq!(outcome.channel, Some(channel_outcome(11, 1, 50)));
    assert_eq!(
        outcome.tx_dc_pwdet,
        Some(tx_dc_pwdet_outcome(PhyCalibrationTrackClass::Wifi))
    );
    assert_eq!(outcome.common_reference_temperature, 50);
    assert_eq!(outcome.wifi_reference_temperature, 50);
    assert_eq!(outcome.bluetooth_ieee802154_reference_temperature, 20);
}

#[test]
fn bluetooth_class_uses_its_own_reference_and_skips_common_when_below_threshold() {
    let actions = run(
        PhyCalibrationTrackingRequest {
            class: PhyCalibrationTrackClass::BluetoothIeee802154,
        },
        PhyCalibrationTrackingParameters {
            common_reference_temperature: 50,
            wifi_reference_temperature: 0,
            bluetooth_ieee802154_reference_temperature: 20,
            ..PARAMETERS
        },
    );
    assert_eq!(
        actions[0],
        PhyCalibrationTrackingAction::SetHardwareFrequencyControl { enabled: false }
    );
    assert!(!actions.contains(&PhyCalibrationTrackingAction::CalibrateDcode));
    assert!(actions.contains(&PhyCalibrationTrackingAction::PublishBluetoothIeee802154TxGain));
}

#[test]
fn override_below_delta_skips_all_calibration_but_never_skips_final_restore() {
    let actions = run(
        PhyCalibrationTrackingRequest {
            class: PhyCalibrationTrackClass::Wifi,
        },
        PhyCalibrationTrackingParameters {
            threshold_override: Some(31),
            ..PARAMETERS
        },
    );
    assert_eq!(
        actions,
        [
            PhyCalibrationTrackingAction::RestoreTxGainCompensation,
            PhyCalibrationTrackingAction::Complete(PhyCalibrationTrackingOutcome {
                class: PhyCalibrationTrackClass::Wifi,
                threshold: 31,
                common_reference_temperature: 20,
                wifi_reference_temperature: 20,
                bluetooth_ieee802154_reference_temperature: 20,
                common_updated: false,
                class_updated: false,
                dcode: None,
                rx_gain: None,
                channel: None,
                tx_dc_pwdet: None,
            }),
        ]
    );
}

#[test]
fn wrong_completion_preserves_action_and_terminal_rejects_more_work() {
    let mut transition = PhyCalibrationTrackingTransition::new(
        PhyCalibrationTrackingRequest {
            class: PhyCalibrationTrackClass::Wifi,
        },
        PARAMETERS,
    );
    assert_eq!(
        transition.advance(PhyCalibrationTrackingCompletion::MacBasebandEnabled),
        Err(PhyCalibrationTrackingTransitionError::WrongCompletion)
    );
    assert_eq!(transition.action(), PhyCalibrationTrackingAction::ClearPbus);
    while !matches!(
        transition.action(),
        PhyCalibrationTrackingAction::Complete(_)
    ) {
        transition.advance(completion(transition.action())).unwrap();
    }
    assert_eq!(
        transition.advance(PhyCalibrationTrackingCompletion::TxGainCompensationRestored),
        Err(PhyCalibrationTrackingTransitionError::AlreadyComplete)
    );
}

#[test]
fn external_lowering_owns_only_complete_direct_hardware_leaves() {
    let direct = [
        PhyCalibrationTrackingAction::SetHardwareFrequencyControl { enabled: false },
        PhyCalibrationTrackingAction::SetHardwareFrequencyControl { enabled: true },
        PhyCalibrationTrackingAction::ConfigureBasebandChannel { cbw: 0 },
        PhyCalibrationTrackingAction::ConfigureBasebandChannel { cbw: 0x13 },
        PhyCalibrationTrackingAction::EnableMacBaseband,
        PhyCalibrationTrackingAction::RestoreTxGainCompensation,
    ];
    for action in direct {
        let binding = PhyCalibrationTrackingExternalBinding::lower(action).unwrap();
        let lowered = match &binding {
            PhyCalibrationTrackingExternalBinding::Register(binding) => binding.action(),
            PhyCalibrationTrackingExternalBinding::MacBaseband(binding) => binding.action(),
        };
        assert_eq!(lowered, action);
    }

    let unresolved = [
        PhyCalibrationTrackingAction::ClearPbus,
        PhyCalibrationTrackingAction::CalibrateDcode,
        PhyCalibrationTrackingAction::RecalibrateRxGain,
        PhyCalibrationTrackingAction::RestoreChipChannel {
            channel: 11,
            cbw: 1,
        },
        PhyCalibrationTrackingAction::ForceTxRxOff { enabled: true },
        PhyCalibrationTrackingAction::CalibrateTxDcPwdet {
            class: PhyCalibrationTrackClass::Wifi,
        },
        PhyCalibrationTrackingAction::PublishWifiTxGain { channel: 11 },
        PhyCalibrationTrackingAction::PublishBluetoothIeee802154TxGain,
        PhyCalibrationTrackingAction::Complete(PhyCalibrationTrackingOutcome {
            class: PhyCalibrationTrackClass::Wifi,
            threshold: 30,
            common_reference_temperature: 20,
            wifi_reference_temperature: 20,
            bluetooth_ieee802154_reference_temperature: 20,
            common_updated: false,
            class_updated: false,
            dcode: None,
            rx_gain: None,
            channel: None,
            tx_dc_pwdet: None,
        }),
        PhyCalibrationTrackingAction::Failed(PhyCalibrationTrackingFailure::PbusClearTimedOut(
            crate::analog::pbus::PhyPbusForceTest::new(4, 1, 0),
        )),
    ];
    for action in unresolved {
        assert_eq!(
            PhyCalibrationTrackingExternalBinding::lower(action),
            Err(PhyCalibrationTrackingBindingError::UnsupportedAction)
        );
    }
}

#[test]
fn force_txrx_parent_proof_requires_both_writes_and_timer_edges() {
    let mut transition = PhyCalibrationTrackingTransition::new(
        PhyCalibrationTrackingRequest {
            class: PhyCalibrationTrackClass::BluetoothIeee802154,
        },
        PhyCalibrationTrackingParameters {
            common_reference_temperature: 50,
            wifi_reference_temperature: 0,
            bluetooth_ieee802154_reference_temperature: 20,
            ..PARAMETERS
        },
    );
    transition
        .advance(PhyCalibrationTrackingCompletion::HardwareFrequencyControlSet { enabled: false })
        .unwrap();

    let child = transition.begin_force_txrx().unwrap();
    assert_eq!(child.parent_action(), transition.action());
    let mut child = child.commit().unwrap_err();
    loop {
        let completion = match child.lower_external() {
            Ok(crate::analog::pbus::PhyForceTxRxExternalBinding::Mmio(binding)) => {
                let crate::analog::pbus::PhyForceTxRxAction::Configure { enabled, phase } =
                    binding.action()
                else {
                    panic!("force MMIO binding lost its identity")
                };
                crate::analog::pbus::PhyForceTxRxCompletion::Configured { enabled, phase }
            }
            Ok(crate::analog::pbus::PhyForceTxRxExternalBinding::Timer(binding)) => {
                assert_eq!(binding.micros(), 1);
                binding.into_completion()
            }
            Err(_) => break,
        };
        child.advance(completion).unwrap();
    }
    let completion = child.commit().unwrap();
    transition.advance(completion).unwrap();
    assert_eq!(transition.action(), PhyCalibrationTrackingAction::ClearPbus);
}

#[test]
fn dcode_parent_proof_starts_the_existing_complete_hardware_graph() {
    let mut transition = PhyCalibrationTrackingTransition::new(
        PhyCalibrationTrackingRequest {
            class: PhyCalibrationTrackClass::Wifi,
        },
        PARAMETERS,
    );
    transition
        .advance(completion(PhyCalibrationTrackingAction::ClearPbus))
        .unwrap();

    let child = transition.begin_dcode().unwrap();
    assert!(matches!(
        child.action(),
        crate::analog::dcode::PhyDcodeAction::Rfpll(_)
    ));
    assert!(matches!(
        child.lower_external(),
        Ok(crate::analog::dcode::PhyDcodeExternalBinding::Rfpll(_))
    ));
    assert!(child.commit().is_err());
}

#[test]
fn dcode_failure_restores_gain_without_publishing_common_progress() {
    let mut transition = PhyCalibrationTrackingTransition::new(
        PhyCalibrationTrackingRequest {
            class: PhyCalibrationTrackClass::Wifi,
        },
        PARAMETERS,
    );
    transition
        .advance(completion(PhyCalibrationTrackingAction::ClearPbus))
        .unwrap();
    let failure = crate::analog::dcode::PhyDcodeFailure::Rfpll {
        calibration_index: 0,
        failure: crate::analog::rfpll::RfpllFrequencyFailure::FrequencyReadyDeadlineExceeded {
            samples: 100,
        },
    };
    transition
        .advance(PhyCalibrationTrackingCompletion::DcodeCompleted(
            PhyCalibrationDcodeCompletion {
                result: Err(failure),
            },
        ))
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyCalibrationTrackingAction::RestoreTxGainCompensation
    );
    transition
        .advance(PhyCalibrationTrackingCompletion::TxGainCompensationRestored)
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyCalibrationTrackingAction::Failed(PhyCalibrationTrackingFailure::Dcode(failure))
    );
}

#[test]
fn rx_gain_parent_proof_forces_dc_and_tables_through_complete_hardware_graph() {
    let mut transition = PhyCalibrationTrackingTransition::new(
        PhyCalibrationTrackingRequest {
            class: PhyCalibrationTrackClass::Wifi,
        },
        PARAMETERS,
    );
    for action in [
        PhyCalibrationTrackingAction::ClearPbus,
        PhyCalibrationTrackingAction::CalibrateDcode,
    ] {
        assert_eq!(transition.action(), action);
        transition.advance(completion(action)).unwrap();
    }

    let child = transition
        .begin_rx_gain_recalibration(crate::rx::gain::PhyRxGainInitParameters {
            dc_calibrated: true,
            tables_initialized: true,
            dc: crate::rx::gain_calibration::PhyRxGainDcParameters {
                crystal_selector: 0x31,
                pbus_rx_path_value: 3,
                rx_saturation_detected: false,
            },
            memory: crate::calibration::baseband::PhyRxGainMemoryParameters {
                parameter_002: 3,
                wifi_index_dc: [[0; 2]; 8],
                wifi_dc_base: [0; 2],
                shared_index_dc: [[0; 2]; 11],
                rxbb_dc_adjustments: [[0; 2]; 6],
                wifi_auxiliary: 0,
            },
        })
        .unwrap();
    assert_eq!(
        child.action(),
        crate::rx::gain::PhyRxGainInitAction::PrepareDcControlRestore
    );
    assert!(matches!(
        child.lower_external(),
        Ok(crate::rx::gain::PhyRxGainInitExternalBinding::Mmio(_))
    ));
    assert!(child.commit().is_err());
}

#[test]
fn channel_parent_proof_starts_the_complete_async_hardware_graph() {
    let mut transition = PhyCalibrationTrackingTransition::new(
        PhyCalibrationTrackingRequest {
            class: PhyCalibrationTrackClass::Wifi,
        },
        PARAMETERS,
    );
    for action in [
        PhyCalibrationTrackingAction::ClearPbus,
        PhyCalibrationTrackingAction::CalibrateDcode,
        PhyCalibrationTrackingAction::RecalibrateRxGain,
    ] {
        assert_eq!(transition.action(), action);
        transition.advance(completion(action)).unwrap();
    }

    let state = crate::state::PhyState::new(crate::state::PhyConfig::production());
    let child = transition
        .begin_channel_restore(state.channel_parameters())
        .unwrap();
    assert_eq!(
        child.action(),
        crate::channel::PhyChipChannelAction::SetAgc { enabled: false }
    );
    assert!(matches!(
        child.lower_external(),
        Ok(crate::channel::PhyChipChannelExternalBinding::Mmio(_))
    ));
    assert!(child.commit().is_err());
}

#[test]
fn restored_channel_temperature_drives_following_class_threshold_and_references() {
    let mut transition = PhyCalibrationTrackingTransition::new(
        PhyCalibrationTrackingRequest {
            class: PhyCalibrationTrackClass::Wifi,
        },
        PhyCalibrationTrackingParameters {
            wifi_reference_temperature: 30,
            ..PARAMETERS
        },
    );
    for action in [
        PhyCalibrationTrackingAction::ClearPbus,
        PhyCalibrationTrackingAction::CalibrateDcode,
        PhyCalibrationTrackingAction::RecalibrateRxGain,
    ] {
        transition.advance(completion(action)).unwrap();
    }
    transition
        .advance(PhyCalibrationTrackingCompletion::ChipChannelRestored(
            PhyCalibrationChannelCompletion {
                result: Ok(channel_outcome(11, 1, 60)),
            },
        ))
        .unwrap();
    transition
        .advance(PhyCalibrationTrackingCompletion::MacBasebandEnabled)
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyCalibrationTrackingAction::SetHardwareFrequencyControl { enabled: false }
    );

    loop {
        let action = transition.action();
        if let PhyCalibrationTrackingAction::Complete(outcome) = action {
            assert_eq!(outcome.common_reference_temperature, 60);
            assert_eq!(outcome.wifi_reference_temperature, 60);
            assert_eq!(outcome.channel, Some(channel_outcome(11, 1, 60)));
            break;
        }
        transition.advance(completion(action)).unwrap();
    }
}

#[test]
fn class_tx_dc_pwdet_parent_selects_both_complete_hardware_graphs() {
    for class in [
        PhyCalibrationTrackClass::Wifi,
        PhyCalibrationTrackClass::BluetoothIeee802154,
    ] {
        let mut transition = PhyCalibrationTrackingTransition::new(
            PhyCalibrationTrackingRequest { class },
            PhyCalibrationTrackingParameters {
                common_reference_temperature: 50,
                ..PARAMETERS
            },
        );
        while !matches!(
            transition.action(),
            PhyCalibrationTrackingAction::CalibrateTxDcPwdet { .. }
        ) {
            let action = transition.action();
            transition.advance(completion(action)).unwrap();
        }

        let state = crate::state::PhyState::new(crate::state::PhyConfig::production());
        let child = transition
            .begin_tx_dc_pwdet(
                state.tx_dc_pwdet_parameters(),
                state.bluetooth_tx_dc_pwdet_transition(),
            )
            .unwrap();
        assert_eq!(child.class(), class);
        assert_eq!(
            child.action(),
            crate::tx::dc_power_detector::PhyTxDcPwdetAction::PrepareRegisters
        );
        assert!(matches!(
            child.lower_external(),
            Ok(crate::tx::dc_power_detector::PhyTxDcPwdetExternalBinding::Mmio(_))
        ));
        assert!(child.commit().is_err());
    }
}

#[test]
fn class_tx_gain_publication_captures_pending_dco_for_both_radio_banks() {
    for class in [
        PhyCalibrationTrackClass::Wifi,
        PhyCalibrationTrackClass::BluetoothIeee802154,
    ] {
        let mut transition = PhyCalibrationTrackingTransition::new(
            PhyCalibrationTrackingRequest { class },
            PhyCalibrationTrackingParameters {
                common_reference_temperature: 50,
                ..PARAMETERS
            },
        );
        while !matches!(
            transition.action(),
            PhyCalibrationTrackingAction::CalibrateTxDcPwdet { .. }
        ) {
            let action = transition.action();
            transition.advance(completion(action)).unwrap();
        }
        transition
            .advance(PhyCalibrationTrackingCompletion::TxDcPwdetCalibrated(
                PhyCalibrationTxDcPwdetCompletion {
                    class,
                    result: Ok(tx_dc_pwdet_outcome(class)),
                },
            ))
            .unwrap();

        let state = crate::state::PhyState::new(crate::state::PhyConfig::production());
        let binding = transition.begin_tx_gain_publication(&state).unwrap();
        assert_eq!(binding.action(), transition.action());
        let dco = match class {
            PhyCalibrationTrackClass::Wifi => 0x111,
            PhyCalibrationTrackClass::BluetoothIeee802154 => 0x222,
        };
        let expected_seed = [dco | (dco << 16); 6];
        match binding.publication {
            PhyCalibrationTxGainPublication::Wifi { channel, image } => {
                assert_eq!(channel, PARAMETERS.current_channel);
                assert_eq!(image.unwrap().seed, expected_seed);
            }
            PhyCalibrationTxGainPublication::BluetoothIeee802154 { image } => {
                assert_eq!(image.seed, expected_seed);
            }
        }

        let wrong = PhyCalibrationTxGainCompletion {
            class: match class {
                PhyCalibrationTrackClass::Wifi => PhyCalibrationTrackClass::BluetoothIeee802154,
                PhyCalibrationTrackClass::BluetoothIeee802154 => PhyCalibrationTrackClass::Wifi,
            },
            channel: None,
        };
        assert_eq!(
            transition.advance(PhyCalibrationTrackingCompletion::TxGainPublished(wrong)),
            Err(PhyCalibrationTrackingTransitionError::WrongCompletion)
        );
    }
}

#[test]
fn tx_dc_pwdet_failure_runs_outer_force_frequency_and_gain_cleanup() {
    let mut transition = PhyCalibrationTrackingTransition::new(
        PhyCalibrationTrackingRequest {
            class: PhyCalibrationTrackClass::BluetoothIeee802154,
        },
        PhyCalibrationTrackingParameters {
            common_reference_temperature: 50,
            ..PARAMETERS
        },
    );
    while !matches!(
        transition.action(),
        PhyCalibrationTrackingAction::CalibrateTxDcPwdet { .. }
    ) {
        let action = transition.action();
        transition.advance(completion(action)).unwrap();
    }
    let failure = crate::tx::dc_power_detector::PhyTxDcPwdetFailure::PbusTimedOut(
        crate::analog::pbus::PhyPbusForceTest::new(4, 1, 0),
    );
    transition
        .advance(PhyCalibrationTrackingCompletion::TxDcPwdetCalibrated(
            PhyCalibrationTxDcPwdetCompletion {
                class: PhyCalibrationTrackClass::BluetoothIeee802154,
                result: Err(failure),
            },
        ))
        .unwrap();
    for expected in [
        PhyCalibrationTrackingAction::ForceTxRxOff { enabled: false },
        PhyCalibrationTrackingAction::SetHardwareFrequencyControl { enabled: true },
        PhyCalibrationTrackingAction::RestoreTxGainCompensation,
    ] {
        assert_eq!(transition.action(), expected);
        transition.advance(completion(expected)).unwrap();
    }
    assert_eq!(
        transition.action(),
        PhyCalibrationTrackingAction::Failed(PhyCalibrationTrackingFailure::TxDcPwdet(failure))
    );
}

#[test]
fn rx_gain_failure_restores_gain_without_publishing_common_progress() {
    let mut transition = PhyCalibrationTrackingTransition::new(
        PhyCalibrationTrackingRequest {
            class: PhyCalibrationTrackClass::Wifi,
        },
        PARAMETERS,
    );
    for action in [
        PhyCalibrationTrackingAction::ClearPbus,
        PhyCalibrationTrackingAction::CalibrateDcode,
    ] {
        transition.advance(completion(action)).unwrap();
    }
    let failure = crate::rx::gain::PhyRxGainInitFailure::Publish(
        crate::rx::gain::PhyRxGainPublishFailure::PbusTimedOut {
            bank: crate::calibration::baseband::PhyRxGainBank::Wifi,
            transaction: crate::analog::pbus::PhyPbusForceTest::new(4, 1, 0),
        },
    );
    transition
        .advance(PhyCalibrationTrackingCompletion::RxGainRecalibrated(
            PhyCalibrationRxGainCompletion {
                result: Err(failure),
            },
        ))
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyCalibrationTrackingAction::RestoreTxGainCompensation
    );
    transition
        .advance(PhyCalibrationTrackingCompletion::TxGainCompensationRestored)
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyCalibrationTrackingAction::Failed(PhyCalibrationTrackingFailure::RxGain(failure))
    );
}

#[test]
fn class_pbus_timeout_restores_force_frequency_and_gain_before_failure() {
    let mut transition = PhyCalibrationTrackingTransition::new(
        PhyCalibrationTrackingRequest {
            class: PhyCalibrationTrackClass::BluetoothIeee802154,
        },
        PhyCalibrationTrackingParameters {
            common_reference_temperature: 50,
            wifi_reference_temperature: 0,
            bluetooth_ieee802154_reference_temperature: 20,
            ..PARAMETERS
        },
    );
    transition
        .advance(PhyCalibrationTrackingCompletion::HardwareFrequencyControlSet { enabled: false })
        .unwrap();
    transition
        .advance(PhyCalibrationTrackingCompletion::ForceTxRxCompleted(
            PhyCalibrationForceTxRxCompletion { enabled: true },
        ))
        .unwrap();

    let child = transition.begin_pbus_clear().unwrap();
    let mut child = child.commit().unwrap_err();
    let crate::calibration::cold::PhyColdExternalBinding::Mmio(binding) =
        child.lower_external().unwrap()
    else {
        panic!("PBus clear must begin with debug-mode MMIO")
    };
    child
        .advance_external(binding.into_completion().unwrap())
        .unwrap();

    let crate::analog::pbus::PhyPbusClearAction::ForceTest(transaction) = child.action() else {
        panic!("PBus clear did not publish its first transaction")
    };
    let crate::calibration::cold::PhyColdExternalBinding::Pbus(mut binding) =
        child.lower_external().unwrap()
    else {
        panic!("force-test action did not lower to a PBus owner")
    };
    binding.started().unwrap();
    child
        .advance_external(binding.into_timeout_completion().unwrap())
        .unwrap();
    let completion = child.commit().unwrap();
    transition.advance(completion).unwrap();

    assert_eq!(
        transition.action(),
        PhyCalibrationTrackingAction::ForceTxRxOff { enabled: false }
    );
    transition
        .advance(PhyCalibrationTrackingCompletion::ForceTxRxCompleted(
            PhyCalibrationForceTxRxCompletion { enabled: false },
        ))
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyCalibrationTrackingAction::SetHardwareFrequencyControl { enabled: true }
    );
    transition
        .advance(PhyCalibrationTrackingCompletion::HardwareFrequencyControlSet { enabled: true })
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyCalibrationTrackingAction::RestoreTxGainCompensation
    );
    transition
        .advance(PhyCalibrationTrackingCompletion::TxGainCompensationRestored)
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyCalibrationTrackingAction::Failed(PhyCalibrationTrackingFailure::PbusClearTimedOut(
            transaction
        ))
    );
    assert_eq!(
        transition.advance(PhyCalibrationTrackingCompletion::TxGainCompensationRestored),
        Err(PhyCalibrationTrackingTransitionError::AlreadyComplete)
    );
}
