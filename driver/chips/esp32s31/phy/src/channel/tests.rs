use super::*;

const PARAMETERS: PhyChipChannelParameters = PhyChipChannelParameters {
    frequency_offset: 0,
    crystal_selector: 3,
    channel_14_mic_enabled: false,
    dot11p_enabled: false,
    dot11p_config: 0,
    tx_gain_skip_publication: false,
    tx_gain_seed: [1, 2, 3, 4, 5, 6],
    tx_gain_config: 0x1234,
    tx_gain_curve: [7, 8, 9, 10, 11, 12],
    tx_gain_correction: -3,
    tx_gain_base: 20,
    tx_gain_attenuation: 2,
    tx_capacitance: [1, 2, 3, 4, 5, 6],
};

const REQUEST: PhyChipChannelRequest = PhyChipChannelRequest {
    channel_or_frequency: 11,
    cbw: 0,
    parameters: PARAMETERS,
};

#[test]
fn rust_tx_gain_translation_matches_the_recovered_packed_layout() {
    let image = calculate_wifi_tx_gain(PhyWifiTxGainRequest {
        channel: 11,
        calibration_curve: PARAMETERS.tx_gain_curve,
        correction: PARAMETERS.tx_gain_correction,
        base_and_delta: PARAMETERS
            .tx_gain_base
            .wrapping_sub(PARAMETERS.tx_gain_attenuation) as i8,
    });
    assert_eq!(
        image.output_32,
        [
            0x373b_3f43,
            0x272b_2f33,
            0x171b_1f23,
            0x070b_0f13,
            0xf7fb_ff03,
            0xfefa_f8f7,
            0xfdf8_fcfa,
            0xf7fb_fff9,
        ]
    );
    assert_eq!(
        image.output_64,
        [
            0x0080_0080,
            0x0080_0080,
            0x0080_0080,
            0x0080_0080,
            0x0080_0080,
            0x0080_0080,
            0x0080_0080,
            0x0080_0080,
            0x0080_0080,
            0x0080_0080,
            0x0080_0080,
            0x0000_0080,
            0,
            0,
            0,
            0,
        ]
    );
    assert_eq!(
        image.output_72,
        [
            0x003f_003f,
            0x003f_003f,
            0x003f_003f,
            0x003f_003f,
            0x003f_003f,
            0x003f_003f,
            0x003f_003f,
            0x003f_003f,
            0x003f_003f,
            0x003f_003f,
            0x002f_0037,
            0x0027_0027,
            0x001f_0027,
            0x0017_001f,
            0x0015_0017,
            0x0015_0015,
        ]
    );
    assert_eq!(image.seed, [0; 6]);
    assert_eq!(image.config, 0);
}

fn temperature_completion(action: PhyTemperatureAction) -> PhyTemperatureCompletion {
    match action {
        PhyTemperatureAction::ReadMasked { field } => {
            PhyTemperatureCompletion::MaskedRead { field, value: 15 }
        }
        PhyTemperatureAction::SampleCode => PhyTemperatureCompletion::CodeSampled { value: 128 },
        PhyTemperatureAction::WriteMasked { field, value } => {
            PhyTemperatureCompletion::MaskedWrite { field, value }
        }
        action => panic!("unexpected terminal temperature action: {action:?}"),
    }
}

