use super::{
    CAP_SEARCH_LIMIT, RfpllCapCorrectionAction, RfpllCapCorrectionBindingError,
    RfpllCapCorrectionCompletion, RfpllCapCorrectionDirection, RfpllCapCorrectionExternalBinding,
    RfpllCapCorrectionRequest, RfpllCapCorrectionTransition, RfpllCapCorrectionTransitionError,
    RfpllCapTrackingAction, RfpllCapTrackingBindingError, RfpllCapTrackingCompletion,
    RfpllCapTrackingExternalBinding, RfpllCapTrackingParameters, RfpllCapTrackingTransition,
    RfpllCapTrackingTransitionError, RfpllFrequencyAction, RfpllFrequencyBindingError,
    RfpllFrequencyCompletion, RfpllFrequencyExternalBinding, RfpllFrequencyI2cBinding,
    RfpllFrequencyOutcome, RfpllFrequencyRequest, RfpllFrequencyTransition, calculate_rfpll_sdm,
};
use crate::analog::i2c::analog_registers;

fn cap_read_completion(
    action: RfpllCapCorrectionAction,
    value: u8,
) -> RfpllCapCorrectionCompletion {
    match action {
        RfpllCapCorrectionAction::ReadMasked { field } => {
            RfpllCapCorrectionCompletion::MaskedRead { field, value }
        }
        RfpllCapCorrectionAction::ReadByte { address } => {
            RfpllCapCorrectionCompletion::ByteRead { address, value }
        }
        action => panic!("expected cap read action, got {action:?}"),
    }
}

fn cap_write_completion(action: RfpllCapCorrectionAction) -> RfpllCapCorrectionCompletion {
    match action {
        RfpllCapCorrectionAction::WriteMasked { field, .. } => {
            RfpllCapCorrectionCompletion::MaskedWrite { field }
        }
        RfpllCapCorrectionAction::WriteByte { address, .. } => {
            RfpllCapCorrectionCompletion::ByteWrite { address }
        }
        action => panic!("expected cap write action, got {action:?}"),
    }
}

#[test]
fn cap_tracking_threshold_uses_inclusive_default_and_override_boundaries() {
    let base = RfpllCapTrackingParameters {
        current_temperature: 24,
        reference_temperature: 20,
        threshold_override: None,
        current_channel: 11,
    };
    let RfpllCapTrackingAction::Complete(outcome) = RfpllCapTrackingTransition::new(base).action()
    else {
        panic!("four-degree default delta must be skipped");
    };
    assert_eq!(outcome.threshold, 5);
    assert!(!outcome.updated);

    assert_eq!(
        RfpllCapTrackingTransition::new(RfpllCapTrackingParameters {
            current_temperature: 25,
            ..base
        })
        .action(),
        RfpllCapTrackingAction::SetHardwareFrequencyControl { enabled: false }
    );

    let overridden = RfpllCapTrackingParameters {
        current_temperature: 10,
        reference_temperature: 20,
        threshold_override: Some(10),
        current_channel: 11,
    };
    assert_eq!(
        RfpllCapTrackingTransition::new(overridden).action(),
        RfpllCapTrackingAction::SetHardwareFrequencyControl { enabled: false }
    );
}

#[test]
fn cap_tracking_owns_disable_correction_enable_and_reference_outcome() {
    let mut transition = RfpllCapTrackingTransition::new(RfpllCapTrackingParameters {
        current_temperature: 25,
        reference_temperature: 20,
        threshold_override: None,
        current_channel: 11,
    });
    assert!(matches!(
        RfpllCapTrackingExternalBinding::lower(transition.action()),
        Ok(RfpllCapTrackingExternalBinding::Mmio(_))
    ));
    transition
        .advance(RfpllCapTrackingCompletion::HardwareFrequencyControlSet { enabled: false })
        .unwrap();
    assert!(matches!(
        RfpllCapTrackingExternalBinding::lower(transition.action()),
        Ok(RfpllCapTrackingExternalBinding::Correction(
            RfpllCapCorrectionExternalBinding::I2c(_)
        ))
    ));
    let RfpllCapTrackingAction::Correct(action) = transition.action() else {
        panic!("tracking did not enter correction");
    };
    transition
        .advance(RfpllCapTrackingCompletion::Correction(cap_read_completion(
            action, 0,
        )))
        .unwrap();
    assert_eq!(
        transition.action(),
        RfpllCapTrackingAction::SetHardwareFrequencyControl { enabled: true }
    );
    transition
        .advance(RfpllCapTrackingCompletion::HardwareFrequencyControlSet { enabled: true })
        .unwrap();

    let RfpllCapTrackingAction::Complete(outcome) = transition.action() else {
        panic!("tracking did not complete");
    };
    assert!(outcome.updated);
    assert_eq!(outcome.previous_reference_temperature, 20);
    assert_eq!(outcome.reference_temperature, 25);
    assert_eq!(
        outcome.correction.unwrap().direction,
        RfpllCapCorrectionDirection::StableZero
    );
    assert_eq!(
        RfpllCapTrackingExternalBinding::lower(transition.action()),
        Err(RfpllCapTrackingBindingError::UnsupportedAction)
    );
}

