use super::*;
use std::vec::Vec;

const POLICY: PhyParamTrackingPolicy = PhyParamTrackingPolicy {
    tracking_inhibited: false,
    rfpll_cap_tracking_enabled: true,
    rfpll_cap_tracking_threshold: None,
    calibration_tracking_threshold: None,
    diagnostics: PhyTrackingDiagnostics::Enabled,
    bluetooth_ieee802154_power_tracking_enabled: true,
    calibration_tracking_enabled: true,
    relaxed_power_tracking_threshold: false,
};

fn completion(action: PhyParamTrackingAction) -> PhyParamTrackingCompletion {
    match action {
        PhyParamTrackingAction::EnterCritical => PhyParamTrackingCompletion::EnteredCritical,
        PhyParamTrackingAction::RfpllCapTrack { .. } => {
            PhyParamTrackingCompletion::RfpllCapTracked(PhyParamTrackingRfpllCompletion {
                committed: (),
            })
        }
        PhyParamTrackingAction::BluetoothIeee802154TxPowerTrack { enabled, .. } => {
            PhyParamTrackingCompletion::BluetoothIeee802154TxPowerTracked { enabled }
        }
        PhyParamTrackingAction::CalibrationTrack { class, .. } => {
            PhyParamTrackingCompletion::CalibrationTracked(PhyParamTrackingCalibrationCompletion {
                class,
            })
        }
        PhyParamTrackingAction::WifiI2cTrack => PhyParamTrackingCompletion::WifiI2cTracked,
        PhyParamTrackingAction::WifiTxPowerTrack { enabled, .. } => {
            PhyParamTrackingCompletion::WifiTxPowerTracked { enabled }
        }
        PhyParamTrackingAction::TemperatureRead => PhyParamTrackingCompletion::TemperatureRead,
        PhyParamTrackingAction::ExitCritical => PhyParamTrackingCompletion::ExitedCritical,
        PhyParamTrackingAction::Complete(_) => panic!("terminal action has no completion"),
    }
}

fn run(
    request: PhyParamTrackRequest,
    policy: PhyParamTrackingPolicy,
) -> Vec<PhyParamTrackingAction> {
    let mut transition = PhyParamTrackingTransition::new(request, policy);
    let mut actions = Vec::new();
    for _ in 0..10 {
        let action = transition.action();
        actions.push(action);
        if matches!(action, PhyParamTrackingAction::Complete(_)) {
            return actions;
        }
        transition.advance(completion(action)).unwrap();
    }
    panic!("finite tracking transition exceeded its maximum path length")
}

#[test]
fn ieee802154_only_preserves_exact_child_order() {
    assert_eq!(
        run(PhyParamTrackRequest::new(false, true), POLICY),
        [
            PhyParamTrackingAction::EnterCritical,
            PhyParamTrackingAction::RfpllCapTrack {
                diagnostics: PhyTrackingDiagnostics::Enabled,
            },
            PhyParamTrackingAction::BluetoothIeee802154TxPowerTrack {
                enabled: true,
                diagnostics: PhyTrackingDiagnostics::Enabled,
            },
            PhyParamTrackingAction::CalibrationTrack {
                diagnostics: PhyTrackingDiagnostics::Enabled,
                class: PhyCalibrationTrackClass::BluetoothIeee802154,
            },
            PhyParamTrackingAction::TemperatureRead,
            PhyParamTrackingAction::ExitCritical,
            PhyParamTrackingAction::Complete(PhyParamTrackingOutcome {
                clients: PhyParamTrackRequest::new(false, true),
                tracking_inhibited: false,
            }),
        ]
    );
}

