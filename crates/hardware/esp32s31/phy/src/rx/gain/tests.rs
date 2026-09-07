use super::*;

fn parameters() -> PhyRxGainMemoryParameters {
    PhyRxGainMemoryParameters {
        parameter_002: 0xbf,
        wifi_index_dc: [[0x100; 2]; 8],
        wifi_dc_base: [0x100; 2],
        shared_index_dc: [[0x100; 2]; 11],
        rxbb_dc_adjustments: [[0; 2]; 6],
        wifi_auxiliary: 0,
    }
}

fn init_parameters() -> PhyRxGainInitParameters {
    PhyRxGainInitParameters {
        dc_calibrated: false,
        tables_initialized: false,
        dc: PhyRxGainDcParameters {
            crystal_selector: 0,
            pbus_rx_path_value: 0,
            rx_saturation_detected: false,
        },
        memory: parameters(),
    }
}

fn complete(action: PhyRxGainPublishAction) -> PhyRxGainPublishCompletion {
    match action {
        PhyRxGainPublishAction::ConfigurePbusDebugMode { bank } => {
            PhyRxGainPublishCompletion::PbusDebugModeConfigured { bank }
        }
        PhyRxGainPublishAction::ForcePbus { bank, transaction } => {
            PhyRxGainPublishCompletion::PbusCompleted { bank, transaction }
        }
        PhyRxGainPublishAction::ConfigureClock {
            bank,
            clock,
            enabled,
        } => PhyRxGainPublishCompletion::ClockConfigured {
            bank,
            clock,
            enabled,
        },
        PhyRxGainPublishAction::ProgramEntry { bank, entry } => {
            PhyRxGainPublishCompletion::EntryProgrammed { bank, entry }
        }
        PhyRxGainPublishAction::ConfigurePbusWorkMode { bank } => {
            PhyRxGainPublishCompletion::PbusWorkModeConfigured {
                bank,
                settle_required: false,
            }
        }
        PhyRxGainPublishAction::ConfigurePbusWorkModePulse { bank } => {
            PhyRxGainPublishCompletion::PbusWorkModePulseConfigured { bank }
        }
        PhyRxGainPublishAction::ClearPbusWorkModePulse { bank } => {
            PhyRxGainPublishCompletion::PbusWorkModePulseCleared { bank }
        }
        PhyRxGainPublishAction::DelayMicros { phase, micros } => {
            PhyRxGainPublishCompletion::DelayElapsed { phase, micros }
        }
        PhyRxGainPublishAction::Complete(_) | PhyRxGainPublishAction::Failed(_) => {
            panic!("terminal action")
        }
    }
}

#[test]
fn complete_publisher_emits_70_wifi_and_76_shared_entries() {
    let mut transition = PhyRxGainPublishTransition::new(parameters());
    let mut wifi_entries = 0;
    let mut shared_entries = 0;
    loop {
        let action = transition.action();
        match action {
            PhyRxGainPublishAction::ProgramEntry {
                bank: PhyRxGainBank::Wifi,
                ..
            } => wifi_entries += 1,
            PhyRxGainPublishAction::ProgramEntry {
                bank: PhyRxGainBank::Shared,
                ..
            } => shared_entries += 1,
            PhyRxGainPublishAction::Complete(outcome) => {
                assert_eq!(outcome.wifi_entries, 70);
                assert_eq!(outcome.shared_entries, 76);
                break;
            }
            _ => {}
        }
        transition.advance(complete(action)).unwrap();
    }
    assert_eq!(wifi_entries, 70);
    assert_eq!(shared_entries, 76);
}

#[test]
fn pbus_failure_is_terminal_and_preserves_operation_identity() {
    let mut transition = PhyRxGainPublishTransition::new(parameters());
    transition
        .advance(PhyRxGainPublishCompletion::PbusDebugModeConfigured {
            bank: PhyRxGainBank::Wifi,
        })
        .unwrap();
    let PhyRxGainPublishAction::ForcePbus { bank, transaction } = transition.action() else {
        panic!("expected PBus");
    };
    transition
        .advance(PhyRxGainPublishCompletion::PbusTimedOut { bank, transaction })
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyRxGainPublishAction::ConfigurePbusWorkMode { bank }
    );
    transition
        .advance(PhyRxGainPublishCompletion::PbusWorkModeConfigured {
            bank,
            settle_required: false,
        })
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyRxGainPublishAction::Failed(PhyRxGainPublishFailure::PbusTimedOut { bank, transaction })
    );
}

#[test]
fn nested_dc_failure_restores_outer_control_before_becoming_terminal() {
    let failure = PhyRxGainDcFailure::Pbus {
        bank: crate::rx::gain_calibration::PhyRxGainDcBank::Wifi,
        transaction: crate::analog::pbus::PhyPbusForceTest::new(2, 1, 0x100),
    };
    let mut transition = PhyRxGainInitTransition::new(init_parameters());
    transition.step = InitStep::RestoreDcControlAfterFailure(failure);

    assert_eq!(transition.action(), PhyRxGainInitAction::RestoreDcControl);
    transition
        .advance(PhyRxGainInitCompletion::DcControlRestored)
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyRxGainInitAction::Failed(PhyRxGainInitFailure::Dc(failure))
    );
}

#[test]
fn work_mode_pulse_uses_the_vendor_two_microsecond_delay() {
    let mut transition = PhyRxGainPublishTransition::new(parameters());
    transition.step = Step::WorkModePulseDelay {
        bank: PhyRxGainBank::Wifi,
        failure: None,
    };
    assert_eq!(
        transition.action(),
        PhyRxGainPublishAction::DelayMicros {
            phase: PhyRxGainDelayPhase::PbusWorkModePulse {
                bank: PhyRxGainBank::Wifi,
            },
            micros: 2,
        }
    );
    assert_eq!(
        transition.advance(PhyRxGainPublishCompletion::DelayElapsed {
            phase: PhyRxGainDelayPhase::PbusWorkModePulse {
                bank: PhyRxGainBank::Wifi,
            },
            micros: 1,
        }),
        Err(PhyRxGainPublishTransitionError::WrongCompletion)
    );
}