#[test]
fn cap_tracking_rejects_an_enable_completion_before_the_disable_edge() {
    let mut transition = RfpllCapTrackingTransition::new(RfpllCapTrackingParameters {
        current_temperature: 25,
        reference_temperature: 20,
        threshold_override: None,
        current_channel: 11,
    });
    assert_eq!(
        transition
            .advance(RfpllCapTrackingCompletion::HardwareFrequencyControlSet { enabled: true }),
        Err(RfpllCapTrackingTransitionError::WrongCompletion)
    );
}

#[test]
fn cap_correction_stable_directions_stop_after_one_field_read() {
    for (field, expected) in [
        (0, RfpllCapCorrectionDirection::StableZero),
        (3, RfpllCapCorrectionDirection::StableThree),
    ] {
        let mut transition = RfpllCapCorrectionTransition::new(RfpllCapCorrectionRequest {
            current_channel: 11,
        });
        assert!(matches!(
            RfpllCapCorrectionExternalBinding::lower(transition.action()),
            Ok(RfpllCapCorrectionExternalBinding::I2c(_))
        ));
        transition
            .advance(cap_read_completion(transition.action(), field))
            .unwrap();
        let RfpllCapCorrectionAction::Complete(outcome) = transition.action() else {
            panic!("stable direction did not complete");
        };
        assert_eq!(outcome.direction, expected);
        assert_eq!(outcome.update, None);
        assert_eq!(
            RfpllCapCorrectionExternalBinding::lower(transition.action()),
            Err(RfpllCapCorrectionBindingError::UnsupportedAction)
        );
    }
}

#[test]
fn cap_correction_increase_composes_cap_write_and_all_memory_updates() {
    let mut transition = RfpllCapCorrectionTransition::new(RfpllCapCorrectionRequest {
        current_channel: 11,
    });
    for value in [1, 0xfe, 0] {
        transition
            .advance(cap_read_completion(transition.action(), value))
            .unwrap();
    }
    assert!(matches!(
        transition.action(),
        RfpllCapCorrectionAction::WriteByte { value: 0, .. }
    ));
    transition
        .advance(cap_write_completion(transition.action()))
        .unwrap();
    assert!(matches!(
        transition.action(),
        RfpllCapCorrectionAction::WriteMasked { value: 1, .. }
    ));
    transition
        .advance(cap_write_completion(transition.action()))
        .unwrap();

    let mut reads = 0_u8;
    let mut writes = 0_u8;
    while let RfpllCapCorrectionAction::Memory(action) = transition.action() {
        assert!(matches!(
            RfpllCapCorrectionExternalBinding::lower(RfpllCapCorrectionAction::Memory(action)),
            Ok(RfpllCapCorrectionExternalBinding::Memory(_))
        ));
        let completion = match action {
            crate::analog::frequency::PhyFrequencyCapMemoryAction::ReadMemory {
                entry_index,
                address,
                mode,
            } => {
                reads += 1;
                crate::analog::frequency::PhyFrequencyCapMemoryCompletion::MemoryRead {
                    entry_index,
                    address,
                    mode,
                    value: 0x0000_bf20,
                }
            }
            crate::analog::frequency::PhyFrequencyCapMemoryAction::WriteMemory {
                entry_index,
                address,
                value,
                mode,
            } => {
                assert_eq!(value, 0x0000_bf22);
                writes += 1;
                crate::analog::frequency::PhyFrequencyCapMemoryCompletion::MemoryWritten {
                    entry_index,
                    address,
                    mode,
                }
            }
            crate::analog::frequency::PhyFrequencyCapMemoryAction::RestoreChannelIndex {
                frequency_index,
            } => crate::analog::frequency::PhyFrequencyCapMemoryCompletion::ChannelIndexRestored {
                frequency_index,
            },
            crate::analog::frequency::PhyFrequencyCapMemoryAction::Complete(_) => {
                panic!("nested terminal action escaped correction owner")
            }
        };
        transition
            .advance(RfpllCapCorrectionCompletion::Memory(completion))
            .unwrap();
    }

    assert_eq!(reads, 85);
    assert_eq!(writes, 85);
    let RfpllCapCorrectionAction::Complete(outcome) = transition.action() else {
        panic!("correction did not complete");
    };
    assert_eq!(outcome.direction, RfpllCapCorrectionDirection::IncreaseTwo);
    let update = outcome.update.unwrap();
    assert_eq!(update.previous_cap, 0xfe);
    assert_eq!(update.requested_cap, 0x100);
    assert_eq!(update.programmed_cap, 0x100);
    assert_eq!(update.memory.entries_updated, 85);
}

