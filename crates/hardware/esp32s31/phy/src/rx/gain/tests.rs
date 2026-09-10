use super::*;
use std::vec::Vec;

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
fn recalibration_publishes_fresh_rxbb_corrections_only_in_wifi_bank() {
    fn publish(old: u16, fresh: u16) -> Vec<(PhyRxGainBank, PhyGainMemoryEntry)> {
        let mut parameters = init_parameters();
        parameters.memory.rxbb_dc_adjustments = [[old; 2]; 6];
        let outcome = PhyRxGainDcOutcome {
            wifi_index_dc: [[0x100; 2]; 8],
            wifi_dc_base: [0x100; 2],
            shared_index_dc: [[0x100; 2]; 11],
            rxbb_dc_adjustments: [[fresh, fresh.wrapping_neg()]; 6],
        };
        let mut root = PhyRxGainInitTransition::new(parameters);
        // Exercise the real handoff after a completed measurement, including
        // outer control restoration and every table publication action.
        root.step = InitStep::RestoreDcControl { outcome };
        root.advance(PhyRxGainInitCompletion::DcControlRestored)
            .unwrap();
        let mut entries = Vec::new();
        loop {
            match root.action() {
                PhyRxGainInitAction::Publish(action) => {
                    if let PhyRxGainPublishAction::ProgramEntry { bank, entry } = action {
                        entries.push((bank, entry));
                    }
                    root.advance(PhyRxGainInitCompletion::Publish(complete(action)))
                        .unwrap();
                }
                PhyRxGainInitAction::ConfigureLimits { wifi_last_index } => root
                    .advance(PhyRxGainInitCompletion::LimitsConfigured { wifi_last_index })
                    .unwrap(),
                PhyRxGainInitAction::EnableIqCorrection => root
                    .advance(PhyRxGainInitCompletion::IqCorrectionEnabled)
                    .unwrap(),
                PhyRxGainInitAction::Complete(result) => {
                    assert_eq!(result.dc, Some(outcome));
                    return entries;
                }
                other => panic!("unexpected publication action: {other:?}"),
            }
        }
    }

    let fresh = publish(0, 3);
    assert_eq!(
        fresh,
        publish(19, 3),
        "old corrections must not leak into publication"
    );
    let changed = publish(0, 7);
    for bank in [PhyRxGainBank::Wifi, PhyRxGainBank::Shared] {
        let entries = |values: &[(PhyRxGainBank, PhyGainMemoryEntry)]| {
            values
                .iter()
                .filter(|(entry_bank, _)| *entry_bank == bank)
                .map(|(_, entry)| *entry)
                .collect::<Vec<_>>()
        };
        assert!(!entries(&fresh).is_empty());
        match bank {
            PhyRxGainBank::Wifi => assert_ne!(entries(&fresh), entries(&changed)),
            PhyRxGainBank::Shared => assert_eq!(entries(&fresh), entries(&changed)),
        }
    }
}

#[test]
fn complete_publisher_emits_72_wifi_and_76_shared_entries() {
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
                assert_eq!(outcome.wifi_entries, 72);
                assert_eq!(outcome.shared_entries, 76);
                break;
            }
            _ => {}
        }
        transition.advance(complete(action)).unwrap();
    }
    assert_eq!(wifi_entries, 72);
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

fn publish_root(parameters: PhyRxGainInitParameters) -> (PhyRxGainInitOutcome, usize) {
    let mut root = PhyRxGainInitTransition::new(parameters);
    let mut steps = 0;
    loop {
        let action = core::hint::black_box(&root).action();
        let completion = match action {
            PhyRxGainInitAction::Publish(action) => {
                PhyRxGainInitCompletion::Publish(complete(action))
            }
            PhyRxGainInitAction::ConfigureLimits { wifi_last_index } => {
                PhyRxGainInitCompletion::LimitsConfigured { wifi_last_index }
            }
            PhyRxGainInitAction::EnableIqCorrection => PhyRxGainInitCompletion::IqCorrectionEnabled,
            PhyRxGainInitAction::Complete(outcome) => return (outcome, steps),
            other => panic!("unexpected publish-only action: {other:?}"),
        };
        core::hint::black_box(&mut root)
            .advance(core::hint::black_box(completion))
            .unwrap();
        steps += 1;
    }
}

