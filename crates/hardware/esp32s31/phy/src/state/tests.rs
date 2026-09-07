use super::*;

const IDENTITY: crate::calibration::registration::PhyCalibrationIdentity =
    crate::calibration::registration::PhyCalibrationIdentity {
        rf_cal_version: 7,
        base_mac_address: [2, 3, 5, 7, 11, 13],
        mac_extension: 17,
    };

#[test]
fn rfpll_tracking_reference_is_initialized_and_committed_only_on_update() {
    let mut state = PhyState::new(PhyConfig::production());
    state.apply_register_temperature_outcome(
        PhyRegisterTemperatureControl::FULL,
        PhyTemperatureOutcome {
            temperature: 20,
            sensor_index: 3,
            next_dac: 4,
        },
    );
    assert_eq!(
        state.rfpll_cap_tracking_parameters(None),
        RfpllCapTrackingParameters {
            current_temperature: 20,
            reference_temperature: 20,
            threshold_override: None,
            current_channel: 0,
        }
    );

    state.apply_temperature_outcome(PhyTemperatureOutcome {
        temperature: 25,
        sensor_index: 3,
        next_dac: 4,
    });
    state.apply_rfpll_cap_tracking_outcome(RfpllCapTrackingOutcome {
        threshold: 5,
        current_temperature: 25,
        previous_reference_temperature: 20,
        reference_temperature: 99,
        correction: None,
        updated: false,
    });
    assert_eq!(
        state
            .rfpll_cap_tracking_parameters(Some(7))
            .reference_temperature,
        20
    );

    state.apply_rfpll_cap_tracking_outcome(RfpllCapTrackingOutcome {
        threshold: 5,
        current_temperature: 25,
        previous_reference_temperature: 20,
        reference_temperature: 25,
        correction: Some(crate::analog::rfpll::RfpllCapCorrectionOutcome {
            direction: crate::analog::rfpll::RfpllCapCorrectionDirection::StableZero,
            update: None,
        }),
        updated: true,
    });
    assert_eq!(
        state
            .rfpll_cap_tracking_parameters(None)
            .reference_temperature,
        25
    );
}

#[test]
fn calibration_tracking_references_are_semantic_and_commit_per_branch() {
    let mut state = PhyState::new(PhyConfig::production());
    state.apply_register_temperature_outcome(
        PhyRegisterTemperatureControl::FULL,
        PhyTemperatureOutcome {
            temperature: 20,
            sensor_index: 2,
            next_dac: 15,
        },
    );
    let initial = state.calibration_tracking_parameters(None);
    assert_eq!(initial.common_reference_temperature, 20);
    assert_eq!(initial.wifi_reference_temperature, 20);
    assert_eq!(initial.bluetooth_ieee802154_reference_temperature, 20);

    state.apply_calibration_tracking_outcome(PhyCalibrationTrackingOutcome {
        class: crate::tracking::parameters::PhyCalibrationTrackClass::Wifi,
        threshold: 30,
        common_reference_temperature: 50,
        wifi_reference_temperature: 50,
        bluetooth_ieee802154_reference_temperature: 99,
        common_updated: true,
        class_updated: true,
        dcode: Some(PhyDcodeOutcome { codes: [7; 8] }),
        rx_gain: Some(crate::rx::gain::PhyRxGainInitOutcome {
            dc: Some(crate::rx::gain_calibration::PhyRxGainDcOutcome {
                wifi_index_dc: [[1; 2]; 8],
                wifi_dc_base: [2; 2],
                shared_index_dc: [[3; 2]; 11],
                rxbb_dc_adjustments: [[4; 2]; 6],
            }),
            generated_tables: true,
            wifi_last_index: 69,
            shared_last_index: 75,
        }),
        channel: Some(crate::channel::PhyChipChannelOutcome {
            channel: 11,
            frequency_mhz: 2_462,
            cbw: 1,
            init_complete: true,
            temperature: PhyTemperatureOutcome {
                temperature: 50,
                sensor_index: 3,
                next_dac: 15,
            },
        }),
        tx_dc_pwdet: Some(crate::tx::dc_power_detector::PhyTxDcPwdetOutcome {
            dco: [[5; 4]; 3],
            total_measurements: 144,
        }),
    });
    let committed = state.calibration_tracking_parameters(Some(31));
    assert_eq!(committed.threshold_override, Some(31));
    assert_eq!(committed.common_reference_temperature, 50);
    assert_eq!(committed.wifi_reference_temperature, 50);
    assert_eq!(committed.bluetooth_ieee802154_reference_temperature, 20);
    assert_eq!(state.common.dcode, [7; 8]);
    assert_eq!(state.wifi.wifi_rx_table_last_index, 69);
    assert_eq!(state.wifi.shared_rx_table_last_index, 75);
    assert_eq!(state.wifi.wifi_index_dc, [[1; 2]; 8]);
    assert_eq!(state.wifi.shared_index_dc, [[3; 2]; 11]);
    assert!(state.wifi.rx_gain_dc_calibrated);
    assert!(state.wifi.rx_gain_tables_initialized);
    assert_eq!(state.current_wifi_channel(), 11);
    assert_eq!(state.common.temperature, 50);
    assert_eq!(state.tx_dc_pwdet_parameters().dco, [[5; 4]; 3]);

    state.apply_calibration_tracking_outcome(PhyCalibrationTrackingOutcome {
        class: crate::tracking::parameters::PhyCalibrationTrackClass::BluetoothIeee802154,
        threshold: 30,
        common_reference_temperature: 60,
        wifi_reference_temperature: 60,
        bluetooth_ieee802154_reference_temperature: 60,
        common_updated: true,
        class_updated: true,
        dcode: None,
        rx_gain: None,
        channel: None,
        tx_dc_pwdet: Some(crate::tx::dc_power_detector::PhyTxDcPwdetOutcome {
            dco: [[6; 4]; 3],
            total_measurements: 144,
        }),
    });
    let rejected = state.calibration_tracking_parameters(None);
    assert_eq!(rejected.common_reference_temperature, 50);
    assert_eq!(rejected.bluetooth_ieee802154_reference_temperature, 20);
    assert_eq!(state.common.dcode, [7; 8]);

    state.apply_calibration_tracking_outcome(PhyCalibrationTrackingOutcome {
        class: crate::tracking::parameters::PhyCalibrationTrackClass::Wifi,
        threshold: 30,
        common_reference_temperature: 70,
        wifi_reference_temperature: 70,
        bluetooth_ieee802154_reference_temperature: 20,
        common_updated: true,
        class_updated: false,
        dcode: Some(PhyDcodeOutcome { codes: [9; 8] }),
        rx_gain: Some(crate::rx::gain::PhyRxGainInitOutcome {
            dc: None,
            generated_tables: true,
            wifi_last_index: 69,
            shared_last_index: 75,
        }),
        channel: Some(crate::channel::PhyChipChannelOutcome {
            channel: 11,
            frequency_mhz: 2_462,
            cbw: 1,
            init_complete: true,
            temperature: PhyTemperatureOutcome {
                temperature: 70,
                sensor_index: 3,
                next_dac: 15,
            },
        }),
        tx_dc_pwdet: None,
    });
    let rejected_partial_rx = state.calibration_tracking_parameters(None);
    assert_eq!(rejected_partial_rx.common_reference_temperature, 50);
    assert_eq!(state.common.dcode, [7; 8]);
}

