//! Explicit ESP32-S31 PHY temperature-sensor transition.
//!
//! The required reference graph is ROM `phy_tsens_temp_read` at
//! `0x2f82_5eec`, its callback target `phy_tsens_temp_read_local` at
//! `0x2f82_5f1e`, and the finite conversion helpers at
//! `0x2f82_5de4..0x2f82_5ecc`. The outer function's indirect call and stores
//! through `g_phyFuns` and `phy_param` are ABI plumbing, not part of the radio
//! algorithm.
//!
//! Rust exposes the PHY-I2C read, one temperature-code sample, and the
//! conditional PHY-I2C range write as identity-bound actions. Invalid DAC
//! codes fail closed instead of reproducing the ROM's out-of-bounds default
//! table index.

use crate::phy_i2c::PhyI2cAddress;

pub const PHY_TEMPERATURE_CODE_ADDRESS: usize = 0x2081_8000;
pub const PHY_TEMPERATURE_CODE_MASK: u32 = 0xff;

const SENSOR_ADDRESS: PhyI2cAddress = PhyI2cAddress::new_internal(0x69, 0);

#[derive(Clone, Copy)]
struct TemperatureAttribute {
    calibration: i8,
    dac: u8,
    low: i16,
    high: i16,
}

// Exact 30-byte `phy_tsens_attribute` object at ROM address 0x2f84_d9ec.
const ATTRIBUTES: [TemperatureAttribute; 5] = [
    TemperatureAttribute {
        calibration: -2,
        dac: 5,
        low: 50,
        high: 125,
    },
    TemperatureAttribute {
        calibration: -1,
        dac: 7,
        low: 20,
        high: 100,
    },
    TemperatureAttribute {
        calibration: 0,
        dac: 15,
        low: -10,
        high: 80,
    },
    TemperatureAttribute {
        calibration: 1,
        dac: 11,
        low: -30,
        high: 50,
    },
    TemperatureAttribute {
        calibration: 2,
        dac: 10,
        low: -40,
        high: 20,
    },
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTemperatureOutcome {
    pub temperature: i16,
    pub sensor_index: u8,
    pub next_dac: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTemperatureFailure {
    InvalidDac(u8),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTemperatureAction {
    ReadMasked {
        address: PhyI2cAddress,
        high_bit: u8,
        low_bit: u8,
    },
    SampleCode {
        address: usize,
        mask: u32,
    },
    WriteMasked {
        address: PhyI2cAddress,
        high_bit: u8,
        low_bit: u8,
        value: u8,
    },
    Complete(PhyTemperatureOutcome),
    Failed(PhyTemperatureFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTemperatureCompletion {
    MaskedRead {
        address: PhyI2cAddress,
        high_bit: u8,
        low_bit: u8,
        value: u8,
    },
    CodeSampled {
        address: usize,
        value: u32,
    },
    MaskedWrite {
        address: PhyI2cAddress,
        high_bit: u8,
        low_bit: u8,
        value: u8,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTemperatureTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyTemperatureStep {
    ReadDac,
    SampleCode { sensor_index: u8 },
    WriteDac { outcome: PhyTemperatureOutcome },
    Complete(PhyTemperatureOutcome),
    Failed(PhyTemperatureFailure),
}

const fn sensor_index(dac: u8) -> Option<u8> {
    match dac {
        5 => Some(0),
        7 => Some(1),
        15 => Some(2),
        11 => Some(3),
        10 => Some(4),
        _ => None,
    }
}

const fn attribute(index: u8) -> TemperatureAttribute {
    match index {
        0 => ATTRIBUTES[0],
        1 => ATTRIBUTES[1],
        2 => ATTRIBUTES[2],
        3 => ATTRIBUTES[3],
        _ => ATTRIBUTES[4],
    }
}

/// Exact arithmetic of ROM `phy_code_to_temp`.
pub const fn temperature_from_code(code: u8, calibration: i8) -> i16 {
    let scaled = (code as i32) * 44 + (calibration as i32) * -2_788;
    if scaled > 27_151 {
        250
    } else {
        let temperature = (scaled - 2_052) / 100;
        if temperature < -200 {
            -200
        } else {
            temperature as i16
        }
    }
}

const fn selected_dac(temperature: i16, current: TemperatureAttribute) -> u8 {
    if temperature >= current.low && temperature <= current.high {
        current.dac
    } else if temperature > 99 {
        5
    } else if temperature > 79 {
        7
    } else if temperature >= -9 {
        15
    } else if temperature < -29 {
        10
    } else {
        11
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTemperatureTransition {
    step: PhyTemperatureStep,
}

impl PhyTemperatureTransition {
    pub const fn new() -> Self {
        Self {
            step: PhyTemperatureStep::ReadDac,
        }
    }

    pub const fn action(self) -> PhyTemperatureAction {
        match self.step {
            PhyTemperatureStep::ReadDac => PhyTemperatureAction::ReadMasked {
                address: SENSOR_ADDRESS,
                high_bit: 6,
                low_bit: 0,
            },
            PhyTemperatureStep::SampleCode { .. } => PhyTemperatureAction::SampleCode {
                address: PHY_TEMPERATURE_CODE_ADDRESS,
                mask: PHY_TEMPERATURE_CODE_MASK,
            },
            PhyTemperatureStep::WriteDac { outcome } => PhyTemperatureAction::WriteMasked {
                address: SENSOR_ADDRESS,
                high_bit: 3,
                low_bit: 0,
                value: outcome.next_dac,
            },
            PhyTemperatureStep::Complete(outcome) => PhyTemperatureAction::Complete(outcome),
            PhyTemperatureStep::Failed(failure) => PhyTemperatureAction::Failed(failure),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyTemperatureCompletion,
    ) -> Result<(), PhyTemperatureTransitionError> {
        self.step = match (self.step, completion) {
            (
                PhyTemperatureStep::ReadDac,
                PhyTemperatureCompletion::MaskedRead {
                    address: SENSOR_ADDRESS,
                    high_bit: 6,
                    low_bit: 0,
                    value,
                },
            ) => match sensor_index(value & 0x0f) {
                Some(sensor_index) => PhyTemperatureStep::SampleCode { sensor_index },
                None => PhyTemperatureStep::Failed(PhyTemperatureFailure::InvalidDac(value & 0x0f)),
            },
            (
                PhyTemperatureStep::SampleCode { sensor_index },
                PhyTemperatureCompletion::CodeSampled {
                    address: PHY_TEMPERATURE_CODE_ADDRESS,
                    value,
                },
            ) => {
                let current = attribute(sensor_index);
                let temperature = temperature_from_code(
                    (value & PHY_TEMPERATURE_CODE_MASK) as u8,
                    current.calibration,
                );
                let next_dac = selected_dac(temperature, current);
                let outcome = PhyTemperatureOutcome {
                    temperature,
                    sensor_index,
                    next_dac,
                };
                if next_dac == current.dac {
                    PhyTemperatureStep::Complete(outcome)
                } else {
                    PhyTemperatureStep::WriteDac { outcome }
                }
            }
            (
                PhyTemperatureStep::WriteDac { outcome },
                PhyTemperatureCompletion::MaskedWrite {
                    address: SENSOR_ADDRESS,
                    high_bit: 3,
                    low_bit: 0,
                    value,
                },
            ) if value == outcome.next_dac => PhyTemperatureStep::Complete(outcome),
            (PhyTemperatureStep::Complete(_), _) | (PhyTemperatureStep::Failed(_), _) => {
                return Err(PhyTemperatureTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyTemperatureTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

impl Default for PhyTemperatureTransition {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        temperature_from_code, PhyTemperatureAction, PhyTemperatureCompletion,
        PhyTemperatureFailure, PhyTemperatureOutcome, PhyTemperatureTransition,
        PhyTemperatureTransitionError, PHY_TEMPERATURE_CODE_ADDRESS, PHY_TEMPERATURE_CODE_MASK,
        SENSOR_ADDRESS,
    };

    fn complete_dac_read(transition: &mut PhyTemperatureTransition, dac: u8) {
        transition
            .advance(PhyTemperatureCompletion::MaskedRead {
                address: SENSOR_ADDRESS,
                high_bit: 6,
                low_bit: 0,
                value: dac,
            })
            .unwrap();
    }

    #[test]
    fn all_five_recovered_dac_codes_are_accepted() {
        for dac in [5, 7, 15, 11, 10] {
            let mut transition = PhyTemperatureTransition::new();
            complete_dac_read(&mut transition, dac);
            assert_eq!(
                transition.action(),
                PhyTemperatureAction::SampleCode {
                    address: PHY_TEMPERATURE_CODE_ADDRESS,
                    mask: PHY_TEMPERATURE_CODE_MASK,
                }
            );
        }
    }

    #[test]
    fn conversion_and_in_range_path_match_rom_integer_arithmetic() {
        assert_eq!(temperature_from_code(128, -2), 91);
        let mut transition = PhyTemperatureTransition::new();
        complete_dac_read(&mut transition, 5);
        transition
            .advance(PhyTemperatureCompletion::CodeSampled {
                address: PHY_TEMPERATURE_CODE_ADDRESS,
                value: 128,
            })
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyTemperatureAction::Complete(PhyTemperatureOutcome {
                temperature: 91,
                sensor_index: 0,
                next_dac: 5,
            })
        );
    }

    #[test]
    fn range_change_requires_an_exact_i2c_write_completion() {
        let mut transition = PhyTemperatureTransition::new();
        complete_dac_read(&mut transition, 15);
        transition
            .advance(PhyTemperatureCompletion::CodeSampled {
                address: PHY_TEMPERATURE_CODE_ADDRESS,
                value: 255,
            })
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyTemperatureAction::WriteMasked {
                address: SENSOR_ADDRESS,
                high_bit: 3,
                low_bit: 0,
                value: 7,
            }
        );
        assert_eq!(
            transition.advance(PhyTemperatureCompletion::MaskedWrite {
                address: SENSOR_ADDRESS,
                high_bit: 3,
                low_bit: 0,
                value: 5,
            }),
            Err(PhyTemperatureTransitionError::WrongCompletion)
        );
        transition
            .advance(PhyTemperatureCompletion::MaskedWrite {
                address: SENSOR_ADDRESS,
                high_bit: 3,
                low_bit: 0,
                value: 7,
            })
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyTemperatureAction::Complete(PhyTemperatureOutcome {
                temperature: 91,
                sensor_index: 2,
                next_dac: 7,
            })
        );
    }

    #[test]
    fn invalid_rom_default_index_is_a_typed_failure() {
        let mut transition = PhyTemperatureTransition::new();
        complete_dac_read(&mut transition, 0);
        assert_eq!(
            transition.action(),
            PhyTemperatureAction::Failed(PhyTemperatureFailure::InvalidDac(0))
        );
    }

    #[test]
    fn sample_completion_is_bound_to_the_exact_mmio_address() {
        let mut transition = PhyTemperatureTransition::new();
        complete_dac_read(&mut transition, 5);
        assert_eq!(
            transition.advance(PhyTemperatureCompletion::CodeSampled {
                address: PHY_TEMPERATURE_CODE_ADDRESS + 4,
                value: 128,
            }),
            Err(PhyTemperatureTransitionError::WrongCompletion)
        );
    }
}
