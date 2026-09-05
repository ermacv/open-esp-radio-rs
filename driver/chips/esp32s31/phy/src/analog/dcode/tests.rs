use super::*;
use crate::analog::rfpll::RfpllFrequencyAction;

#[test]
fn first_nested_rfpll_request_owns_the_exact_rom_frequency() {
    let transition = PhyDcodeTransition::new(PhyDcodeParameters {
        crystal_selector: 0x31,
    });
    let PhyDcodeAction::Rfpll(action) = transition.action() else {
        panic!("first action must be RFPLL");
    };
    match action {
        RfpllFrequencyAction::WriteMasked { .. } => {}
        _ => panic!("RFPLL begins with a masked write"),
    }
}

#[test]
fn frequency_table_is_the_exact_four_byte_rom_object() {
    assert_eq!(PHY_DCODE_FREQUENCY_CODES, [0x73, 0x74, 0x75, 0x76]);
}

#[test]
fn foreign_completion_is_rejected_without_advancing() {
    let mut transition = PhyDcodeTransition::new(PhyDcodeParameters {
        crystal_selector: 0x31,
    });
    assert_eq!(
        transition.advance(PhyDcodeCompletion::NrxConfigured {
            frequency_code: PHY_DCODE_FREQUENCY_CODES[0],
        }),
        Err(PhyDcodeTransitionError::WrongCompletion)
    );
    assert!(matches!(transition.action(), PhyDcodeAction::Rfpll(_)));
}

#[test]
fn ckgen_and_two_reads_commit_the_final_owned_pair() {
    let mut transition = PhyDcodeTransition {
        parameters: PhyDcodeParameters {
            crystal_selector: 0x31,
        },
        codes: [1, 2, 3, 4, 5, 6, 0, 0],
        step: PhyDcodeStep::ConfigureNrx {
            calibration_index: 3,
        },
    };

    let PhyDcodeAction::ConfigureNrx { frequency_code } = transition.action() else {
        panic!("expected NRX action");
    };
    transition
        .advance(PhyDcodeCompletion::NrxConfigured { frequency_code })
        .unwrap();

    for _ in 0..4 {
        let PhyDcodeAction::WriteMasked { field, value } = transition.action() else {
            panic!("expected CKGEN write");
        };
        transition
            .advance(PhyDcodeCompletion::MaskedWrite { field, value })
            .unwrap();
    }

    for value in [7, 8] {
        let PhyDcodeAction::ReadMasked { field } = transition.action() else {
            panic!("expected D-code read");
        };
        transition
            .advance(PhyDcodeCompletion::MaskedRead { field, value })
            .unwrap();
    }

    assert_eq!(
        transition.action(),
        PhyDcodeAction::Complete(PhyDcodeOutcome {
            codes: [1, 2, 3, 4, 5, 6, 7, 8]
        })
    );
}

#[test]
fn six_bit_read_validation_fails_closed() {
    let mut transition = PhyDcodeTransition {
        parameters: PhyDcodeParameters {
            crystal_selector: 0x31,
        },
        codes: [0; 8],
        step: PhyDcodeStep::ReadLow {
            calibration_index: 0,
        },
    };
    let PhyDcodeAction::ReadMasked { field } = transition.action() else {
        panic!("expected D-code read");
    };
    assert_eq!(
        transition.advance(PhyDcodeCompletion::MaskedRead { field, value: 0x40 }),
        Err(PhyDcodeTransitionError::WrongCompletion)
    );
}

#[test]
fn external_lowering_covers_every_dcode_operation_class() {
    assert!(matches!(
        PhyDcodeExternalBinding::lower(PhyDcodeAction::Rfpll(RfpllFrequencyAction::DelayMicros(5))),
        Ok(PhyDcodeExternalBinding::Rfpll(_))
    ));
    assert!(matches!(
        PhyDcodeExternalBinding::lower(PhyDcodeAction::ConfigureNrx {
            frequency_code: PHY_DCODE_FREQUENCY_CODES[0],
        }),
        Ok(PhyDcodeExternalBinding::Mmio(_))
    ));
    assert!(matches!(
        PhyDcodeExternalBinding::lower(PhyDcodeAction::WriteMasked {
            field: analog_registers::RFPLL_DCODE_0_SOURCE_SELECT,
            value: 0,
        }),
        Ok(PhyDcodeExternalBinding::I2c(_))
    ));
    assert!(matches!(
        PhyDcodeExternalBinding::lower(PhyDcodeAction::ReadMasked {
            field: analog_registers::RFPLL_INTERNAL_DCODE_0,
        }),
        Ok(PhyDcodeExternalBinding::I2c(_))
    ));
    assert!(matches!(
        PhyDcodeExternalBinding::lower(PhyDcodeAction::Complete(PhyDcodeOutcome { codes: [0; 8] })),
        Err(PhyDcodeBindingError::UnsupportedAction)
    ));
}