#[test]
fn cap_correction_decrease_clips_only_the_live_negative_cap_write() {
    let mut transition =
        RfpllCapCorrectionTransition::new(RfpllCapCorrectionRequest { current_channel: 1 });
    for value in [2, 1, 0] {
        transition
            .advance(cap_read_completion(transition.action(), value))
            .unwrap();
    }
    assert!(matches!(
        transition.action(),
        RfpllCapCorrectionAction::WriteByte { value: 0, .. }
    ));
    transition
        .advance(cap_write_completion(transition.action()))
        .unwrap();
    assert!(matches!(
        transition.action(),
        RfpllCapCorrectionAction::WriteMasked { value: 0, .. }
    ));
}

#[test]
fn cap_correction_rejects_a_completion_for_another_reviewed_field() {
    let mut transition = RfpllCapCorrectionTransition::new(RfpllCapCorrectionRequest {
        current_channel: 11,
    });
    assert_eq!(
        transition.advance(RfpllCapCorrectionCompletion::MaskedRead {
            field: super::analog_registers::RFPLL_LOCK_STATUS,
            value: 1,
        }),
        Err(RfpllCapCorrectionTransitionError::WrongCompletion)
    );
}

fn complete_write(action: RfpllFrequencyAction) -> RfpllFrequencyCompletion {
    match action {
        RfpllFrequencyAction::WriteMasked { field, .. } => {
            RfpllFrequencyCompletion::MaskedWrite { field }
        }
        RfpllFrequencyAction::WriteByte { address, .. } => {
            RfpllFrequencyCompletion::ByteWrite { address }
        }
        action => panic!("expected write action, got {action:?}"),
    }
}

fn advance_writes(transition: &mut RfpllFrequencyTransition, count: usize) {
    let mut index = 0;
    while index != count {
        let completion = complete_write(transition.action());
        transition.advance(completion).unwrap();
        index += 1;
    }
}

fn enter_cap_search(transition: &mut RfpllFrequencyTransition, low: u8, high: u8) {
    advance_writes(transition, 13);
    assert_eq!(transition.action(), RfpllFrequencyAction::DelayMicros(20));
    transition
        .advance(RfpllFrequencyCompletion::DelayElapsed(20))
        .unwrap();
    let RfpllFrequencyAction::ReadMasked { field } = transition.action() else {
        panic!("expected lock read");
    };
    transition
        .advance(RfpllFrequencyCompletion::MaskedRead { field, value: 1 })
        .unwrap();

    let RfpllFrequencyAction::ReadByte { address } = transition.action() else {
        panic!("expected cap low read");
    };
    transition
        .advance(RfpllFrequencyCompletion::ByteRead {
            address,
            value: low,
        })
        .unwrap();
    let RfpllFrequencyAction::ReadMasked { field } = transition.action() else {
        panic!("expected cap high read");
    };
    transition
        .advance(RfpllFrequencyCompletion::MaskedRead { field, value: high })
        .unwrap();
    let completion = complete_write(transition.action());
    transition.advance(completion).unwrap();
}