#[test]
fn both_classes_run_bluetooth_before_wifi_and_temperature_last() {
    assert_eq!(
        run(PhyParamTrackRequest::new(true, true), POLICY),
        [
            PhyParamTrackingAction::EnterCritical,
            PhyParamTrackingAction::RfpllCapTrack {
                diagnostics: PhyTrackingDiagnostics::Enabled,
            },
            PhyParamTrackingAction::BluetoothIeee802154TxPowerTrack {
                enabled: true,
                diagnostics: PhyTrackingDiagnostics::Enabled,
            },
            PhyParamTrackingAction::CalibrationTrack {
                diagnostics: PhyTrackingDiagnostics::Enabled,
                class: PhyCalibrationTrackClass::BluetoothIeee802154,
            },
            PhyParamTrackingAction::WifiI2cTrack,
            PhyParamTrackingAction::WifiTxPowerTrack {
                enabled: true,
                diagnostics: PhyTrackingDiagnostics::Enabled,
            },
            PhyParamTrackingAction::CalibrationTrack {
                diagnostics: PhyTrackingDiagnostics::Enabled,
                class: PhyCalibrationTrackClass::Wifi,
            },
            PhyParamTrackingAction::TemperatureRead,
            PhyParamTrackingAction::ExitCritical,
            PhyParamTrackingAction::Complete(PhyParamTrackingOutcome {
                clients: PhyParamTrackRequest::new(true, true),
                tracking_inhibited: false,
            }),
        ]
    );
}

#[test]
fn guard_exits_critical_section_without_running_children() {
    let mut policy = POLICY;
    policy.tracking_inhibited = true;
    assert_eq!(
        run(PhyParamTrackRequest::new(true, true), policy),
        [
            PhyParamTrackingAction::EnterCritical,
            PhyParamTrackingAction::ExitCritical,
            PhyParamTrackingAction::Complete(PhyParamTrackingOutcome {
                clients: PhyParamTrackRequest::new(true, true),
                tracking_inhibited: true,
            }),
        ]
    );
}

#[test]
fn disabled_optional_branches_are_absent() {
    let policy = PhyParamTrackingPolicy {
        rfpll_cap_tracking_enabled: false,
        calibration_tracking_enabled: false,
        ..POLICY
    };
    assert_eq!(
        run(PhyParamTrackRequest::new(false, true), policy),
        [
            PhyParamTrackingAction::EnterCritical,
            PhyParamTrackingAction::BluetoothIeee802154TxPowerTrack {
                enabled: true,
                diagnostics: PhyTrackingDiagnostics::Enabled,
            },
            PhyParamTrackingAction::TemperatureRead,
            PhyParamTrackingAction::ExitCritical,
            PhyParamTrackingAction::Complete(PhyParamTrackingOutcome {
                clients: PhyParamTrackRequest::new(false, true),
                tracking_inhibited: false,
            }),
        ]
    );
}

#[test]
fn wrong_completion_does_not_advance_and_terminal_rejects_more_work() {
    let mut transition =
        PhyParamTrackingTransition::new(PhyParamTrackRequest::new(false, true), POLICY);
    assert_eq!(
        transition.advance(PhyParamTrackingCompletion::TemperatureRead),
        Err(PhyParamTrackingTransitionError::WrongCompletion)
    );
    assert_eq!(transition.action(), PhyParamTrackingAction::EnterCritical);

    while !matches!(transition.action(), PhyParamTrackingAction::Complete(_)) {
        transition.advance(completion(transition.action())).unwrap();
    }
    assert_eq!(
        transition.advance(PhyParamTrackingCompletion::ExitedCritical),
        Err(PhyParamTrackingTransitionError::AlreadyComplete)
    );
}

