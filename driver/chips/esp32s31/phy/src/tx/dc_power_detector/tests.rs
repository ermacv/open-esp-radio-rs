use super::*;

#[test]
fn unsupported_pbus_read_fails_closed() {
    assert_eq!(
        require_pbus_result(None),
        Err(PhyTxDcPwdetHardwareInvariant::UnsupportedPbusRead)
    );
}

fn tone_sar_completion(action: PhyToneSarAction, value: u16) -> PhyToneSarCompletion {
    match action {
        PhyToneSarAction::ArmTone {
            measurement,
            sample,
        } => PhyToneSarCompletion::ToneArmed {
            measurement,
            sample,
        },
        PhyToneSarAction::DelayMicros {
            measurement,
            sample,
            phase,
            micros,
        } => PhyToneSarCompletion::DelayElapsed {
            measurement,
            sample,
            phase,
            micros,
        },
        PhyToneSarAction::TriggerSar {
            measurement,
            sample,
        } => PhyToneSarCompletion::SarTriggered {
            measurement,
            sample,
        },
        PhyToneSarAction::PollReady {
            measurement,
            sample,
        } => PhyToneSarCompletion::ReadySampled {
            measurement,
            sample,
            ready: true,
        },
        PhyToneSarAction::ClearTone {
            measurement,
            sample,
        } => PhyToneSarCompletion::ToneCleared {
            measurement,
            sample,
        },
        PhyToneSarAction::ReadSar {
            measurement,
            sample,
        } => PhyToneSarCompletion::SarRead {
            measurement,
            sample,
            value,
        },
        terminal => panic!("unexpected terminal action {terminal:?}"),
    }
}

#[test]
fn child_scan_is_finite_and_commits_only_selected_dco_pair() {
    let initial = [1, 2, 0x100, 0x100];
    let mut transition = PhyTxDcPwdetSearchTransition::new(PhyTxDcPwdetSearchRequest {
        identity: 0,
        initial,
        clear_tone_after_ready: false,
    });
    let mut samples = 0_u16;
    loop {
        let completion = match transition.action() {
            PhyTxDcPwdetSearchAction::ForcePbus(transaction) => {
                PhyTxDcPwdetSearchCompletion::PbusCompleted(transaction)
            }
            PhyTxDcPwdetSearchAction::DelayMicros {
                identity,
                component,
                measurement,
                micros,
            } => PhyTxDcPwdetSearchCompletion::DelayElapsed {
                identity,
                component,
                measurement,
                micros,
            },
            PhyTxDcPwdetSearchAction::ToneSar(action) => {
                if matches!(action, PhyToneSarAction::ReadSar { .. }) {
                    samples += 1;
                }
                PhyTxDcPwdetSearchCompletion::ToneSar(tone_sar_completion(action, 100))
            }
            PhyTxDcPwdetSearchAction::Complete(outcome) => {
                assert_eq!(outcome.dco[0..2], initial[0..2]);
                assert!(outcome.measurements <= 208);
                break;
            }
            PhyTxDcPwdetSearchAction::Failed(failure) => {
                panic!("unexpected failure {failure:?}")
            }
        };
        transition.advance(completion).unwrap();
    }
    assert!(samples <= 416);
}