#[test]
fn periodic_gain_tracking_commits_only_terminal_runtime_outcomes() {
    let mut state = PhyState::new(PhyConfig::production());
    state.apply_temperature_outcome(PhyTemperatureOutcome {
        temperature: 25,
        sensor_index: 3,
        next_dac: 4,
    });
    assert_eq!(
        state.tx_power_tracking_parameters(true),
        PhyTxPowerTrackingParameters {
            current_temperature: 25,
            reference_temperature: 0,
            previous_tracking_temperature: 0,
            previous_tracking_gain_base: 0,
            wifi_gain_base: 0,
            bluetooth_ieee802154_gain_base: 0,
            relaxed_threshold: true,
        }
    );

    state.apply_tx_power_tracking_outcome(PhyTxPowerTrackingOutcome {
        class: crate::tracking::parameters::PhyCalibrationTrackClass::BluetoothIeee802154,
        gain_updated: true,
        tracking_temperature: 25,
        tracking_gain_base: 5,
        wifi_gain_base: 0,
        bluetooth_ieee802154_gain_base: 5,
    });
    assert_eq!(state.bluetooth_tx_gain_parameters().base, 5);
    assert_eq!(state.channel_parameters().tx_gain_base, 0);
    assert_eq!(
        state.tx_power_tracking_parameters(false),
        PhyTxPowerTrackingParameters {
            current_temperature: 25,
            reference_temperature: 0,
            previous_tracking_temperature: 25,
            previous_tracking_gain_base: 5,
            wifi_gain_base: 0,
            bluetooth_ieee802154_gain_base: 5,
            relaxed_threshold: false,
        }
    );

    state.apply_tx_power_tracking_outcome(PhyTxPowerTrackingOutcome {
        class: crate::tracking::parameters::PhyCalibrationTrackClass::Wifi,
        gain_updated: false,
        tracking_temperature: -60,
        tracking_gain_base: -12,
        wifi_gain_base: -12,
        bluetooth_ieee802154_gain_base: -12,
    });
    assert_eq!(state.bluetooth_tx_gain_parameters().base, 5);
    assert_eq!(state.channel_parameters().tx_gain_base, 0);
}

