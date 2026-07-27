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
    SampleCode,
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
        value: u8,
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
            PhyTemperatureStep::SampleCode { .. } => PhyTemperatureAction::SampleCode,
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
                PhyTemperatureCompletion::CodeSampled { value },
            ) => {
                let current = attribute(sensor_index);
                let temperature = temperature_from_code(value, current.calibration);
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTemperatureBindingError {
    UnsupportedAction,
    IncompleteTransaction,
    UnexpectedOutcome,
}

/// Non-cloneable owner of one temperature-sensor PHY-I2C transaction.
///
/// The original high-level action remains attached to the low-level
/// transaction until completion, so an observation for another field at the
/// same I2C address cannot be accepted accidentally.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyTemperatureI2cBinding {
    outer_action: PhyTemperatureAction,
    transaction: crate::phy_cold::PhyColdI2cTransaction,
}

impl PhyTemperatureI2cBinding {
    pub fn new(action: PhyTemperatureAction) -> Result<Self, PhyTemperatureBindingError> {
        let request = match action {
            PhyTemperatureAction::ReadMasked {
                address,
                high_bit,
                low_bit,
            } => crate::phy_cold::PhyColdI2cRequest::read_masked(address, high_bit, low_bit),
            PhyTemperatureAction::WriteMasked {
                address,
                high_bit,
                low_bit,
                value,
            } => {
                crate::phy_cold::PhyColdI2cRequest::write_masked(address, high_bit, low_bit, value)
            }
            _ => None,
        }
        .ok_or(PhyTemperatureBindingError::UnsupportedAction)?;
        Ok(Self {
            outer_action: action,
            transaction: crate::phy_cold::PhyColdI2cTransaction::new(request),
        })
    }

    pub const fn outer_action(&self) -> PhyTemperatureAction {
        self.outer_action
    }

    pub const fn action(&self) -> crate::phy_cold::PhyColdI2cAction {
        self.transaction.action()
    }

    pub fn read_started(&mut self) -> Result<(), crate::phy_cold::PhyColdI2cError> {
        self.transaction.read_started()
    }

    pub fn write_started(&mut self) -> Result<(), crate::phy_cold::PhyColdI2cError> {
        self.transaction.write_started()
    }

    pub fn observe_read_result(
        &mut self,
        result: Result<u8, crate::phy_i2c::PhyI2cError>,
    ) -> Result<crate::phy_cold::PhyColdI2cObservation, crate::phy_cold::PhyColdI2cError> {
        self.transaction.observe_read_result(result)
    }

    pub fn observe_write_result(
        &mut self,
        result: Result<(), crate::phy_i2c::PhyI2cError>,
    ) -> Result<crate::phy_cold::PhyColdI2cObservation, crate::phy_cold::PhyColdI2cError> {
        self.transaction.observe_write_result(result)
    }

    #[cfg(target_arch = "riscv32")]
    pub fn start_target(
        &mut self,
        registers: &mut open_esp_radio_hal_esp32s31::RadioRegisters,
    ) -> Result<(), crate::phy_cold::PhyColdI2cError> {
        self.transaction.start_target(registers)
    }

    #[cfg(target_arch = "riscv32")]
    pub fn observe_target_edge(
        &mut self,
        registers: &mut open_esp_radio_hal_esp32s31::RadioRegisters,
    ) -> Result<crate::phy_cold::PhyColdI2cObservation, crate::phy_cold::PhyColdI2cError> {
        self.transaction.observe_target_edge(registers)
    }

    pub fn into_completion(self) -> Result<PhyTemperatureCompletion, PhyTemperatureBindingError> {
        let crate::phy_cold::PhyColdI2cAction::Complete(outcome) = self.transaction.action() else {
            return Err(PhyTemperatureBindingError::IncompleteTransaction);
        };
        match (self.outer_action, outcome) {
            (
                PhyTemperatureAction::ReadMasked {
                    address,
                    high_bit,
                    low_bit,
                },
                crate::phy_cold::PhyColdI2cOutcome::Read {
                    address: completed_address,
                    value,
                },
            ) if completed_address == address => Ok(PhyTemperatureCompletion::MaskedRead {
                address,
                high_bit,
                low_bit,
                value,
            }),
            (
                PhyTemperatureAction::WriteMasked {
                    address,
                    high_bit,
                    low_bit,
                    value,
                },
                crate::phy_cold::PhyColdI2cOutcome::Written {
                    address: completed_address,
                },
            ) if completed_address == address => Ok(PhyTemperatureCompletion::MaskedWrite {
                address,
                high_bit,
                low_bit,
                value,
            }),
            _ => Err(PhyTemperatureBindingError::UnexpectedOutcome),
        }
    }
}