fn complete_cap_candidate(transition: &mut RfpllFrequencyTransition, status: u8) {
    advance_writes(transition, 2);
    assert_eq!(transition.action(), RfpllFrequencyAction::DelayMicros(5));
    transition
        .advance(RfpllFrequencyCompletion::DelayElapsed(5))
        .unwrap();
    let RfpllFrequencyAction::ReadMasked { field } = transition.action() else {
        panic!("expected cap status read");
    };
    transition
        .advance(RfpllFrequencyCompletion::MaskedRead {
            field,
            value: status,
        })
        .unwrap();
}

#[test]
fn sdm_image_matches_the_actual_xtal_duty_request() {
    assert_eq!(
        calculate_rfpll_sdm(0x983, 0x31, 0).bytes(),
        [0x05, 0xaa, 0x2a, 0x31, 0x00]
    );
    assert_eq!(
        calculate_rfpll_sdm(0x0fa1, 1, 7).bytes(),
        [0x01, 0xe8, 0x30, 0x3b, 0x00]
    );
}

#[test]
fn lock_deadline_is_one_hundred_external_delay_and_read_edges() {
    let mut transition = RfpllFrequencyTransition::new(RfpllFrequencyRequest {
        crystal_selector: 0x31,
        frequency_code: 0x983,
        offset: 0,
    });
    advance_writes(&mut transition, 13);

    let mut attempts = 0;
    while attempts != 100 {
        assert_eq!(transition.action(), RfpllFrequencyAction::DelayMicros(20));
        transition
            .advance(RfpllFrequencyCompletion::DelayElapsed(20))
            .unwrap();
        let RfpllFrequencyAction::ReadMasked { field } = transition.action() else {
            panic!("expected lock read");
        };
        transition
            .advance(RfpllFrequencyCompletion::MaskedRead { field, value: 0 })
            .unwrap();
        attempts += 1;
    }
    assert!(matches!(
        transition.action(),
        RfpllFrequencyAction::ReadByte { .. }
    ));
}

#[test]
fn capacitor_search_preserves_shared_offset_sum_and_first_match_order() {
    let mut transition = RfpllFrequencyTransition::new(RfpllFrequencyRequest {
        crystal_selector: 0x31,
        frequency_code: 0x983,
        offset: 0,
    });
    enter_cap_search(&mut transition, 100, 0);

    complete_cap_candidate(&mut transition, 0);
    complete_cap_candidate(&mut transition, 0);
    complete_cap_candidate(&mut transition, 1);
    complete_cap_candidate(&mut transition, 0);
    complete_cap_candidate(&mut transition, 1);

    advance_writes(&mut transition, 2);
    transition
        .advance(RfpllFrequencyCompletion::DelayElapsed(5))
        .unwrap();
    let RfpllFrequencyAction::Complete(outcome) = transition.action() else {
        panic!("expected completion");
    };
    assert_eq!(outcome.initial_cap, 100);
    assert_eq!(outcome.final_cap, (100 + 99 + 103) / 3);
    assert_eq!(outcome.accepted_cap_samples, 3);
    assert!(outcome.lock_observed);
}

#[test]
fn bounded_rom_cap_path_preserves_initial_when_no_sample_is_accepted() {
    let mut transition = RfpllFrequencyTransition::new(RfpllFrequencyRequest {
        crystal_selector: 0x31,
        frequency_code: 0x983,
        offset: 0,
    });
    enter_cap_search(&mut transition, 100, 0);
    let mut index = 0;
    while index != CAP_SEARCH_LIMIT * 2 {
        complete_cap_candidate(&mut transition, 1);
        index += 1;
    }
    advance_writes(&mut transition, 2);
    transition
        .advance(RfpllFrequencyCompletion::DelayElapsed(5))
        .unwrap();
    let RfpllFrequencyAction::Complete(outcome) = transition.action() else {
        panic!("expected completion");
    };
    assert_eq!(outcome.initial_cap, 100);
    assert_eq!(outcome.final_cap, 100);
    assert_eq!(outcome.accepted_cap_samples, 0);
}