#[test]
fn root_publishes_both_banks_and_retains_state_on_rejected_completion() {
    let mut parameters = init_parameters();
    parameters.dc_calibrated = true;
    let mut root = PhyRxGainInitTransition::new(parameters);
    loop {
        let action = root.action();
        let PhyRxGainInitAction::Publish(action) = action else {
            break;
        };
        let before = root;
        assert_eq!(
            root.advance(PhyRxGainInitCompletion::Publish(
                PhyRxGainPublishCompletion::DelayElapsed {
                    phase: PhyRxGainDelayPhase::PbusWorkMode {
                        bank: PhyRxGainBank::Wifi
                    },
                    micros: 0,
                }
            )),
            Err(PhyRxGainInitTransitionError::WrongCompletion)
        );
        assert_eq!(root, before);
        if let PhyRxGainPublishAction::ProgramEntry { bank, entry } = action {
            let wrong_bank = match bank {
                PhyRxGainBank::Wifi => PhyRxGainBank::Shared,
                PhyRxGainBank::Shared => PhyRxGainBank::Wifi,
            };
            assert_eq!(
                root.advance(PhyRxGainInitCompletion::Publish(
                    PhyRxGainPublishCompletion::EntryProgrammed {
                        bank: wrong_bank,
                        entry
                    }
                )),
                Err(PhyRxGainInitTransitionError::WrongCompletion)
            );
            assert_eq!(root, before);
        }

        root.advance(PhyRxGainInitCompletion::Publish(complete(action)))
            .unwrap();
    }
    assert!(matches!(
        root.action(),
        PhyRxGainInitAction::ConfigureLimits { .. }
    ));
    let (outcome, _) = publish_root(parameters);
    assert!(outcome.generated_tables);
    assert_eq!(
        (outcome.wifi_last_index, outcome.shared_last_index),
        (71, 75)
    );
}

/// Host CPU cost only: no MMIO, waits or radio-quality claim.
#[test]
#[ignore = "manual release-mode CPU benchmark of the production publisher"]
fn benchmark_rx_gain_publication() {
    for sample in 0..3 {
        let start = std::time::Instant::now();
        let mut steps = 0;
        for iteration in 0..20_000 {
            let mut parameters = init_parameters();
            parameters.dc_calibrated = true;
            parameters.memory.parameter_002 ^= iteration as u8;
            let (outcome, count) = publish_root(core::hint::black_box(parameters));
            core::hint::black_box(outcome);
            steps += count;
        }
        std::println!(
            "sample={sample} iterations=20000 steps={steps} elapsed_ns={}",
            start.elapsed().as_nanos()
        );
    }
}

#[test]
fn root_preserves_publish_failure_through_cleanup() {
    let mut parameters = init_parameters();
    parameters.dc_calibrated = true;
    let mut root = PhyRxGainInitTransition::new(parameters);
    let PhyRxGainInitAction::Publish(action) = root.action() else {
        panic!("publisher")
    };
    root.advance(PhyRxGainInitCompletion::Publish(complete(action)))
        .unwrap();
    let PhyRxGainInitAction::Publish(PhyRxGainPublishAction::ForcePbus { bank, transaction }) =
        root.action()
    else {
        panic!("PBus transaction")
    };
    root.advance(PhyRxGainInitCompletion::Publish(
        PhyRxGainPublishCompletion::PbusTimedOut { bank, transaction },
    ))
    .unwrap();
    assert_eq!(
        root.action(),
        PhyRxGainInitAction::Publish(PhyRxGainPublishAction::ConfigurePbusWorkMode { bank })
    );
    root.advance(PhyRxGainInitCompletion::Publish(complete(
        PhyRxGainPublishAction::ConfigurePbusWorkMode { bank },
    )))
    .unwrap();
    assert_eq!(
        root.action(),
        PhyRxGainInitAction::Failed(PhyRxGainInitFailure::Publish(
            PhyRxGainPublishFailure::PbusTimedOut { bank, transaction }
        ))
    );
}
