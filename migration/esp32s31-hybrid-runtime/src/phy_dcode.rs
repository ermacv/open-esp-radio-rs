//! Rust-owned ESP32-S31 PHY D-code calibration.
//!
//! The required root is ROM `phy_dcode_cal_init` at `0x2f82_b8da`, size
//! 128 bytes. It visits four fixed RF frequency codes, resets four CKGEN
//! fields through PHY-I2C, reads two six-bit D-code values, and stores the
//! resulting eight bytes through the global `phy_param` pointer.
//!
//! The nested RFPLL work reuses [`crate::phy_rfpll`]. Every remaining I2C and
//! MMIO operation is an identity-bound action. The four-byte ROM table and
//! eight output bytes are Rust-owned values; no ROM RAM or callback table is
//! required.

use crate::phy_i2c::PhyI2cAddress;
use crate::phy_rfpll::{
    RfpllFrequencyAction, RfpllFrequencyCompletion, RfpllFrequencyFailure, RfpllFrequencyRequest,
    RfpllFrequencyTransition,
};

pub const PHY_DCODE_FREQUENCY_CODES: [u8; 4] = [115, 116, 117, 118];
pub const PHY_NRX_FREQUENCY_CONTROL_ADDRESS: usize = 0x2010_7848;

const RFPLL_BLOCK: u8 = 0x62;
const CKGEN_WRITES: [(u8, u8, u8, u8); 4] = [
    (0x13, 6, 6, 0),
    (0x14, 6, 6, 0),
    (0x04, 7, 7, 0),
    (0x04, 7, 7, 1),
];