/// Non-cloneable token for the single MMIO temperature-code sample.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyTemperatureSampleBinding {
    _private: (),
}

impl PhyTemperatureSampleBinding {
    pub fn new(action: PhyTemperatureAction) -> Result<Self, PhyTemperatureBindingError> {
        match action {
            PhyTemperatureAction::SampleCode => Ok(Self { _private: () }),
            _ => Err(PhyTemperatureBindingError::UnsupportedAction),
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut open_esp_radio_hal_esp32s31::RadioRegisters,
    ) -> PhyTemperatureCompletion {
        PhyTemperatureCompletion::CodeSampled {
            value: open_esp_radio_hal_esp32s31::phy_temperature::read_code(registers),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum PhyTemperatureExternalBinding {
    I2c(PhyTemperatureI2cBinding),
    Sample(PhyTemperatureSampleBinding),
}

impl PhyTemperatureExternalBinding {
    pub fn lower(action: PhyTemperatureAction) -> Result<Self, PhyTemperatureBindingError> {
        if let Ok(binding) = PhyTemperatureI2cBinding::new(action) {
            return Ok(Self::I2c(binding));
        }
        if let Ok(binding) = PhyTemperatureSampleBinding::new(action) {
            return Ok(Self::Sample(binding));
        }
        Err(PhyTemperatureBindingError::UnsupportedAction)
    }
}

#[cfg(test)]
mod binding_tests {
    use super::*;
    use crate::phy_cold::{PhyColdI2cAction, PhyColdI2cObservation, PhyColdI2cOutcome};

    #[test]
    fn i2c_binding_preserves_read_identity_and_extracts_the_field() {
        let action = PhyTemperatureTransition::new().action();
        let mut binding = PhyTemperatureI2cBinding::new(action).unwrap();
        assert_eq!(
            binding.action(),
            PhyColdI2cAction::StartRead {
                address: SENSOR_ADDRESS,
            }
        );
        binding.read_started().unwrap();
        assert_eq!(
            binding.observe_read_result(Ok(0b1100_1111)).unwrap(),
            PhyColdI2cObservation::EdgeConsumed
        );
        assert_eq!(
            binding.action(),
            PhyColdI2cAction::Complete(PhyColdI2cOutcome::Read {
                address: SENSOR_ADDRESS,
                value: 0x4f,
            })
        );
        assert_eq!(
            binding.into_completion().unwrap(),
            PhyTemperatureCompletion::MaskedRead {
                address: SENSOR_ADDRESS,
                high_bit: 6,
                low_bit: 0,
                value: 0x4f,
            }
        );
    }

    #[test]
    fn code_sample_binding_rejects_terminal_and_i2c_actions() {
        assert!(PhyTemperatureSampleBinding::new(PhyTemperatureAction::SampleCode).is_ok());
        assert_eq!(
            PhyTemperatureSampleBinding::new(PhyTemperatureTransition::new().action()),
            Err(PhyTemperatureBindingError::UnsupportedAction)
        );
    }

    #[test]
    fn external_lowering_covers_temperature_i2c_and_sample_actions() {
        assert!(matches!(
            PhyTemperatureExternalBinding::lower(PhyTemperatureTransition::new().action()),
            Ok(PhyTemperatureExternalBinding::I2c(_))
        ));
        assert!(matches!(
            PhyTemperatureExternalBinding::lower(PhyTemperatureAction::SampleCode),
            Ok(PhyTemperatureExternalBinding::Sample(_))
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::{
        temperature_from_code, PhyTemperatureAction, PhyTemperatureCompletion,
        PhyTemperatureFailure, PhyTemperatureOutcome, PhyTemperatureTransition,
        PhyTemperatureTransitionError, SENSOR_ADDRESS,
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
            assert_eq!(transition.action(), PhyTemperatureAction::SampleCode);
        }
    }

    #[test]
    fn conversion_and_in_range_path_match_rom_integer_arithmetic() {
        assert_eq!(temperature_from_code(128, -2), 91);
        let mut transition = PhyTemperatureTransition::new();
        complete_dac_read(&mut transition, 5);
        transition
            .advance(PhyTemperatureCompletion::CodeSampled { value: 128 })
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
            .advance(PhyTemperatureCompletion::CodeSampled { value: 255 })
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
    fn sample_step_rejects_a_non_sample_completion() {
        let mut transition = PhyTemperatureTransition::new();
        complete_dac_read(&mut transition, 5);
        assert_eq!(
            transition.advance(PhyTemperatureCompletion::MaskedRead {
                address: SENSOR_ADDRESS,
                high_bit: 6,
                low_bit: 0,
                value: 128,
            }),
            Err(PhyTemperatureTransitionError::WrongCompletion)
        );
    }
}