fn direct_completion(action: PhyChipChannelAction, ready: bool) -> PhyChipChannelCompletion {
    match action {
        PhyChipChannelAction::SetAgc { enabled } => PhyChipChannelCompletion::AgcSet { enabled },
        PhyChipChannelAction::SetBbpllCalibration { enabled } => {
            PhyChipChannelCompletion::BbpllCalibrationSet { enabled }
        }
        PhyChipChannelAction::Temperature(action) => {
            PhyChipChannelCompletion::Temperature(temperature_completion(action))
        }
        PhyChipChannelAction::StartFrequencySwitch {
            frequency_index,
            crystal_selector,
        } => PhyChipChannelCompletion::FrequencySwitchStarted {
            frequency_index,
            crystal_selector,
        },
        PhyChipChannelAction::DelayMicros { phase, micros } => {
            PhyChipChannelCompletion::DelayElapsed { phase, micros }
        }
        PhyChipChannelAction::ClearFrequencySwitch => {
            PhyChipChannelCompletion::FrequencySwitchCleared
        }
        PhyChipChannelAction::AwaitFrequencyReadyEdge { .. } => {
            PhyChipChannelCompletion::FrequencyReadyObserved { ready }
        }
        PhyChipChannelAction::ConfigureNrx { frequency_mhz } => {
            PhyChipChannelCompletion::NrxConfigured { frequency_mhz }
        }
        PhyChipChannelAction::ConfigureBssCbw { cbw } => {
            PhyChipChannelCompletion::BssCbwConfigured { cbw }
        }
        PhyChipChannelAction::ConfigureRxCompensation => {
            PhyChipChannelCompletion::RxCompensationConfigured
        }
        PhyChipChannelAction::WriteI2c {
            phase,
            address,
            value,
        } => PhyChipChannelCompletion::I2cWriteCompleted {
            phase,
            address,
            value,
        },
        PhyChipChannelAction::CalculateTxGain(request) => {
            PhyChipChannelCompletion::TxGainCalculated {
                request,
                image: PhyWifiTxGainImage {
                    seed: [0; 6],
                    output_32: [0x20; 8],
                    output_64: [0x40; 16],
                    output_72: [0x48; 16],
                    config: 0,
                },
            }
        }
        PhyChipChannelAction::PublishTxGain(_) => PhyChipChannelCompletion::TxGainPublished,
        PhyChipChannelAction::ReadI2c { phase, address } => {
            PhyChipChannelCompletion::I2cReadCompleted {
                phase,
                address,
                value: 0xc5,
            }
        }
        PhyChipChannelAction::PublishTxCapCommandMemory { value } => {
            PhyChipChannelCompletion::TxCapCommandMemoryPublished { value }
        }
        PhyChipChannelAction::ConfigureChannelCbw { cbw } => {
            PhyChipChannelCompletion::ChannelCbwConfigured { cbw }
        }
        PhyChipChannelAction::ClearDcMemory => PhyChipChannelCompletion::DcMemoryCleared,
        action => panic!("unexpected terminal channel action: {action:?}"),
    }
}

#[test]
fn pure_channel_frequency_helpers_match_24ghz_reference_edges() {
    assert_eq!(channel_to_frequency(1), 2_412);
    assert_eq!(channel_to_frequency(11), 2_462);
    assert_eq!(channel_to_frequency(14), 2_484);
    assert_eq!(channel_to_frequency(2_462), 2_462);
    assert_eq!(frequency_to_channel(2_412), 1);
    assert_eq!(frequency_to_channel(2_462), 11);
    assert_eq!(frequency_to_channel(2_484), 14);
}

#[test]
fn cold_channel_eleven_traverses_the_complete_ordered_graph() {
    let mut transition = PhyChipChannelTransition::new(REQUEST);
    let mut actions = 0;
    let mut ready_samples = 0;
    let mut rx_compensation_count = 0;
    let mut saw_gain_image = false;

    loop {
        actions += 1;
        assert!(actions < 80);
        let action = transition.action();
        match action {
            PhyChipChannelAction::AwaitFrequencyReadyEdge { samples, .. } => {
                assert_eq!(samples, ready_samples);
                ready_samples += 1;
            }
            PhyChipChannelAction::ConfigureRxCompensation => {
                rx_compensation_count += 1;
            }
            PhyChipChannelAction::PublishTxGain(image) => {
                saw_gain_image = true;
                assert_eq!(image.seed, PARAMETERS.tx_gain_seed);
                assert_eq!(image.config, PARAMETERS.tx_gain_config);
            }
            PhyChipChannelAction::Complete(outcome) => {
                assert_eq!(outcome.channel, 11);
                assert_eq!(outcome.frequency_mhz, 2_462);
                assert_eq!(outcome.cbw, 0);
                assert!(!outcome.init_complete);
                break;
            }
            PhyChipChannelAction::Failed(failure) => {
                panic!("channel transition failed: {failure:?}")
            }
            _ => {}
        }
        let completion = direct_completion(action, ready_samples == 3);
        transition.advance(completion).unwrap();
    }

    assert_eq!(ready_samples, 3);
    assert_eq!(rx_compensation_count, 2);
    assert!(saw_gain_image);
}

#[test]
fn off_grid_frequency_uses_raw_then_channel_normalized_nrx_values() {
    let mut request = REQUEST;
    request.channel_or_frequency = 2_413;
    let mut transition = PhyChipChannelTransition::new(request);
    let mut nrx = [0_u16; 2];
    let mut nrx_count = 0;

    loop {
        let action = transition.action();
        match action {
            PhyChipChannelAction::ConfigureNrx { frequency_mhz } => {
                nrx[nrx_count] = frequency_mhz;
                nrx_count += 1;
            }
            PhyChipChannelAction::Complete(outcome) => {
                assert_eq!(outcome.channel, 1);
                assert_eq!(outcome.frequency_mhz, 2_413);
                break;
            }
            PhyChipChannelAction::Failed(failure) => {
                panic!("off-grid channel transition failed: {failure:?}")
            }
            _ => {}
        }
        transition.advance(direct_completion(action, true)).unwrap();
    }

    assert_eq!(nrx, [2_413, 2_412]);
}