const fn address(register: u8) -> PhyI2cAddress {
    PhyI2cAddress::new_internal(RFPLL_BLOCK, register)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyDcodeParameters {
    pub crystal_selector: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyDcodeOutcome {
    pub codes: [u8; 8],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyDcodeFailure {
    Rfpll {
        calibration_index: u8,
        failure: RfpllFrequencyFailure,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyDcodeAction {
    Rfpll(RfpllFrequencyAction),
    ConfigureNrx {
        frequency_code: u8,
        address: usize,
    },
    WriteMasked {
        address: PhyI2cAddress,
        high_bit: u8,
        low_bit: u8,
        value: u8,
    },
    ReadMasked {
        address: PhyI2cAddress,
        high_bit: u8,
        low_bit: u8,
    },
    Complete(PhyDcodeOutcome),
    Failed(PhyDcodeFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyDcodeCompletion {
    Rfpll(RfpllFrequencyCompletion),
    NrxConfigured {
        frequency_code: u8,
        address: usize,
    },
    MaskedWrite {
        address: PhyI2cAddress,
        high_bit: u8,
        low_bit: u8,
        value: u8,
    },
    MaskedRead {
        address: PhyI2cAddress,
        high_bit: u8,
        low_bit: u8,
        value: u8,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyDcodeTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyDcodeStep {
    Rfpll {
        calibration_index: u8,
        transition: RfpllFrequencyTransition,
    },
    ConfigureNrx {
        calibration_index: u8,
    },
    ResetCkgen {
        calibration_index: u8,
        write_index: u8,
    },
    ReadLow {
        calibration_index: u8,
    },
    ReadHigh {
        calibration_index: u8,
    },
    Complete(PhyDcodeOutcome),
    Failed(PhyDcodeFailure),
}

const fn rfpll_transition(calibration_index: u8, crystal_selector: u8) -> RfpllFrequencyTransition {
    RfpllFrequencyTransition::new(RfpllFrequencyRequest {
        crystal_selector,
        frequency_code: PHY_DCODE_FREQUENCY_CODES[calibration_index as usize] as u16,
        offset: 0,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyDcodeTransition {
    parameters: PhyDcodeParameters,
    codes: [u8; 8],
    step: PhyDcodeStep,
}

impl PhyDcodeTransition {
    pub const fn new(parameters: PhyDcodeParameters) -> Self {
        Self {
            parameters,
            codes: [0; 8],
            step: PhyDcodeStep::Rfpll {
                calibration_index: 0,
                transition: rfpll_transition(0, parameters.crystal_selector),
            },
        }
    }

    pub const fn action(self) -> PhyDcodeAction {
        match self.step {
            PhyDcodeStep::Rfpll { transition, .. } => PhyDcodeAction::Rfpll(transition.action()),
            PhyDcodeStep::ConfigureNrx { calibration_index } => PhyDcodeAction::ConfigureNrx {
                frequency_code: PHY_DCODE_FREQUENCY_CODES[calibration_index as usize],
                address: PHY_NRX_FREQUENCY_CONTROL_ADDRESS,
            },
            PhyDcodeStep::ResetCkgen { write_index, .. } => {
                let (register, high_bit, low_bit, value) = CKGEN_WRITES[write_index as usize];
                PhyDcodeAction::WriteMasked {
                    address: address(register),
                    high_bit,
                    low_bit,
                    value,
                }
            }
            PhyDcodeStep::ReadLow { .. } => PhyDcodeAction::ReadMasked {
                address: address(0x11),
                high_bit: 5,
                low_bit: 0,
            },
            PhyDcodeStep::ReadHigh { .. } => PhyDcodeAction::ReadMasked {
                address: address(0x12),
                high_bit: 5,
                low_bit: 0,
            },
            PhyDcodeStep::Complete(outcome) => PhyDcodeAction::Complete(outcome),
            PhyDcodeStep::Failed(failure) => PhyDcodeAction::Failed(failure),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyDcodeCompletion,
    ) -> Result<(), PhyDcodeTransitionError> {
        self.step = match (self.step, completion) {
            (
                PhyDcodeStep::Rfpll {
                    calibration_index,
                    mut transition,
                },
                PhyDcodeCompletion::Rfpll(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyDcodeTransitionError::WrongCompletion)?;
                match transition.action() {
                    RfpllFrequencyAction::Complete(_) => {
                        PhyDcodeStep::ConfigureNrx { calibration_index }
                    }
                    RfpllFrequencyAction::Failed(failure) => {
                        PhyDcodeStep::Failed(PhyDcodeFailure::Rfpll {
                            calibration_index,
                            failure,
                        })
                    }
                    _ => PhyDcodeStep::Rfpll {
                        calibration_index,
                        transition,
                    },
                }
            }
            (
                PhyDcodeStep::ConfigureNrx { calibration_index },
                PhyDcodeCompletion::NrxConfigured {
                    frequency_code,
                    address: PHY_NRX_FREQUENCY_CONTROL_ADDRESS,
                },
            ) if frequency_code == PHY_DCODE_FREQUENCY_CODES[calibration_index as usize] => {
                PhyDcodeStep::ResetCkgen {
                    calibration_index,
                    write_index: 0,
                }
            }
            (
                PhyDcodeStep::ResetCkgen {
                    calibration_index,
                    write_index,
                },
                PhyDcodeCompletion::MaskedWrite {
                    address: completed_address,
                    high_bit,
                    low_bit,
                    value,
                },
            ) => {
                let (register, expected_high, expected_low, expected_value) =
                    CKGEN_WRITES[write_index as usize];
                if completed_address != address(register)
                    || high_bit != expected_high
                    || low_bit != expected_low
                    || value != expected_value
                {
                    return Err(PhyDcodeTransitionError::WrongCompletion);
                }
                if write_index + 1 == CKGEN_WRITES.len() as u8 {
                    PhyDcodeStep::ReadLow { calibration_index }
                } else {
                    PhyDcodeStep::ResetCkgen {
                        calibration_index,
                        write_index: write_index + 1,
                    }
                }
            }
            (
                PhyDcodeStep::ReadLow { calibration_index },
                PhyDcodeCompletion::MaskedRead {
                    address: completed_address,
                    high_bit: 5,
                    low_bit: 0,
                    value,
                },
            ) if completed_address == address(0x11) && value <= 0x3f => {
                self.codes[calibration_index as usize * 2] = value;
                PhyDcodeStep::ReadHigh { calibration_index }
            }
            (
                PhyDcodeStep::ReadHigh { calibration_index },
                PhyDcodeCompletion::MaskedRead {
                    address: completed_address,
                    high_bit: 5,
                    low_bit: 0,
                    value,
                },
            ) if completed_address == address(0x12) && value <= 0x3f => {
                self.codes[calibration_index as usize * 2 + 1] = value;
                if calibration_index + 1 == PHY_DCODE_FREQUENCY_CODES.len() as u8 {
                    PhyDcodeStep::Complete(PhyDcodeOutcome { codes: self.codes })
                } else {
                    let next = calibration_index + 1;
                    PhyDcodeStep::Rfpll {
                        calibration_index: next,
                        transition: rfpll_transition(next, self.parameters.crystal_selector),
                    }
                }
            }
            (PhyDcodeStep::Complete(_), _) | (PhyDcodeStep::Failed(_), _) => {
                return Err(PhyDcodeTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyDcodeTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PhyDcodeAction, PhyDcodeCompletion, PhyDcodeOutcome, PhyDcodeParameters, PhyDcodeStep,
        PhyDcodeTransition, PhyDcodeTransitionError, PHY_DCODE_FREQUENCY_CODES,
        PHY_NRX_FREQUENCY_CONTROL_ADDRESS,
    };
    use crate::phy_rfpll::RfpllFrequencyAction;

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
                address: PHY_NRX_FREQUENCY_CONTROL_ADDRESS,
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

        let PhyDcodeAction::ConfigureNrx {
            frequency_code,
            address,
        } = transition.action()
        else {
            panic!("expected NRX action");
        };
        transition
            .advance(PhyDcodeCompletion::NrxConfigured {
                frequency_code,
                address,
            })
            .unwrap();

        for _ in 0..4 {
            let PhyDcodeAction::WriteMasked {
                address,
                high_bit,
                low_bit,
                value,
            } = transition.action()
            else {
                panic!("expected CKGEN write");
            };
            transition
                .advance(PhyDcodeCompletion::MaskedWrite {
                    address,
                    high_bit,
                    low_bit,
                    value,
                })
                .unwrap();
        }

        for value in [7, 8] {
            let PhyDcodeAction::ReadMasked {
                address,
                high_bit,
                low_bit,
            } = transition.action()
            else {
                panic!("expected D-code read");
            };
            transition
                .advance(PhyDcodeCompletion::MaskedRead {
                    address,
                    high_bit,
                    low_bit,
                    value,
                })
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
        let PhyDcodeAction::ReadMasked {
            address,
            high_bit,
            low_bit,
        } = transition.action()
        else {
            panic!("expected D-code read");
        };
        assert_eq!(
            transition.advance(PhyDcodeCompletion::MaskedRead {
                address,
                high_bit,
                low_bit,
                value: 0x40,
            }),
            Err(PhyDcodeTransitionError::WrongCompletion)
        );
    }
}