#[test]
fn bluetooth_mode_reads_and_forces_the_bluetooth_tx_path_before_sar_setup() {
    let mut transition = PhyTxDcPwdetTransition::new_bluetooth(
        PhyTxDcPwdetParameters {
            dco: [[0; 4]; 3],
            clear_tone_after_ready: false,
        },
        0x12,
    );

    loop {
        let completion = match transition.action() {
            PhyTxDcPwdetAction::PrepareRegisters => PhyTxDcPwdetCompletion::RegistersPrepared,
            PhyTxDcPwdetAction::ConfigureTxClock { enabled } => {
                PhyTxDcPwdetCompletion::TxClockConfigured { enabled }
            }
            PhyTxDcPwdetAction::ConfigurePowerDetector => {
                PhyTxDcPwdetCompletion::PowerDetectorConfigured
            }
            PhyTxDcPwdetAction::ConfigurePbusDebugMode => {
                PhyTxDcPwdetCompletion::PbusDebugModeConfigured
            }
            PhyTxDcPwdetAction::ForcePbus(transaction) => {
                PhyTxDcPwdetCompletion::PbusCompleted(transaction)
            }
            PhyTxDcPwdetAction::ConfigureTone {
                enabled,
                selector,
                attenuation,
            } => PhyTxDcPwdetCompletion::ToneConfigured {
                enabled,
                selector,
                attenuation,
            },
            PhyTxDcPwdetAction::DelayMicros { phase, micros } => {
                PhyTxDcPwdetCompletion::DelayElapsed { phase, micros }
            }
            PhyTxDcPwdetAction::ReadPbus { selector, path } => {
                assert_eq!((selector, path), (1, 1));
                transition
                    .advance(PhyTxDcPwdetCompletion::PbusRead {
                        selector,
                        path,
                        value: 0x34,
                    })
                    .unwrap();
                break;
            }
            action => panic!("unexpected Bluetooth prefix action {action:?}"),
        };
        transition.advance(completion).unwrap();
    }

    let forced_path = PhyPbusForceTest::new(1, 1, 0x36);
    assert_eq!(
        transition.action(),
        PhyTxDcPwdetAction::ForcePbus(forced_path)
    );
    transition
        .advance(PhyTxDcPwdetCompletion::PbusCompleted(forced_path))
        .unwrap();

    let forced_tx_path = PhyPbusForceTest::new(4, 2, 0x90);
    assert_eq!(
        transition.action(),
        PhyTxDcPwdetAction::ForcePbus(forced_tx_path)
    );
    transition
        .advance(PhyTxDcPwdetCompletion::PbusCompleted(forced_tx_path))
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyTxDcPwdetAction::ConfigureSarCalibration
    );
}

#[test]
fn root_cleanup_restores_registers_before_terminal_failure() {
    let mut transition = PhyTxDcPwdetTransition::new(PhyTxDcPwdetParameters {
        dco: [[0; 4]; 3],
        clear_tone_after_ready: false,
    });
    let mut inject_initial_failure = true;
    let mut pulse_cleared = false;
    let mut clock_disabled = false;
    let mut registers_restored = false;
    loop {
        let completion = match transition.action() {
            PhyTxDcPwdetAction::PrepareRegisters => PhyTxDcPwdetCompletion::RegistersPrepared,
            PhyTxDcPwdetAction::ConfigureTxClock { enabled } => {
                if !enabled {
                    assert!(pulse_cleared);
                    clock_disabled = true;
                }
                PhyTxDcPwdetCompletion::TxClockConfigured { enabled }
            }
            PhyTxDcPwdetAction::ConfigurePowerDetector => {
                PhyTxDcPwdetCompletion::PowerDetectorConfigured
            }
            PhyTxDcPwdetAction::ConfigurePbusDebugMode => {
                PhyTxDcPwdetCompletion::PbusDebugModeConfigured
            }
            PhyTxDcPwdetAction::ForcePbus(transaction) if inject_initial_failure => {
                inject_initial_failure = false;
                PhyTxDcPwdetCompletion::PbusTimedOut(transaction)
            }
            PhyTxDcPwdetAction::ForcePbus(transaction) => {
                PhyTxDcPwdetCompletion::PbusCompleted(transaction)
            }
            PhyTxDcPwdetAction::ConfigureTone {
                enabled,
                selector,
                attenuation,
            } => PhyTxDcPwdetCompletion::ToneConfigured {
                enabled,
                selector,
                attenuation,
            },
            PhyTxDcPwdetAction::ConfigurePbusWorkMode => {
                PhyTxDcPwdetCompletion::PbusWorkModeConfigured {
                    settle_required: true,
                }
            }
            PhyTxDcPwdetAction::DelayMicros {
                phase: PhyTxDcPwdetDelayPhase::WorkMode,
                micros,
            } => PhyTxDcPwdetCompletion::DelayElapsed {
                phase: PhyTxDcPwdetDelayPhase::WorkMode,
                micros,
            },
            PhyTxDcPwdetAction::ConfigurePbusWorkModePulse => {
                PhyTxDcPwdetCompletion::PbusWorkModePulseConfigured
            }
            PhyTxDcPwdetAction::DelayMicros {
                phase: PhyTxDcPwdetDelayPhase::WorkModePulse,
                micros,
            } => {
                assert_eq!(micros, 2);
                PhyTxDcPwdetCompletion::DelayElapsed {
                    phase: PhyTxDcPwdetDelayPhase::WorkModePulse,
                    micros,
                }
            }
            PhyTxDcPwdetAction::ClearPbusWorkModePulse => {
                pulse_cleared = true;
                PhyTxDcPwdetCompletion::PbusWorkModePulseCleared
            }
            PhyTxDcPwdetAction::RestoreRegisters => {
                assert!(clock_disabled);
                registers_restored = true;
                PhyTxDcPwdetCompletion::RegistersRestored
            }
            PhyTxDcPwdetAction::Failed(PhyTxDcPwdetFailure::PbusTimedOut(_)) => {
                assert!(registers_restored);
                break;
            }
            action => panic!("unexpected cleanup action {action:?}"),
        };
        transition.advance(completion).unwrap();
    }
}