#[test]
fn frequency_timeout_runs_full_radio_cleanup() {
    let mut transition = PhyChipChannelTransition::new(REQUEST);
    loop {
        match transition.action() {
            PhyChipChannelAction::AwaitFrequencyReadyEdge { samples: 2, .. } => {
                transition
                    .advance(PhyChipChannelCompletion::FrequencyReadyTimedOut)
                    .unwrap();
                break;
            }
            action => {
                let completion = direct_completion(action, false);
                transition.advance(completion).unwrap();
            }
        }
    }

    assert_eq!(
        transition.action(),
        PhyChipChannelAction::SetBbpllCalibration { enabled: false }
    );
    transition
        .advance(PhyChipChannelCompletion::BbpllCalibrationSet { enabled: false })
        .unwrap();
    assert_eq!(transition.action(), PhyChipChannelAction::ClearDcMemory);
    transition
        .advance(PhyChipChannelCompletion::DcMemoryCleared)
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyChipChannelAction::SetAgc { enabled: true }
    );
    transition
        .advance(PhyChipChannelCompletion::AgcSet { enabled: true })
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyChipChannelAction::Failed(PhyChipChannelFailure::FrequencyReadyTimedOut { samples: 2 })
    );
}

#[test]
fn unsupported_profile_fails_before_touching_radio() {
    let mut request = REQUEST;
    request.parameters.channel_14_mic_enabled = true;
    assert_eq!(
        PhyChipChannelTransition::new(request).action(),
        PhyChipChannelAction::Failed(PhyChipChannelFailure::Channel14MicEnabled)
    );

    request.parameters.channel_14_mic_enabled = false;
    request.channel_or_frequency = 14;
    assert_eq!(
        PhyChipChannelTransition::new(request).action(),
        PhyChipChannelAction::Failed(PhyChipChannelFailure::UnsupportedChannel(14))
    );
}

#[test]
fn direct_binding_rejects_timer_i2c_and_terminal_actions() {
    assert_eq!(
        PhyChipChannelMmioBinding::new(PhyChipChannelAction::DelayMicros {
            phase: PhyChipChannelDelay::FrequencySettle,
            micros: 10,
        }),
        Err(PhyChipChannelBindingError::NotDirectMmio)
    );
    assert_eq!(
        PhyChipChannelMmioBinding::new(PhyChipChannelAction::WriteI2c {
            phase: PhyChipChannelI2cPhase::ProgramTxCap,
            address: TX_CAP_ADDRESS,
            value: 0xc1,
        }),
        Err(PhyChipChannelBindingError::NotDirectMmio)
    );
}

#[test]
fn external_lowering_covers_each_channel_operation_class() {
    assert!(matches!(
        PhyChipChannelExternalBinding::lower(PhyChipChannelAction::SetAgc { enabled: false }),
        Ok(PhyChipChannelExternalBinding::Mmio(_))
    ));
    assert!(matches!(
        PhyChipChannelExternalBinding::lower(PhyChipChannelAction::Temperature(
            PhyTemperatureTransition::new().action()
        )),
        Ok(PhyChipChannelExternalBinding::Temperature(_))
    ));
    assert!(matches!(
        PhyChipChannelExternalBinding::lower(PhyChipChannelAction::DelayMicros {
            phase: PhyChipChannelDelay::FrequencySettle,
            micros: 10,
        }),
        Ok(PhyChipChannelExternalBinding::Timer(_))
    ));
    assert!(matches!(
        PhyChipChannelExternalBinding::lower(PhyChipChannelAction::WriteI2c {
            phase: PhyChipChannelI2cPhase::ProgramTxCap,
            address: TX_CAP_ADDRESS,
            value: 0xc1,
        }),
        Ok(PhyChipChannelExternalBinding::I2c(_))
    ));
    assert!(matches!(
        PhyChipChannelExternalBinding::lower(PhyChipChannelAction::CalculateTxGain(
            PhyWifiTxGainRequest {
                channel: 1,
                calibration_curve: [0; 6],
                correction: 0,
                base_and_delta: 0,
            }
        )),
        Ok(PhyChipChannelExternalBinding::TxGain(_))
    ));
    assert!(matches!(
        PhyChipChannelExternalBinding::lower(PhyChipChannelAction::Failed(
            PhyChipChannelFailure::UnsupportedChannel(14)
        )),
        Err(PhyChipChannelExternalBindingError::UnsupportedAction)
    ));
}