#[test]
fn wifi_channel_uses_the_rom_fast_switch_without_rfpll_i2c() {
    let mut transition = RfpllFrequencyTransition::new(RfpllFrequencyRequest {
        crystal_selector: 0,
        frequency_code: 1,
        offset: 0,
    });
    assert_eq!(
        transition.action(),
        RfpllFrequencyAction::StartChannelSwitch {
            frequency_index: 12,
            crystal_selector: 0,
        }
    );
    transition
        .advance(RfpllFrequencyCompletion::ChannelSwitchStarted {
            frequency_index: 12,
            crystal_selector: 0,
        })
        .unwrap();
    assert_eq!(transition.action(), RfpllFrequencyAction::DelayMicros(1));
    transition
        .advance(RfpllFrequencyCompletion::DelayElapsed(1))
        .unwrap();
    assert_eq!(
        transition.action(),
        RfpllFrequencyAction::ClearChannelSwitch
    );
    transition
        .advance(RfpllFrequencyCompletion::ChannelSwitchCleared)
        .unwrap();
    assert_eq!(transition.action(), RfpllFrequencyAction::DelayMicros(10));
    transition
        .advance(RfpllFrequencyCompletion::DelayElapsed(10))
        .unwrap();
    assert_eq!(
        transition.action(),
        RfpllFrequencyAction::ReadChannelReady { samples: 0 }
    );
    transition
        .advance(RfpllFrequencyCompletion::ChannelReadyObserved { ready: true })
        .unwrap();
    assert_eq!(
        transition.action(),
        RfpllFrequencyAction::ConfigureNrx {
            frequency_mhz: 2_412,
        }
    );
    transition
        .advance(RfpllFrequencyCompletion::NrxConfigured {
            frequency_mhz: 2_412,
        })
        .unwrap();
    let RfpllFrequencyAction::Complete(outcome) = transition.action() else {
        panic!("expected fast-channel completion");
    };
    assert!(outcome.lock_observed);
    assert_eq!(outcome.accepted_cap_samples, 0);
}

#[test]
fn external_lowering_covers_rfpll_mmio_i2c_and_timer_actions() {
    assert!(matches!(
        RfpllFrequencyExternalBinding::lower(RfpllFrequencyAction::StartChannelSwitch {
            frequency_index: 12,
            crystal_selector: 0,
        }),
        Ok(RfpllFrequencyExternalBinding::Mmio(_))
    ));
    assert!(matches!(
        RfpllFrequencyExternalBinding::lower(RfpllFrequencyAction::ReadMasked {
            field: super::analog_registers::RFPLL_LOCK_STATUS,
        }),
        Ok(RfpllFrequencyExternalBinding::I2c(_))
    ));
    assert!(matches!(
        RfpllFrequencyExternalBinding::lower(RfpllFrequencyAction::WriteByte {
            address: analog_registers::RFPLL_LOCK_STATUS.address(),
            value: 3,
        }),
        Ok(RfpllFrequencyExternalBinding::I2c(_))
    ));
    let timer =
        RfpllFrequencyExternalBinding::lower(RfpllFrequencyAction::DelayMicros(20)).unwrap();
    let RfpllFrequencyExternalBinding::Timer(timer) = timer else {
        panic!("expected timer");
    };
    assert_eq!(timer.micros(), 20);
    assert_eq!(
        timer.into_completion(),
        RfpllFrequencyCompletion::DelayElapsed(20)
    );
    assert!(matches!(
        RfpllFrequencyExternalBinding::lower(RfpllFrequencyAction::Complete(
            RfpllFrequencyOutcome {
                sdm: calculate_rfpll_sdm(0x983, 0x31, 0),
                lock_observed: true,
                initial_cap: 1,
                final_cap: 1,
                accepted_cap_samples: 1,
            }
        )),
        Err(RfpllFrequencyBindingError::UnsupportedAction)
    ));
}

#[test]
fn rfpll_i2c_binding_preserves_the_masked_read_identity() {
    let field = super::analog_registers::RFPLL_CAPACITOR_CORRECTION_DIRECTION;
    let mut binding =
        RfpllFrequencyI2cBinding::new(RfpllFrequencyAction::ReadMasked { field }).unwrap();
    binding.read_started().unwrap();
    assert_eq!(
        binding.observe_read_result(Ok(0b0100)).unwrap(),
        crate::calibration::cold::PhyColdI2cObservation::EdgeConsumed
    );
    assert_eq!(
        binding.into_completion().unwrap(),
        RfpllFrequencyCompletion::MaskedRead { field, value: 1 }
    );
}