#[test]
fn tracking_gain_regeneration_uses_live_typed_state_and_honors_wifi_skip() {
    let mut state = PhyState::new(PhyConfig::production());
    let mut bluetooth = state.bluetooth_tx_gain_parameters();
    bluetooth.base = (-7_i8) as u8;
    assert_eq!(
        state.bluetooth_ieee802154_tracking_gain_image(-7),
        calculate_bluetooth_tx_gain(bluetooth)
    );

    let parameters = state.channel_parameters();
    let mut wifi = calculate_wifi_tx_gain(PhyWifiTxGainRequest {
        channel: 11,
        calibration_curve: parameters.tx_gain_curve,
        correction: parameters.tx_gain_correction,
        base_and_delta: (5_u8).wrapping_sub(parameters.tx_gain_attenuation) as i8,
    });
    wifi.seed = parameters.tx_gain_seed;
    wifi.config = parameters.tx_gain_config;
    assert_eq!(state.wifi_tracking_gain_image(11, 5), Some(wifi));

    state.config.tx_gain_skip_publication = true;
    assert_eq!(state.wifi_tracking_gain_image(11, 5), None);
}

#[test]
fn calibration_gain_regeneration_uses_pending_txdc_seed_before_state_commit() {
    let mut state = PhyState::new(PhyConfig::production());
    let pending = PhyTxDcPwdetOutcome {
        dco: [[0x1234; 4]; 3],
        total_measurements: 144,
    };
    let expected_seed = [0x1234_1234; 6];

    assert_ne!(state.channel_parameters().tx_gain_seed, expected_seed);
    assert_ne!(state.bluetooth_tx_gain_parameters().seed, expected_seed);
    assert_eq!(
        state.wifi_calibration_gain_image(11, pending).unwrap().seed,
        expected_seed
    );
    assert_eq!(
        state
            .bluetooth_ieee802154_calibration_gain_image(pending)
            .seed,
        expected_seed
    );

    state.config.tx_gain_skip_publication = true;
    assert_eq!(state.wifi_calibration_gain_image(11, pending), None);
}

#[test]
fn cache_contains_calibration_but_not_runtime_role_state() {
    let mut calibrated = PhyState::new(PhyConfig::production());
    calibrated.set_dot11p_configuration(1, 4);
    calibrated.set_current_level(9);
    calibrated.set_tx_power_tracking_slow(0);
    calibrated.set_bt_power_tracking(0);
    calibrated.set_ble_channel_base(21);
    calibrated.mark_baseband_calibration_complete();
    calibrated.apply_tx_power_outcome(PhyTxPowerOutcome {
        reference_codes: [80, 120],
        power_curve: [-3, 4, 5],
        point_corrections: [6, -7, 8],
        power_adjustment: -9,
        final_attenuation: 13,
        current_channel: 11,
        calibration_performed: true,
    });

    let cache = calibrated.calibration_cache(IDENTITY);
    let snapshot = cache.snapshot();
    assert!(snapshot.wifi.baseband_calibrated);
    assert!(snapshot.wifi.tx_power_calibrated);
    assert_eq!(snapshot.wifi.calibrated_attenuation, 13);
    assert_eq!(snapshot.wifi.tx_power_curve, [-3, 4, 5]);
    assert_eq!(snapshot.wifi.tx_power_corrections, [6, -7, 8]);
    assert_eq!(snapshot.wifi.tx_power_adjustment, -9);

    let restored = PhyState::new(PhyConfig::esp32s31_default());
    assert_eq!(
        restored.dot11p_configuration(),
        PhyDot11pConfiguration {
            enabled: 0,
            configuration: 0
        }
    );
    assert_eq!(restored.current_level(), 0);
    assert_eq!(restored.tx_power_tracking_slow(), 1);
    assert_eq!(restored.bt_power_tracking(), 1);
    assert_eq!(restored.ble_channel_base(), 0);
    assert!(
        !restored
            .channel_frequency_control()
            .frequency_table_initialized
    );
}

#[test]
fn cache_schema_is_checked_before_artifact_admission() {
    let state = PhyState::new(PhyConfig::production());
    let cache = state.calibration_cache(IDENTITY);
    let mut snapshot = cache.into_snapshot();
    snapshot.schema += 1;
    assert!(PhyCalibrationCache::from_snapshot(snapshot).is_none());
}