#[test]
fn rfpll_child_routes_threshold_and_mints_parent_proof_only_after_commit() {
    let policy = PhyParamTrackingPolicy {
        rfpll_cap_tracking_threshold: Some(6),
        calibration_tracking_enabled: false,
        ..POLICY
    };
    let mut transition =
        PhyParamTrackingTransition::new(PhyParamTrackRequest::new(false, true), policy);
    transition
        .advance(PhyParamTrackingCompletion::EnteredCritical)
        .unwrap();

    let mut state = crate::state::PhyState::new(crate::state::PhyConfig::production());
    state.apply_register_temperature_outcome(
        crate::state::PhyRegisterTemperatureControl::FULL,
        crate::analog::temperature::PhyTemperatureOutcome {
            temperature: 20,
            sensor_index: 2,
            next_dac: 15,
        },
    );
    state.apply_temperature_outcome(crate::analog::temperature::PhyTemperatureOutcome {
        temperature: 25,
        sensor_index: 2,
        next_dac: 15,
    });

    let child = transition.begin_rfpll_cap_tracking(&mut state).unwrap();
    let crate::analog::rfpll::RfpllCapTrackingAction::Complete(outcome) = child.action() else {
        panic!("six-degree override must skip a five-degree delta")
    };
    assert_eq!(outcome.threshold, 6);
    assert!(!outcome.updated);
    let completion = child.commit().unwrap();
    assert_eq!(
        state
            .rfpll_cap_tracking_parameters(None)
            .reference_temperature,
        20
    );
    transition.advance(completion).unwrap();
    assert!(matches!(
        transition.action(),
        PhyParamTrackingAction::BluetoothIeee802154TxPowerTrack { .. }
    ));

    let mut incomplete = PhyParamTrackingTransition::new(
        PhyParamTrackRequest::new(false, false),
        PhyParamTrackingPolicy {
            rfpll_cap_tracking_threshold: None,
            ..policy
        },
    );
    incomplete
        .advance(PhyParamTrackingCompletion::EnteredCritical)
        .unwrap();
    let child = incomplete.begin_rfpll_cap_tracking(&mut state).unwrap();
    assert_eq!(
        child.action(),
        crate::analog::rfpll::RfpllCapTrackingAction::SetHardwareFrequencyControl {
            enabled: false
        }
    );
    let child = child.commit().unwrap_err();
    assert!(matches!(
        child.lower_external(),
        Ok(crate::analog::rfpll::RfpllCapTrackingExternalBinding::Mmio(
            _
        ))
    ));
}

#[test]
fn calibration_child_routes_threshold_and_mints_parent_proof_only_after_commit() {
    let policy = PhyParamTrackingPolicy {
        rfpll_cap_tracking_enabled: false,
        calibration_tracking_threshold: Some(31),
        ..POLICY
    };
    let mut transition =
        PhyParamTrackingTransition::new(PhyParamTrackRequest::new(false, true), policy);
    transition
        .advance(PhyParamTrackingCompletion::EnteredCritical)
        .unwrap();
    transition
        .advance(
            PhyParamTrackingCompletion::BluetoothIeee802154TxPowerTracked {
                enabled: policy.bluetooth_ieee802154_power_tracking_enabled,
            },
        )
        .unwrap();

    let mut state = crate::state::PhyState::new(crate::state::PhyConfig::production());
    state.apply_register_temperature_outcome(
        crate::state::PhyRegisterTemperatureControl::FULL,
        crate::analog::temperature::PhyTemperatureOutcome {
            temperature: 20,
            sensor_index: 2,
            next_dac: 15,
        },
    );
    state.apply_temperature_outcome(crate::analog::temperature::PhyTemperatureOutcome {
        temperature: 50,
        sensor_index: 2,
        next_dac: 15,
    });

    let child = transition.begin_calibration_tracking(&mut state).unwrap();
    assert_eq!(child.parent_action(), transition.action());
    assert_eq!(
        child.action(),
        crate::tracking::calibration::PhyCalibrationTrackingAction::RestoreTxGainCompensation
    );
    let mut child = child.commit().unwrap_err();
    let binding = child.lower_external().unwrap();
    assert!(matches!(
        binding,
        crate::tracking::calibration::PhyCalibrationTrackingExternalBinding::Register(_)
    ));
    assert_eq!(
        child
            .state()
            .calibration_tracking_parameters(None)
            .common_reference_temperature,
        20
    );
    child
        .advance(
            crate::tracking::calibration::PhyCalibrationTrackingCompletion::TxGainCompensationRestored,
        )
        .unwrap();
    let completion = child.commit().unwrap();

    let committed = state.calibration_tracking_parameters(None);
    assert_eq!(committed.common_reference_temperature, 20);
    assert_eq!(committed.bluetooth_ieee802154_reference_temperature, 20);
    transition.advance(completion).unwrap();
    assert_eq!(transition.action(), PhyParamTrackingAction::TemperatureRead);
}