#[test]
fn external_lowering_covers_root_and_search_operation_classes() {
    let transaction = PhyPbusForceTest::new(1, 2, 0x80);
    assert!(matches!(
        PhyTxDcPwdetExternalBinding::lower(PhyTxDcPwdetAction::PrepareRegisters),
        Ok(PhyTxDcPwdetExternalBinding::Mmio(_))
    ));
    assert!(matches!(
        PhyTxDcPwdetExternalBinding::lower(PhyTxDcPwdetAction::ForcePbus(transaction)),
        Ok(PhyTxDcPwdetExternalBinding::Pbus(_))
    ));
    assert!(matches!(
        PhyTxDcPwdetExternalBinding::lower(PhyTxDcPwdetAction::DelayMicros {
            phase: PhyTxDcPwdetDelayPhase::InitialTone,
            micros: 1,
        }),
        Ok(PhyTxDcPwdetExternalBinding::Timer(_))
    ));
    assert!(matches!(
        PhyTxDcPwdetExternalBinding::lower(PhyTxDcPwdetAction::Search(
            PhyTxDcPwdetSearchAction::ForcePbus(transaction)
        )),
        Ok(PhyTxDcPwdetExternalBinding::Search(
            PhyTxDcPwdetSearchExternalBinding::Pbus(_)
        ))
    ));
    assert!(matches!(
        PhyTxDcPwdetSearchExternalBinding::lower(PhyTxDcPwdetSearchAction::DelayMicros {
            identity: 1,
            component: 2,
            measurement: 3,
            micros: 2,
        }),
        Ok(PhyTxDcPwdetSearchExternalBinding::Timer(_))
    ));
    assert!(matches!(
        PhyTxDcPwdetSearchExternalBinding::lower(PhyTxDcPwdetSearchAction::ToneSar(
            PhyToneSarAction::ClearTone {
                measurement: 0,
                sample: 0,
            }
        )),
        Ok(PhyTxDcPwdetSearchExternalBinding::ToneSar(_))
    ));
    assert!(matches!(
        PhyTxDcPwdetExternalBinding::lower(PhyTxDcPwdetAction::Complete(PhyTxDcPwdetOutcome {
            dco: [[0; 4]; 3],
            total_measurements: 0,
        })),
        Err(PhyTxDcPwdetExternalBindingError::UnsupportedAction)
    ));
}