#[test]
fn runtime_views_are_independent_named_state() {
    let mut state = PhyState::default();
    state.set_dot11p_configuration(1, 0x5a);
    state.set_current_level(0x34);
    state.set_bt_power_tracking(0x12);
    state.set_ble_channel_base(0x56);
    state.set_initialization_parameter(u32::MAX);
    state.set_temperature_tracking_debug(0x78, 0x9a);
    state.set_tx_power_tracking_slow(0xbc);

    assert_eq!(
        state.dot11p_configuration(),
        PhyDot11pConfiguration {
            enabled: 1,
            configuration: 0x5a,
        }
    );
    assert_eq!(state.current_level(), 0x34);
    assert_eq!(state.bt_power_tracking(), 0x12);
    assert_eq!(state.ble_channel_base(), 0x56);
    assert!(state.initialization_parameter());
    assert_eq!(
        state.temperature_tracking_debug(),
        PhyTemperatureTrackingDebug {
            first: 0x78,
            second: 0x9a,
        }
    );
    assert_eq!(state.tx_power_tracking_slow(), 0xbc);
}

#[test]
fn rx_table_preparation_updates_both_semantic_indices() {
    let mut state = PhyState::default();
    assert_eq!(
        state.prepare_rx_table_init(),
        PhyRxTableInitParameters {
            parameter_002: 0xbf,
            parameter_121: PHY_RX_TABLE_ENTRY_COUNT,
        }
    );
    assert_eq!(
        state.register_init_parameters(),
        PhyRegisterInitParameters {
            parameter_121: PHY_RX_TABLE_ENTRY_COUNT,
            parameter_120: PHY_RX_TABLE_ENTRY_COUNT,
        }
    );
}

#[test]
fn rx_saturation_is_one_way_and_failures_do_not_commit() {
    let mut state = PhyState::default();
    assert_eq!(state.rx_saturation_parameter_002(), 0xbf);
    assert_eq!(
        state.apply_rx_saturation_outcome(PhyRxSaturationOutcome::CaptureTimedOut),
        Err(PhyRxSaturationOutcome::CaptureTimedOut)
    );
    assert!(!state.rx_gain_dc_parameters().rx_saturation_detected);

    state
        .apply_rx_saturation_outcome(PhyRxSaturationOutcome::Measured {
            saturated_samples: 1,
            samples: 100,
        })
        .unwrap();
    state
        .apply_rx_saturation_outcome(PhyRxSaturationOutcome::Measured {
            saturated_samples: 0,
            samples: 100,
        })
        .unwrap();
    assert!(state.rx_gain_dc_parameters().rx_saturation_detected);
}

#[test]
fn calibration_outputs_have_named_cache_fields() {
    let mut state = PhyState::default();
    state.apply_temperature_outcome(PhyTemperatureOutcome {
        temperature: -37,
        sensor_index: 3,
        next_dac: 11,
    });
    state.apply_dcode_outcome(PhyDcodeOutcome {
        codes: [1, 2, 3, 4, 5, 6, 7, 8],
    });

    let snapshot = state.calibration_cache(IDENTITY).into_snapshot();
    assert_eq!(snapshot.common.temperature, -37);
    assert_eq!(snapshot.common.sensor_index, 3);
    assert_eq!(snapshot.common.dcode, [1, 2, 3, 4, 5, 6, 7, 8]);
}

#[test]
fn wifi_tx_calibration_updates_only_wifi_calibration_data() {
    let mut state = PhyState::default();
    let bluetooth_before = state.bluetooth_tx_dco();
    let outcome = PhyTxDcOutcome {
        dco: [
            [1, 2, 3, 4],
            [5, 6, 7, 8],
            [9, 10, 11, 12],
            [13, 14, 15, 16],
            [17, 18, 19, 20],
        ],
    };
    state.apply_tx_dc_outcome(outcome);
    state.apply_pwdet_outcome(PhyPwdetOutcome {
        reference_codes: [-101, 202],
        calibrated: true,
        measurement_performed: true,
    });

    assert_eq!(
        state.tx_dc_pwdet_parameters().dco,
        [outcome.dco[0], outcome.dco[1], outcome.dco[2]]
    );
    assert_eq!(state.pwdet_parameters().reference_codes, [-101, 202]);
    assert!(state.pwdet_parameters().already_calibrated);
    assert_eq!(state.bluetooth_tx_dco(), bluetooth_before);
}

#[test]
fn rf_prefix_consumes_only_typed_views() {
    let mut state = PhyState::default();
    assert!(!state.rc_calibration_complete());
    assert_eq!(
        state.xtal_duty_parameters(),
        XtalDutyCalibrationParameters {
            rf_frequency_offset_base: 0,
            pbus_rx_path_value: 0xbf,
        }
    );
    assert_eq!(
        state.channel_frequency_control(),
        PhyChannelFrequencyInitControl {
            frequency_register_parameter_override: false,
            frequency_table_initialized: false,
            front_end_parameter_bit: true,
        }
    );

    state.apply_rc_calibration(45);
    let snapshot = state.calibration_cache(IDENTITY).into_snapshot();
    assert!(snapshot.common.rc_calibrated);
    assert_ne!(snapshot.common.filter_dcap, [0; 5]);
}