#[test]
fn failed_temperature_child_cannot_complete_parent_or_mutate_state() {
    let mut policy = POLICY;
    policy.rfpll_cap_tracking_enabled = false;
    let mut transition =
        PhyParamTrackingTransition::new(PhyParamTrackRequest::new(false, false), policy);
    transition
        .advance(PhyParamTrackingCompletion::EnteredCritical)
        .unwrap();
    assert_eq!(transition.action(), PhyParamTrackingAction::TemperatureRead);

    let mut state = crate::state::PhyState::new(crate::state::PhyConfig::production());
    let mut temperature = transition.begin_temperature_read(&mut state).unwrap();
    let crate::analog::temperature::PhyTemperatureAction::ReadMasked { field } =
        temperature.action()
    else {
        panic!("temperature child did not begin with its DAC read")
    };
    temperature
        .advance(
            crate::analog::temperature::PhyTemperatureCompletion::MaskedRead { field, value: 3 },
        )
        .unwrap();
    assert_eq!(
        temperature.action(),
        crate::analog::temperature::PhyTemperatureAction::Failed(
            crate::analog::temperature::PhyTemperatureFailure::InvalidDac(3),
        )
    );
    let temperature = temperature.commit().unwrap_err();
    assert_eq!(
        temperature
            .state()
            .tx_power_tracking_parameters(false)
            .current_temperature,
        0
    );
}

#[test]
fn registered_policy_projects_cold_facts_and_live_tracking_state() {
    let mut state = crate::state::PhyState::new(crate::state::PhyConfig::production());

    let cold = PhyParamTrackingPolicy::for_registered_state(&state);
    assert!(!cold.tracking_inhibited);
    assert!(!cold.rfpll_cap_tracking_enabled);
    assert_eq!(cold.rfpll_cap_tracking_threshold, None);
    assert_eq!(cold.calibration_tracking_threshold, None);
    assert_eq!(cold.diagnostics, PhyTrackingDiagnostics::Disabled);
    assert!(cold.bluetooth_ieee802154_power_tracking_enabled);
    assert!(cold.calibration_tracking_enabled);
    assert!(cold.relaxed_power_tracking_threshold);

    state.set_bt_power_tracking(0);
    state.set_tx_power_tracking_slow(0);
    state.set_temperature_tracking_debug(3, 17);
    let configured = PhyParamTrackingPolicy::for_registered_state(&state);
    assert_eq!(configured.rfpll_cap_tracking_threshold, Some(17));
    assert_eq!(configured.calibration_tracking_threshold, Some(17));
    assert!(!configured.bluetooth_ieee802154_power_tracking_enabled);
    assert!(!configured.relaxed_power_tracking_threshold);
}

#[test]
fn temperature_to_power_matches_all_signed_16_bit_deltas() {
    for raw_delta in 0..=u16::MAX {
        let delta = raw_delta as i16;
        let expected_wifi = ((i32::from(delta) / 3) as u8) as i8;
        let expected_bluetooth_ieee802154 = ((i32::from(delta) / 4) as u8) as i8;
        let expected_positive = ((i32::from(delta) / 5) as u8) as i8;

        assert_eq!(
            temperature_to_tracking_power(
                0,
                0_i16.wrapping_sub(delta),
                PhyCalibrationTrackClass::Wifi
            ),
            if delta > 0 {
                expected_positive
            } else {
                expected_wifi
            },
        );
        assert_eq!(
            temperature_to_tracking_power(
                0,
                0_i16.wrapping_sub(delta),
                PhyCalibrationTrackClass::BluetoothIeee802154,
            ),
            if delta > 0 {
                expected_positive
            } else {
                expected_bluetooth_ieee802154
            },
        );
    }

    assert_eq!(
        temperature_to_tracking_power(i16::MIN, 1, PhyCalibrationTrackClass::BluetoothIeee802154,),
        (32_767_i16 / 5) as i8,
    );
}
