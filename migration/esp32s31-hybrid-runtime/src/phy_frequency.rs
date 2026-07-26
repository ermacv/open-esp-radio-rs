//! Rust-owned ESP32-S31 cold frequency-table generation.
//!
//! The pinned reference is `libphy.a[phy_hw_freq.o]::phy_get_rf_freq_init`,
//! size `0x1d8`, and the rev0 ROM leaves `phy_get_data_sat`,
//! `phy_get_freq_mem_param`, `phy_freq_i2c_mem_write`, and
//! `phy_wr_rf_freq_mem`. The vendor parent materializes one three-word record
//! at a time for 85 frequencies beginning at `0x960`.
//!
//! Rust retains only the interpolation scalars and the current entry/word
//! indices. It never allocates or stores the complete 1,020-byte table in
//! SRAM. Every hardware-memory publication is an identity-bound finite MMIO
//! action completed by the caller.

use crate::{
    phy_i2c::PhyI2cAddress,
    phy_rfpll::{
        calculate_rfpll_sdm, RfpllFrequencyAction, RfpllFrequencyCompletion, RfpllFrequencyFailure,
        RfpllFrequencyOutcome, RfpllFrequencyRequest, RfpllFrequencyTransition,
    },
};

pub const PHY_FREQUENCY_TABLE_ENTRY_COUNT: u8 = 85;
pub const PHY_FREQUENCY_TABLE_FIRST_CODE: u16 = 0x960;
const PHY_FREQUENCY_MEMORY_BASE: u16 = 0x12;
const PHY_FREQUENCY_MEMORY_ENTRY_STRIDE: u16 = 7;
const PHY_FREQUENCY_MEMORY_WORD_STRIDE: u16 = 3;
const PHY_FREQUENCY_MEMORY_MODE: u8 = 7;
const CAP_INTERPOLATION_DIVISOR: i32 = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyFrequencyTableParameters {
    /// Explicit replacement for `phy_param[0x4f]`.
    pub crystal_selector: u8,
    /// Explicit replacement for `phy_param[0x19f]`.
    pub middle_xtal_duty: u8,
    /// Explicit replacement for `phy_param[0x1a0]`.
    pub outer_xtal_duty: u8,
    /// Upper five bits read once from PHY-I2C block `0x63`, register `6`.
    pub sdm_register_six_upper: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyFrequencyMemoryRecord {
    words: [u32; 3],
}

impl PhyFrequencyMemoryRecord {
    pub const fn words(self) -> [u32; 3] {
        self.words
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyFrequencyTableRequest {
    pub parameters: PhyFrequencyTableParameters,
    pub low_frequency_cap: u16,
    pub high_frequency_cap: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyFrequencyTableOutcome {
    pub entries_written: u8,
    pub low_frequency_cap: u16,
    pub high_frequency_cap: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyFrequencyTableAction {
    WriteMemory {
        entry_index: u8,
        word_index: u8,
        address: u16,
        value: u32,
        mode: u8,
    },
    Complete(PhyFrequencyTableOutcome),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyFrequencyTableCompletion {
    pub entry_index: u8,
    pub word_index: u8,
    pub address: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyFrequencyTableTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

/// Exact stateless body of pinned `libphy.a[phy_rx_cal.o]::phy_get_xtal_duty`.
pub const fn phy_frequency_xtal_duty(
    frequency_code: u16,
    middle_xtal_duty: u8,
    outer_xtal_duty: u8,
) -> u8 {
    if frequency_code <= 0x967 {
        17
    } else if frequency_code.wrapping_sub(0x975) <= 38 {
        middle_xtal_duty
    } else {
        outer_xtal_duty
    }
}

const fn interpolated_cap(low: u16, high: u16, index: u8) -> u16 {
    let low = low as i16 as i32;
    let delta = (high as i16 as i32).wrapping_sub(low);
    let interpolated =
        low.wrapping_add(delta.wrapping_mul(index as i32) / CAP_INTERPOLATION_DIVISOR);
    if interpolated < 0 {
        0
    } else if interpolated > 0x1ff {
        0x1ff
    } else {
        interpolated as u16
    }
}

/// Reproduce one packed three-word `phy_wr_rf_freq_mem` input record.
pub const fn phy_frequency_memory_record(
    request: PhyFrequencyTableRequest,
    index: u8,
) -> PhyFrequencyMemoryRecord {
    let frequency_code = PHY_FREQUENCY_TABLE_FIRST_CODE.wrapping_add(index as u16);
    let sdm = calculate_rfpll_sdm(frequency_code, request.parameters.crystal_selector, 0).bytes();
    let cap = interpolated_cap(request.low_frequency_cap, request.high_frequency_cap, index);
    let cap_high = 0xbf | (((cap >> 8) as u8 & 1) << 6);
    let sdm_low = sdm[0] | (request.parameters.sdm_register_six_upper & 0xf8);
    let xtal_duty = phy_frequency_xtal_duty(
        frequency_code,
        request.parameters.middle_xtal_duty,
        request.parameters.outer_xtal_duty,
    );

    PhyFrequencyMemoryRecord {
        words: [
            cap as u8 as u32 | ((cap_high as u32) << 8) | ((sdm_low as u32) << 16),
            sdm[1] as u32 | ((sdm[2] as u32) << 8) | ((sdm[3] as u32) << 16),
            xtal_duty as u32,
        ],
    }
}

/// Caller-driven 85-entry frequency-memory publisher.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyFrequencyTableTransition {
    request: PhyFrequencyTableRequest,
    entry_index: u8,
    word_index: u8,
}

impl PhyFrequencyTableTransition {
    pub const fn new(request: PhyFrequencyTableRequest) -> Self {
        Self {
            request,
            entry_index: 0,
            word_index: 0,
        }
    }

    const fn address(self) -> u16 {
        PHY_FREQUENCY_MEMORY_BASE
            .wrapping_add((self.entry_index as u16).wrapping_mul(PHY_FREQUENCY_MEMORY_ENTRY_STRIDE))
            .wrapping_add((self.word_index as u16).wrapping_mul(PHY_FREQUENCY_MEMORY_WORD_STRIDE))
    }

    pub const fn action(self) -> PhyFrequencyTableAction {
        if self.entry_index == PHY_FREQUENCY_TABLE_ENTRY_COUNT {
            PhyFrequencyTableAction::Complete(PhyFrequencyTableOutcome {
                entries_written: self.entry_index,
                low_frequency_cap: self.request.low_frequency_cap,
                high_frequency_cap: self.request.high_frequency_cap,
            })
        } else {
            let record = phy_frequency_memory_record(self.request, self.entry_index);
            PhyFrequencyTableAction::WriteMemory {
                entry_index: self.entry_index,
                word_index: self.word_index,
                address: self.address(),
                value: record.words[self.word_index as usize],
                mode: PHY_FREQUENCY_MEMORY_MODE,
            }
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyFrequencyTableCompletion,
    ) -> Result<(), PhyFrequencyTableTransitionError> {
        let PhyFrequencyTableAction::WriteMemory {
            entry_index,
            word_index,
            address,
            ..
        } = self.action()
        else {
            return Err(PhyFrequencyTableTransitionError::AlreadyComplete);
        };
        if completion.entry_index != entry_index
            || completion.word_index != word_index
            || completion.address != address
        {
            return Err(PhyFrequencyTableTransitionError::WrongCompletion);
        }

        if self.word_index == 2 {
            self.word_index = 0;
            self.entry_index += 1;
        } else {
            self.word_index += 1;
        }
        Ok(())
    }
}

const fn i2c_address(block: u8, register: u8) -> PhyI2cAddress {
    PhyI2cAddress::new_internal(block, register)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyFrequencyI2cRequest {
    /// Explicit replacement for the single bit read from `phy_param[0x1af]`.
    pub front_end_parameter_bit: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyFrequencyI2cNumberAddressImage {
    pub control_field: u32,
    pub words: [u32; 3],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyFrequencyI2cOutcome {
    pub rfpll_register_0b: u8,
    pub sdm_register_0: u8,
    pub front_end_register_3: u8,
    pub number_addresses: PhyFrequencyI2cNumberAddressImage,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyFrequencyI2cAction {
    WriteMasked {
        address: PhyI2cAddress,
        high_bit: u8,
        low_bit: u8,
        value: u8,
    },
    ReadByte {
        address: PhyI2cAddress,
    },
    WriteMemory {
        descriptor_index: u8,
        copy_index: u8,
        address: u16,
        value: u32,
        mode: u8,
    },
    ConfigureNumberAddresses(PhyFrequencyI2cNumberAddressImage),
    Complete(PhyFrequencyI2cOutcome),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyFrequencyI2cCompletion {
    MaskedWrite {
        address: PhyI2cAddress,
        high_bit: u8,
        low_bit: u8,
    },
    ByteRead {
        address: PhyI2cAddress,
        value: u8,
    },
    MemoryWrite {
        descriptor_index: u8,
        copy_index: u8,
        address: u16,
    },
    NumberAddressesConfigured(PhyFrequencyI2cNumberAddressImage),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyFrequencyI2cTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PhyFrequencyI2cDescriptor {
    block: u8,
    register: u8,
    encoded_index: u8,
    data: u8,
}

impl PhyFrequencyI2cDescriptor {
    const fn kind(self) -> u8 {
        self.encoded_index >> 4
    }

    const fn index(self) -> u8 {
        self.encoded_index & 0x0f
    }

    const fn number_address(self) -> u8 {
        match self.kind() {
            0 => self.index(),
            1 => self.index().wrapping_add(1),
            _ => self.index().wrapping_add(4),
        }
    }

    const fn memory_write_count(self) -> u8 {
        if self.kind() == 0 {
            3
        } else {
            1
        }
    }

    const fn memory_write(self, copy_index: u8) -> (u16, u32, u8) {
        let block_register = ((self.block as u32) << 8) | self.register as u32;
        let data_word = ((self.data as u32) << 16) | block_register;
        match self.kind() {
            0 => match copy_index {
                0 => (
                    self.index() as u16 * 3,
                    data_word,
                    PHY_FREQUENCY_MEMORY_MODE,
                ),
                1 => (
                    (self.index() as u16 + 1) * 3,
                    block_register,
                    PHY_FREQUENCY_MEMORY_MODE,
                ),
                _ => (
                    self.index() as u16 * 3 + 6,
                    block_register,
                    PHY_FREQUENCY_MEMORY_MODE,
                ),
            },
            1 => (
                self.index() as u16 * 3 + 9,
                data_word,
                PHY_FREQUENCY_MEMORY_MODE,
            ),
            _ => (
                self.index() as u16 * 2 + PHY_FREQUENCY_MEMORY_BASE,
                block_register,
                3,
            ),
        }
    }
}

const fn frequency_i2c_descriptor(
    index: u8,
    rfpll_register_0b: u8,
    sdm_register_0: u8,
    front_end_register_3: u8,
    front_end_parameter_bit: bool,
) -> PhyFrequencyI2cDescriptor {
    match index {
        0 => PhyFrequencyI2cDescriptor {
            block: 0x62,
            register: 1,
            encoded_index: 0x20,
            data: 0,
        },
        1 => PhyFrequencyI2cDescriptor {
            block: 0x62,
            register: 2,
            encoded_index: 0x21,
            data: 0,
        },
        2 => PhyFrequencyI2cDescriptor {
            block: 0x63,
            register: 0,
            encoded_index: 0x10,
            data: sdm_register_0 & 0xf7,
        },
        3 => PhyFrequencyI2cDescriptor {
            block: 0x63,
            register: 6,
            encoded_index: 0x22,
            data: 0,
        },
        4 => PhyFrequencyI2cDescriptor {
            block: 0x63,
            register: 5,
            encoded_index: 0x23,
            data: 0,
        },
        5 => PhyFrequencyI2cDescriptor {
            block: 0x63,
            register: 4,
            encoded_index: 0x24,
            data: 0,
        },
        6 => PhyFrequencyI2cDescriptor {
            block: 0x63,
            register: 3,
            encoded_index: 0x25,
            data: 0,
        },
        7 => PhyFrequencyI2cDescriptor {
            block: 0x63,
            register: 0,
            encoded_index: 0x11,
            data: sdm_register_0 | 8,
        },
        8 => PhyFrequencyI2cDescriptor {
            block: 0x62,
            register: 0x0b,
            encoded_index: 0x12,
            data: rfpll_register_0b,
        },
        9 => PhyFrequencyI2cDescriptor {
            block: 0x61,
            register: 0x0a,
            encoded_index: 0x26,
            data: 0,
        },
        _ => PhyFrequencyI2cDescriptor {
            block: 0x67,
            register: 3,
            encoded_index: front_end_register_3 | if front_end_parameter_bit { 4 } else { 0 },
            data: 1,
        },
    }
}

const fn pack_six_number_addresses(
    start: u8,
    rfpll_register_0b: u8,
    sdm_register_0: u8,
    front_end_register_3: u8,
    front_end_parameter_bit: bool,
) -> u32 {
    let mut value = 0_u32;
    let mut slot = 0_u8;
    while slot != 6 {
        let index = start.wrapping_add(slot);
        if index < 11 {
            value |= (frequency_i2c_descriptor(
                index,
                rfpll_register_0b,
                sdm_register_0,
                front_end_register_3,
                front_end_parameter_bit,
            )
            .number_address() as u32)
                << (slot * 5);
        }
        slot += 1;
    }
    value
}

pub const fn phy_frequency_i2c_number_address_image(
    rfpll_register_0b: u8,
    sdm_register_0: u8,
    front_end_register_3: u8,
    front_end_parameter_bit: bool,
) -> PhyFrequencyI2cNumberAddressImage {
    let first = frequency_i2c_descriptor(
        0,
        rfpll_register_0b,
        sdm_register_0,
        front_end_register_3,
        front_end_parameter_bit,
    )
    .number_address();
    let second = frequency_i2c_descriptor(
        1,
        rfpll_register_0b,
        sdm_register_0,
        front_end_register_3,
        front_end_parameter_bit,
    )
    .number_address();
    PhyFrequencyI2cNumberAddressImage {
        control_field: (((second as u32) << 5) | first as u32) << 8,
        words: [
            pack_six_number_addresses(
                2,
                rfpll_register_0b,
                sdm_register_0,
                front_end_register_3,
                front_end_parameter_bit,
            ),
            pack_six_number_addresses(
                8,
                rfpll_register_0b,
                sdm_register_0,
                front_end_register_3,
                front_end_parameter_bit,
            ),
            0,
        ],
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyFrequencyI2cStep {
    EnableSnapshot,
    ReadRfpll,
    ReadSdm {
        rfpll_register_0b: u8,
    },
    ReadFrontEnd {
        rfpll_register_0b: u8,
        sdm_register_0: u8,
    },
    Memory {
        rfpll_register_0b: u8,
        sdm_register_0: u8,
        front_end_register_3: u8,
        descriptor_index: u8,
        copy_index: u8,
    },
    NumberAddresses {
        rfpll_register_0b: u8,
        sdm_register_0: u8,
        front_end_register_3: u8,
    },
    Complete(PhyFrequencyI2cOutcome),
}

/// Complete caller-driven replacement for pinned
/// `phy_freq_i2c_data_write(1)`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyFrequencyI2cTransition {
    request: PhyFrequencyI2cRequest,
    step: PhyFrequencyI2cStep,
}

impl PhyFrequencyI2cTransition {
    pub const fn new(request: PhyFrequencyI2cRequest) -> Self {
        Self {
            request,
            step: PhyFrequencyI2cStep::EnableSnapshot,
        }
    }

    pub const fn action(self) -> PhyFrequencyI2cAction {
        match self.step {
            PhyFrequencyI2cStep::EnableSnapshot => PhyFrequencyI2cAction::WriteMasked {
                address: i2c_address(0x62, 0x0b),
                high_bit: 6,
                low_bit: 6,
                value: 1,
            },
            PhyFrequencyI2cStep::ReadRfpll => PhyFrequencyI2cAction::ReadByte {
                address: i2c_address(0x62, 0x0b),
            },
            PhyFrequencyI2cStep::ReadSdm { .. } => PhyFrequencyI2cAction::ReadByte {
                address: i2c_address(0x63, 0),
            },
            PhyFrequencyI2cStep::ReadFrontEnd { .. } => PhyFrequencyI2cAction::ReadByte {
                address: i2c_address(0x67, 3),
            },
            PhyFrequencyI2cStep::Memory {
                rfpll_register_0b,
                sdm_register_0,
                front_end_register_3,
                descriptor_index,
                copy_index,
            } => {
                let descriptor = frequency_i2c_descriptor(
                    descriptor_index,
                    rfpll_register_0b,
                    sdm_register_0,
                    front_end_register_3,
                    self.request.front_end_parameter_bit,
                );
                let (address, value, mode) = descriptor.memory_write(copy_index);
                PhyFrequencyI2cAction::WriteMemory {
                    descriptor_index,
                    copy_index,
                    address,
                    value,
                    mode,
                }
            }
            PhyFrequencyI2cStep::NumberAddresses {
                rfpll_register_0b,
                sdm_register_0,
                front_end_register_3,
            } => PhyFrequencyI2cAction::ConfigureNumberAddresses(
                phy_frequency_i2c_number_address_image(
                    rfpll_register_0b,
                    sdm_register_0,
                    front_end_register_3,
                    self.request.front_end_parameter_bit,
                ),
            ),
            PhyFrequencyI2cStep::Complete(outcome) => PhyFrequencyI2cAction::Complete(outcome),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyFrequencyI2cCompletion,
    ) -> Result<(), PhyFrequencyI2cTransitionError> {
        self.step = match (self.step, completion) {
            (
                PhyFrequencyI2cStep::EnableSnapshot,
                PhyFrequencyI2cCompletion::MaskedWrite {
                    address,
                    high_bit: 6,
                    low_bit: 6,
                },
            ) if address == i2c_address(0x62, 0x0b) => PhyFrequencyI2cStep::ReadRfpll,
            (
                PhyFrequencyI2cStep::ReadRfpll,
                PhyFrequencyI2cCompletion::ByteRead { address, value },
            ) if address == i2c_address(0x62, 0x0b) => PhyFrequencyI2cStep::ReadSdm {
                rfpll_register_0b: value,
            },
            (
                PhyFrequencyI2cStep::ReadSdm { rfpll_register_0b },
                PhyFrequencyI2cCompletion::ByteRead { address, value },
            ) if address == i2c_address(0x63, 0) => PhyFrequencyI2cStep::ReadFrontEnd {
                rfpll_register_0b,
                sdm_register_0: value,
            },
            (
                PhyFrequencyI2cStep::ReadFrontEnd {
                    rfpll_register_0b,
                    sdm_register_0,
                },
                PhyFrequencyI2cCompletion::ByteRead { address, value },
            ) if address == i2c_address(0x67, 3) => PhyFrequencyI2cStep::Memory {
                rfpll_register_0b,
                sdm_register_0,
                front_end_register_3: value,
                descriptor_index: 0,
                copy_index: 0,
            },
            (
                PhyFrequencyI2cStep::Memory {
                    rfpll_register_0b,
                    sdm_register_0,
                    front_end_register_3,
                    descriptor_index,
                    copy_index,
                },
                PhyFrequencyI2cCompletion::MemoryWrite {
                    descriptor_index: completed_descriptor,
                    copy_index: completed_copy,
                    address: completed_address,
                },
            ) => {
                let descriptor = frequency_i2c_descriptor(
                    descriptor_index,
                    rfpll_register_0b,
                    sdm_register_0,
                    front_end_register_3,
                    self.request.front_end_parameter_bit,
                );
                let (address, _, _) = descriptor.memory_write(copy_index);
                if descriptor_index != completed_descriptor
                    || copy_index != completed_copy
                    || address != completed_address
                {
                    return Err(PhyFrequencyI2cTransitionError::WrongCompletion);
                }
                if copy_index + 1 != descriptor.memory_write_count() {
                    PhyFrequencyI2cStep::Memory {
                        rfpll_register_0b,
                        sdm_register_0,
                        front_end_register_3,
                        descriptor_index,
                        copy_index: copy_index + 1,
                    }
                } else if descriptor_index == 10 {
                    PhyFrequencyI2cStep::NumberAddresses {
                        rfpll_register_0b,
                        sdm_register_0,
                        front_end_register_3,
                    }
                } else {
                    PhyFrequencyI2cStep::Memory {
                        rfpll_register_0b,
                        sdm_register_0,
                        front_end_register_3,
                        descriptor_index: descriptor_index + 1,
                        copy_index: 0,
                    }
                }
            }
            (
                PhyFrequencyI2cStep::NumberAddresses {
                    rfpll_register_0b,
                    sdm_register_0,
                    front_end_register_3,
                },
                PhyFrequencyI2cCompletion::NumberAddressesConfigured(completed),
            ) => {
                let expected = phy_frequency_i2c_number_address_image(
                    rfpll_register_0b,
                    sdm_register_0,
                    front_end_register_3,
                    self.request.front_end_parameter_bit,
                );
                if completed != expected {
                    return Err(PhyFrequencyI2cTransitionError::WrongCompletion);
                }
                PhyFrequencyI2cStep::Complete(PhyFrequencyI2cOutcome {
                    rfpll_register_0b,
                    sdm_register_0,
                    front_end_register_3,
                    number_addresses: expected,
                })
            }
            (PhyFrequencyI2cStep::Complete(_), _) => {
                return Err(PhyFrequencyI2cTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyFrequencyI2cTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyChannelFrequencyInitControl {
    /// Explicit replacement for the hidden `phy_param[0x193]` branch.
    pub frequency_register_parameter_override: bool,
    /// Rust-owned replacement for `phy_param[0xa4] & 0x20`.
    pub frequency_table_initialized: bool,
    /// Explicit replacement for the bit read from `phy_param[0x1af]`.
    pub front_end_parameter_bit: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyChannelFrequencyInitRequest {
    /// Explicit replacement for the hidden `phy_param[0x193]` branch.
    pub frequency_register_parameter_override: bool,
    /// Rust-owned replacement for `phy_param[0xa4] & 0x20`.
    pub frequency_table_initialized: bool,
    /// Explicit replacement for `phy_param[0x4f]`.
    pub crystal_selector: u8,
    /// Explicit replacement for `phy_param[0x19f]`.
    pub middle_xtal_duty: u8,
    /// Explicit replacement for `phy_param[0x1a0]`.
    pub outer_xtal_duty: u8,
    /// Explicit replacement for the bit read from `phy_param[0x1af]`.
    pub front_end_parameter_bit: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyChannelFrequencyCalibrationOutcome {
    pub nominal: RfpllFrequencyOutcome,
    pub low: RfpllFrequencyOutcome,
    pub high: RfpllFrequencyOutcome,
    pub table: PhyFrequencyTableOutcome,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyChannelFrequencyInitOutcome {
    pub table_was_initialized: bool,
    pub table_is_initialized: bool,
    pub calibration: Option<PhyChannelFrequencyCalibrationOutcome>,
    pub i2c: PhyFrequencyI2cOutcome,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyChannelFrequencyRfpllPoint {
    Nominal,
    Low,
    High,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyChannelFrequencyInitFailure {
    Rfpll {
        point: PhyChannelFrequencyRfpllPoint,
        failure: RfpllFrequencyFailure,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyChannelFrequencyInitAction {
    ConfigureFrequencyRegisters {
        parameter_override: bool,
    },
    WriteMasked {
        address: PhyI2cAddress,
        high_bit: u8,
        low_bit: u8,
        value: u8,
    },
    WriteByte {
        address: PhyI2cAddress,
        value: u8,
    },
    ReadByte {
        address: PhyI2cAddress,
    },
    Rfpll {
        point: PhyChannelFrequencyRfpllPoint,
        action: RfpllFrequencyAction,
    },
    Table(PhyFrequencyTableAction),
    I2c(PhyFrequencyI2cAction),
    Complete(PhyChannelFrequencyInitOutcome),
    Failed(PhyChannelFrequencyInitFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyChannelFrequencyInitCompletion {
    FrequencyRegistersConfigured {
        parameter_override: bool,
    },
    MaskedWrite {
        address: PhyI2cAddress,
        high_bit: u8,
        low_bit: u8,
    },
    ByteWrite {
        address: PhyI2cAddress,
    },
    ByteRead {
        address: PhyI2cAddress,
        value: u8,
    },
    Rfpll(RfpllFrequencyCompletion),
    Table(PhyFrequencyTableCompletion),
    I2c(PhyFrequencyI2cCompletion),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyChannelFrequencyInitTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyChannelFrequencyInitStep {
    ConfigureRegisters,
    InitialCapLow,
    InitialCapHigh,
    DisableRfpll,
    Nominal(RfpllFrequencyTransition),
    EnableCapRange {
        nominal: RfpllFrequencyOutcome,
    },
    EnableRfpll {
        nominal: RfpllFrequencyOutcome,
    },
    Low {
        nominal: RfpllFrequencyOutcome,
        transition: RfpllFrequencyTransition,
    },
    High {
        nominal: RfpllFrequencyOutcome,
        low: RfpllFrequencyOutcome,
        transition: RfpllFrequencyTransition,
    },
    ReadSdmUpper {
        nominal: RfpllFrequencyOutcome,
        low: RfpllFrequencyOutcome,
        high: RfpllFrequencyOutcome,
    },
    Table {
        nominal: RfpllFrequencyOutcome,
        low: RfpllFrequencyOutcome,
        high: RfpllFrequencyOutcome,
        transition: PhyFrequencyTableTransition,
    },
    I2c {
        calibration: Option<PhyChannelFrequencyCalibrationOutcome>,
        transition: PhyFrequencyI2cTransition,
    },
    Complete(PhyChannelFrequencyInitOutcome),
    Failed(PhyChannelFrequencyInitFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyChannelFrequencyRfpllAdvance {
    Pending,
    Complete(RfpllFrequencyOutcome),
    Failed(PhyChannelFrequencyInitFailure),
}

/// Caller-driven replacement for the complete pinned
/// `phy_set_chan_freq_hw_init(2, 4)` graph.
///
/// The transition owns the former `phy_param[0xa4]` initialized bit. Every
/// PHY-I2C transaction, RFPLL timer edge, frequency-memory publication, and
/// final number-address register image is externally completed. There is no
/// allocation, internal polling, delay loop, callback, or mutable C state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyChannelFrequencyInitTransition {
    request: PhyChannelFrequencyInitRequest,
    step: PhyChannelFrequencyInitStep,
}

impl PhyChannelFrequencyInitTransition {
    const RFPLL_ADDRESS: PhyI2cAddress = PhyI2cAddress::new_internal(0x62, 2);
    const SDM_REGISTER_SIX: PhyI2cAddress = PhyI2cAddress::new_internal(0x63, 6);

    pub const fn new(request: PhyChannelFrequencyInitRequest) -> Self {
        Self {
            request,
            step: PhyChannelFrequencyInitStep::ConfigureRegisters,
        }
    }

    const fn rfpll_transition(self, frequency_code: u16) -> RfpllFrequencyTransition {
        RfpllFrequencyTransition::new(RfpllFrequencyRequest {
            crystal_selector: self.request.crystal_selector,
            frequency_code,
            offset: 0,
        })
    }

    pub const fn action(self) -> PhyChannelFrequencyInitAction {
        match self.step {
            PhyChannelFrequencyInitStep::ConfigureRegisters => {
                PhyChannelFrequencyInitAction::ConfigureFrequencyRegisters {
                    parameter_override: self.request.frequency_register_parameter_override,
                }
            }
            PhyChannelFrequencyInitStep::InitialCapLow => {
                PhyChannelFrequencyInitAction::WriteByte {
                    address: i2c_address(0x62, 1),
                    value: 0xc8,
                }
            }
            PhyChannelFrequencyInitStep::InitialCapHigh => {
                PhyChannelFrequencyInitAction::WriteMasked {
                    address: Self::RFPLL_ADDRESS,
                    high_bit: 6,
                    low_bit: 6,
                    value: 0,
                }
            }
            PhyChannelFrequencyInitStep::DisableRfpll => {
                PhyChannelFrequencyInitAction::WriteMasked {
                    address: Self::RFPLL_ADDRESS,
                    high_bit: 7,
                    low_bit: 7,
                    value: 0,
                }
            }
            PhyChannelFrequencyInitStep::Nominal(transition) => {
                PhyChannelFrequencyInitAction::Rfpll {
                    point: PhyChannelFrequencyRfpllPoint::Nominal,
                    action: transition.action(),
                }
            }
            PhyChannelFrequencyInitStep::EnableCapRange { .. } => {
                PhyChannelFrequencyInitAction::WriteMasked {
                    address: Self::RFPLL_ADDRESS,
                    high_bit: 5,
                    low_bit: 0,
                    value: 0x3f,
                }
            }
            PhyChannelFrequencyInitStep::EnableRfpll { .. } => {
                PhyChannelFrequencyInitAction::WriteMasked {
                    address: Self::RFPLL_ADDRESS,
                    high_bit: 7,
                    low_bit: 7,
                    value: 1,
                }
            }
            PhyChannelFrequencyInitStep::Low { transition, .. } => {
                PhyChannelFrequencyInitAction::Rfpll {
                    point: PhyChannelFrequencyRfpllPoint::Low,
                    action: transition.action(),
                }
            }
            PhyChannelFrequencyInitStep::High { transition, .. } => {
                PhyChannelFrequencyInitAction::Rfpll {
                    point: PhyChannelFrequencyRfpllPoint::High,
                    action: transition.action(),
                }
            }
            PhyChannelFrequencyInitStep::ReadSdmUpper { .. } => {
                PhyChannelFrequencyInitAction::ReadByte {
                    address: Self::SDM_REGISTER_SIX,
                }
            }
            PhyChannelFrequencyInitStep::Table { transition, .. } => {
                PhyChannelFrequencyInitAction::Table(transition.action())
            }
            PhyChannelFrequencyInitStep::I2c { transition, .. } => {
                PhyChannelFrequencyInitAction::I2c(transition.action())
            }
            PhyChannelFrequencyInitStep::Complete(outcome) => {
                PhyChannelFrequencyInitAction::Complete(outcome)
            }
            PhyChannelFrequencyInitStep::Failed(failure) => {
                PhyChannelFrequencyInitAction::Failed(failure)
            }
        }
    }

    fn advance_rfpll(
        point: PhyChannelFrequencyRfpllPoint,
        transition: &mut RfpllFrequencyTransition,
        completion: RfpllFrequencyCompletion,
    ) -> Result<PhyChannelFrequencyRfpllAdvance, PhyChannelFrequencyInitTransitionError> {
        transition
            .advance(completion)
            .map_err(|_| PhyChannelFrequencyInitTransitionError::WrongCompletion)?;
        match transition.action() {
            RfpllFrequencyAction::Complete(outcome) => {
                Ok(PhyChannelFrequencyRfpllAdvance::Complete(outcome))
            }
            RfpllFrequencyAction::Failed(failure) => Ok(PhyChannelFrequencyRfpllAdvance::Failed(
                PhyChannelFrequencyInitFailure::Rfpll { point, failure },
            )),
            _ => Ok(PhyChannelFrequencyRfpllAdvance::Pending),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyChannelFrequencyInitCompletion,
    ) -> Result<(), PhyChannelFrequencyInitTransitionError> {
        self.step = match (self.step, completion) {
            (
                PhyChannelFrequencyInitStep::ConfigureRegisters,
                PhyChannelFrequencyInitCompletion::FrequencyRegistersConfigured {
                    parameter_override,
                },
            ) if parameter_override == self.request.frequency_register_parameter_override => {
                if self.request.frequency_table_initialized {
                    PhyChannelFrequencyInitStep::I2c {
                        calibration: None,
                        transition: PhyFrequencyI2cTransition::new(PhyFrequencyI2cRequest {
                            front_end_parameter_bit: self.request.front_end_parameter_bit,
                        }),
                    }
                } else {
                    PhyChannelFrequencyInitStep::InitialCapLow
                }
            }
            (
                PhyChannelFrequencyInitStep::InitialCapLow,
                PhyChannelFrequencyInitCompletion::ByteWrite { address },
            ) if address == i2c_address(0x62, 1) => PhyChannelFrequencyInitStep::InitialCapHigh,
            (
                PhyChannelFrequencyInitStep::InitialCapHigh,
                PhyChannelFrequencyInitCompletion::MaskedWrite {
                    address,
                    high_bit: 6,
                    low_bit: 6,
                },
            ) if address == Self::RFPLL_ADDRESS => PhyChannelFrequencyInitStep::DisableRfpll,
            (
                PhyChannelFrequencyInitStep::DisableRfpll,
                PhyChannelFrequencyInitCompletion::MaskedWrite {
                    address,
                    high_bit: 7,
                    low_bit: 7,
                },
            ) if address == Self::RFPLL_ADDRESS => {
                PhyChannelFrequencyInitStep::Nominal(self.rfpll_transition(0x985))
            }
            (
                PhyChannelFrequencyInitStep::Nominal(mut transition),
                PhyChannelFrequencyInitCompletion::Rfpll(completion),
            ) => {
                match Self::advance_rfpll(
                    PhyChannelFrequencyRfpllPoint::Nominal,
                    &mut transition,
                    completion,
                )? {
                    PhyChannelFrequencyRfpllAdvance::Complete(nominal) => {
                        PhyChannelFrequencyInitStep::EnableCapRange { nominal }
                    }
                    PhyChannelFrequencyRfpllAdvance::Pending => {
                        PhyChannelFrequencyInitStep::Nominal(transition)
                    }
                    PhyChannelFrequencyRfpllAdvance::Failed(failure) => {
                        PhyChannelFrequencyInitStep::Failed(failure)
                    }
                }
            }
            (
                PhyChannelFrequencyInitStep::EnableCapRange { nominal },
                PhyChannelFrequencyInitCompletion::MaskedWrite {
                    address,
                    high_bit: 5,
                    low_bit: 0,
                },
            ) if address == Self::RFPLL_ADDRESS => {
                PhyChannelFrequencyInitStep::EnableRfpll { nominal }
            }
            (
                PhyChannelFrequencyInitStep::EnableRfpll { nominal },
                PhyChannelFrequencyInitCompletion::MaskedWrite {
                    address,
                    high_bit: 7,
                    low_bit: 7,
                },
            ) if address == Self::RFPLL_ADDRESS => PhyChannelFrequencyInitStep::Low {
                nominal,
                transition: self.rfpll_transition(0x960),
            },
            (
                PhyChannelFrequencyInitStep::Low {
                    nominal,
                    mut transition,
                },
                PhyChannelFrequencyInitCompletion::Rfpll(completion),
            ) => {
                match Self::advance_rfpll(
                    PhyChannelFrequencyRfpllPoint::Low,
                    &mut transition,
                    completion,
                )? {
                    PhyChannelFrequencyRfpllAdvance::Complete(low) => {
                        PhyChannelFrequencyInitStep::High {
                            nominal,
                            low,
                            transition: self.rfpll_transition(0x9a0),
                        }
                    }
                    PhyChannelFrequencyRfpllAdvance::Pending => PhyChannelFrequencyInitStep::Low {
                        nominal,
                        transition,
                    },
                    PhyChannelFrequencyRfpllAdvance::Failed(failure) => {
                        PhyChannelFrequencyInitStep::Failed(failure)
                    }
                }
            }
            (
                PhyChannelFrequencyInitStep::High {
                    nominal,
                    low,
                    mut transition,
                },
                PhyChannelFrequencyInitCompletion::Rfpll(completion),
            ) => {
                match Self::advance_rfpll(
                    PhyChannelFrequencyRfpllPoint::High,
                    &mut transition,
                    completion,
                )? {
                    PhyChannelFrequencyRfpllAdvance::Complete(high) => {
                        PhyChannelFrequencyInitStep::ReadSdmUpper { nominal, low, high }
                    }
                    PhyChannelFrequencyRfpllAdvance::Pending => PhyChannelFrequencyInitStep::High {
                        nominal,
                        low,
                        transition,
                    },
                    PhyChannelFrequencyRfpllAdvance::Failed(failure) => {
                        PhyChannelFrequencyInitStep::Failed(failure)
                    }
                }
            }
            (
                PhyChannelFrequencyInitStep::ReadSdmUpper { nominal, low, high },
                PhyChannelFrequencyInitCompletion::ByteRead { address, value },
            ) if address == Self::SDM_REGISTER_SIX => PhyChannelFrequencyInitStep::Table {
                nominal,
                low,
                high,
                transition: PhyFrequencyTableTransition::new(PhyFrequencyTableRequest {
                    parameters: PhyFrequencyTableParameters {
                        crystal_selector: self.request.crystal_selector,
                        middle_xtal_duty: self.request.middle_xtal_duty,
                        outer_xtal_duty: self.request.outer_xtal_duty,
                        sdm_register_six_upper: value & 0xf8,
                    },
                    // The pinned archive redundantly reads each cap immediately
                    // after the corresponding complete RFPLL call. The child
                    // transition already owns those exact final values.
                    low_frequency_cap: low.final_cap,
                    high_frequency_cap: high.final_cap,
                }),
            },
            (
                PhyChannelFrequencyInitStep::Table {
                    nominal,
                    low,
                    high,
                    mut transition,
                },
                PhyChannelFrequencyInitCompletion::Table(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyChannelFrequencyInitTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyFrequencyTableAction::Complete(table) => PhyChannelFrequencyInitStep::I2c {
                        calibration: Some(PhyChannelFrequencyCalibrationOutcome {
                            nominal,
                            low,
                            high,
                            table,
                        }),
                        transition: PhyFrequencyI2cTransition::new(PhyFrequencyI2cRequest {
                            front_end_parameter_bit: self.request.front_end_parameter_bit,
                        }),
                    },
                    _ => PhyChannelFrequencyInitStep::Table {
                        nominal,
                        low,
                        high,
                        transition,
                    },
                }
            }
            (
                PhyChannelFrequencyInitStep::I2c {
                    calibration,
                    mut transition,
                },
                PhyChannelFrequencyInitCompletion::I2c(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyChannelFrequencyInitTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyFrequencyI2cAction::Complete(i2c) => {
                        PhyChannelFrequencyInitStep::Complete(PhyChannelFrequencyInitOutcome {
                            table_was_initialized: self.request.frequency_table_initialized,
                            table_is_initialized: true,
                            calibration,
                            i2c,
                        })
                    }
                    _ => PhyChannelFrequencyInitStep::I2c {
                        calibration,
                        transition,
                    },
                }
            }
            (PhyChannelFrequencyInitStep::Complete(_), _)
            | (PhyChannelFrequencyInitStep::Failed(_), _) => {
                return Err(PhyChannelFrequencyInitTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyChannelFrequencyInitTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        i2c_address, phy_frequency_i2c_number_address_image, phy_frequency_memory_record,
        phy_frequency_xtal_duty, PhyChannelFrequencyInitAction, PhyChannelFrequencyInitCompletion,
        PhyChannelFrequencyInitRequest, PhyChannelFrequencyInitTransition,
        PhyChannelFrequencyRfpllPoint, PhyFrequencyI2cAction, PhyFrequencyI2cCompletion,
        PhyFrequencyI2cRequest, PhyFrequencyI2cTransition, PhyFrequencyI2cTransitionError,
        PhyFrequencyTableAction, PhyFrequencyTableCompletion, PhyFrequencyTableParameters,
        PhyFrequencyTableRequest, PhyFrequencyTableTransition, PhyFrequencyTableTransitionError,
        PHY_FREQUENCY_TABLE_ENTRY_COUNT,
    };
    use crate::phy_rfpll::{RfpllFrequencyAction, RfpllFrequencyCompletion};

    const REQUEST: PhyFrequencyTableRequest = PhyFrequencyTableRequest {
        parameters: PhyFrequencyTableParameters {
            crystal_selector: 0x31,
            middle_xtal_duty: 0x2a,
            outer_xtal_duty: 0x35,
            sdm_register_six_upper: 0xa8,
        },
        low_frequency_cap: 0x0c8,
        high_frequency_cap: 0x118,
    };

    #[test]
    fn crystal_duty_preserves_both_unsigned_vendor_boundaries() {
        assert_eq!(phy_frequency_xtal_duty(0x967, 0x2a, 0x35), 17);
        assert_eq!(phy_frequency_xtal_duty(0x968, 0x2a, 0x35), 0x35);
        assert_eq!(phy_frequency_xtal_duty(0x974, 0x2a, 0x35), 0x35);
        assert_eq!(phy_frequency_xtal_duty(0x975, 0x2a, 0x35), 0x2a);
        assert_eq!(phy_frequency_xtal_duty(0x99b, 0x2a, 0x35), 0x2a);
        assert_eq!(phy_frequency_xtal_duty(0x99c, 0x2a, 0x35), 0x35);
    }

    #[test]
    fn record_packs_cap_sdm_and_duty_without_a_backing_table() {
        assert_eq!(
            phy_frequency_memory_record(REQUEST, 0).words(),
            [0x00a8_bfc8, 0x0030_0000, 17]
        );
        assert_eq!(
            phy_frequency_memory_record(REQUEST, 64).words(),
            [0x00a9_ff18, 0x0032_2222, 0x35]
        );
        assert_eq!(
            phy_frequency_memory_record(REQUEST, 84).words(),
            [0x00ae_ff31, 0x0032_cccc, 0x35]
        );
    }

    #[test]
    fn transition_publishes_exactly_three_words_for_all_85_entries() {
        let mut transition = PhyFrequencyTableTransition::new(REQUEST);
        let mut writes = 0;
        loop {
            match transition.action() {
                PhyFrequencyTableAction::WriteMemory {
                    entry_index,
                    word_index,
                    address,
                    mode,
                    ..
                } => {
                    assert_eq!(mode, 7);
                    assert_eq!(
                        address,
                        0x12 + u16::from(entry_index) * 7 + u16::from(word_index) * 3
                    );
                    transition
                        .advance(PhyFrequencyTableCompletion {
                            entry_index,
                            word_index,
                            address,
                        })
                        .unwrap();
                    writes += 1;
                }
                PhyFrequencyTableAction::Complete(outcome) => {
                    assert_eq!(outcome.entries_written, PHY_FREQUENCY_TABLE_ENTRY_COUNT);
                    break;
                }
            }
        }
        assert_eq!(writes, usize::from(PHY_FREQUENCY_TABLE_ENTRY_COUNT) * 3);
        assert_eq!(
            transition.advance(PhyFrequencyTableCompletion {
                entry_index: 0,
                word_index: 0,
                address: 0x12,
            }),
            Err(PhyFrequencyTableTransitionError::AlreadyComplete)
        );
    }

    #[test]
    fn transition_rejects_out_of_order_memory_completion() {
        let mut transition = PhyFrequencyTableTransition::new(REQUEST);
        assert_eq!(
            transition.advance(PhyFrequencyTableCompletion {
                entry_index: 0,
                word_index: 1,
                address: 0x15,
            }),
            Err(PhyFrequencyTableTransitionError::WrongCompletion)
        );
    }

    #[test]
    fn i2c_number_address_image_matches_the_pinned_rom_packing() {
        assert_eq!(
            phy_frequency_i2c_number_address_image(0x5a, 0x8f, 0x10, false),
            super::PhyFrequencyI2cNumberAddressImage {
                control_field: 0x0000_a400,
                words: [0x0494_1cc1, 0x0000_0543, 0],
            }
        );
        assert_eq!(
            phy_frequency_i2c_number_address_image(0x5a, 0x8f, 0x10, true),
            super::PhyFrequencyI2cNumberAddressImage {
                control_field: 0x0000_a400,
                words: [0x0494_1cc1, 0x0000_1543, 0],
            }
        );
    }

    fn complete_i2c_snapshot(
        transition: &mut PhyFrequencyI2cTransition,
        rfpll_register_0b: u8,
        sdm_register_0: u8,
        front_end_register_3: u8,
    ) {
        assert_eq!(
            transition.action(),
            PhyFrequencyI2cAction::WriteMasked {
                address: i2c_address(0x62, 0x0b),
                high_bit: 6,
                low_bit: 6,
                value: 1,
            }
        );
        transition
            .advance(PhyFrequencyI2cCompletion::MaskedWrite {
                address: i2c_address(0x62, 0x0b),
                high_bit: 6,
                low_bit: 6,
            })
            .unwrap();
        for (address, value) in [
            (i2c_address(0x62, 0x0b), rfpll_register_0b),
            (i2c_address(0x63, 0), sdm_register_0),
            (i2c_address(0x67, 3), front_end_register_3),
        ] {
            assert_eq!(
                transition.action(),
                PhyFrequencyI2cAction::ReadByte { address }
            );
            transition
                .advance(PhyFrequencyI2cCompletion::ByteRead { address, value })
                .unwrap();
        }
    }

    fn collect_i2c_memory_writes(
        transition: &mut PhyFrequencyI2cTransition,
    ) -> std::vec::Vec<(u8, u8, u16, u32, u8)> {
        let mut writes = std::vec::Vec::new();
        loop {
            let PhyFrequencyI2cAction::WriteMemory {
                descriptor_index,
                copy_index,
                address,
                value,
                mode,
            } = transition.action()
            else {
                break;
            };
            writes.push((descriptor_index, copy_index, address, value, mode));
            transition
                .advance(PhyFrequencyI2cCompletion::MemoryWrite {
                    descriptor_index,
                    copy_index,
                    address,
                })
                .unwrap();
        }
        writes
    }

    #[test]
    fn i2c_transition_publishes_the_fixed_graph_and_dynamic_kind_one_tail() {
        let mut transition = PhyFrequencyI2cTransition::new(PhyFrequencyI2cRequest {
            front_end_parameter_bit: false,
        });
        complete_i2c_snapshot(&mut transition, 0x5a, 0x8f, 0x10);

        let writes = collect_i2c_memory_writes(&mut transition);
        assert_eq!(writes.len(), 11);
        assert_eq!(writes[2], (2, 0, 9, 0x0087_6300, 7));
        assert_eq!(writes[7], (7, 0, 12, 0x008f_6300, 7));
        assert_eq!(writes[8], (8, 0, 15, 0x005a_620b, 7));
        assert_eq!(writes[10], (10, 0, 9, 0x0001_6703, 7));

        let image = phy_frequency_i2c_number_address_image(0x5a, 0x8f, 0x10, false);
        assert_eq!(
            transition.action(),
            PhyFrequencyI2cAction::ConfigureNumberAddresses(image)
        );
        transition
            .advance(PhyFrequencyI2cCompletion::NumberAddressesConfigured(image))
            .unwrap();
        let PhyFrequencyI2cAction::Complete(outcome) = transition.action() else {
            panic!("transition did not complete");
        };
        assert_eq!(outcome.rfpll_register_0b, 0x5a);
        assert_eq!(outcome.sdm_register_0, 0x8f);
        assert_eq!(outcome.front_end_register_3, 0x10);
        assert_eq!(outcome.number_addresses, image);
        assert_eq!(
            transition.advance(PhyFrequencyI2cCompletion::NumberAddressesConfigured(image)),
            Err(PhyFrequencyI2cTransitionError::AlreadyComplete)
        );
    }

    #[test]
    fn i2c_transition_expands_a_dynamic_kind_zero_tail_to_three_writes() {
        let mut transition = PhyFrequencyI2cTransition::new(PhyFrequencyI2cRequest {
            front_end_parameter_bit: false,
        });
        complete_i2c_snapshot(&mut transition, 0x5a, 0x8f, 0);

        let writes = collect_i2c_memory_writes(&mut transition);
        assert_eq!(writes.len(), 13);
        assert_eq!(
            &writes[10..],
            &[
                (10, 0, 0, 0x0001_6703, 7),
                (10, 1, 3, 0x0000_6703, 7),
                (10, 2, 6, 0x0000_6703, 7),
            ]
        );
    }

    #[test]
    fn i2c_transition_rejects_a_completion_for_another_snapshot_register() {
        let mut transition = PhyFrequencyI2cTransition::new(PhyFrequencyI2cRequest {
            front_end_parameter_bit: false,
        });
        assert_eq!(
            transition.advance(PhyFrequencyI2cCompletion::MaskedWrite {
                address: i2c_address(0x62, 0x0a),
                high_bit: 6,
                low_bit: 6,
            }),
            Err(PhyFrequencyI2cTransitionError::WrongCompletion)
        );
    }

    const CHANNEL_REQUEST: PhyChannelFrequencyInitRequest = PhyChannelFrequencyInitRequest {
        frequency_register_parameter_override: false,
        frequency_table_initialized: false,
        crystal_selector: 0x31,
        middle_xtal_duty: 0x2a,
        outer_xtal_duty: 0x35,
        front_end_parameter_bit: false,
    };

    fn point_index(point: PhyChannelFrequencyRfpllPoint) -> usize {
        match point {
            PhyChannelFrequencyRfpllPoint::Nominal => 0,
            PhyChannelFrequencyRfpllPoint::Low => 1,
            PhyChannelFrequencyRfpllPoint::High => 2,
        }
    }

    fn rfpll_completion(
        point: PhyChannelFrequencyRfpllPoint,
        action: RfpllFrequencyAction,
        status_reads: &mut [u8; 3],
    ) -> RfpllFrequencyCompletion {
        match action {
            RfpllFrequencyAction::WriteMasked {
                address,
                high_bit,
                low_bit,
                ..
            } => RfpllFrequencyCompletion::MaskedWrite {
                address,
                high_bit,
                low_bit,
            },
            RfpllFrequencyAction::WriteByte { address, .. } => {
                RfpllFrequencyCompletion::ByteWrite { address }
            }
            RfpllFrequencyAction::ReadMasked {
                address,
                high_bit,
                low_bit,
            } => RfpllFrequencyCompletion::MaskedRead {
                address,
                high_bit,
                low_bit,
                value: if high_bit == 1 { 1 } else { 0 },
            },
            RfpllFrequencyAction::ReadByte { address } if address == i2c_address(0x62, 5) => {
                RfpllFrequencyCompletion::ByteRead {
                    address,
                    value: match point {
                        PhyChannelFrequencyRfpllPoint::Nominal => 0xc8,
                        PhyChannelFrequencyRfpllPoint::Low => 0x80,
                        PhyChannelFrequencyRfpllPoint::High => 0xc0,
                    },
                }
            }
            RfpllFrequencyAction::ReadByte { address } => {
                let index = point_index(point);
                let status = if status_reads[index] & 1 == 0 { 0 } else { 1 };
                status_reads[index] += 1;
                RfpllFrequencyCompletion::ByteRead {
                    address,
                    value: status << 2,
                }
            }
            RfpllFrequencyAction::DelayMicros(micros) => {
                RfpllFrequencyCompletion::DelayElapsed(micros)
            }
            action => panic!("unexpected terminal RFPLL action: {action:?}"),
        }
    }

    fn i2c_completion(action: PhyFrequencyI2cAction) -> PhyFrequencyI2cCompletion {
        match action {
            PhyFrequencyI2cAction::WriteMasked {
                address,
                high_bit,
                low_bit,
                ..
            } => PhyFrequencyI2cCompletion::MaskedWrite {
                address,
                high_bit,
                low_bit,
            },
            PhyFrequencyI2cAction::ReadByte { address } => {
                let value = if address == i2c_address(0x62, 0x0b) {
                    0x5a
                } else if address == i2c_address(0x63, 0) {
                    0x8f
                } else {
                    0x10
                };
                PhyFrequencyI2cCompletion::ByteRead { address, value }
            }
            PhyFrequencyI2cAction::WriteMemory {
                descriptor_index,
                copy_index,
                address,
                ..
            } => PhyFrequencyI2cCompletion::MemoryWrite {
                descriptor_index,
                copy_index,
                address,
            },
            PhyFrequencyI2cAction::ConfigureNumberAddresses(image) => {
                PhyFrequencyI2cCompletion::NumberAddressesConfigured(image)
            }
            action => panic!("unexpected terminal I2C action: {action:?}"),
        }
    }

    #[test]
    fn channel_frequency_init_composes_the_complete_cold_graph() {
        let mut transition = PhyChannelFrequencyInitTransition::new(CHANNEL_REQUEST);
        let mut status_reads = [0_u8; 3];
        let mut rfpll_actions = [0_usize; 3];
        let mut table_writes = 0_usize;
        let mut steps = 0_usize;

        loop {
            steps += 1;
            assert!(steps < 1_000);
            let completion = match transition.action() {
                PhyChannelFrequencyInitAction::ConfigureFrequencyRegisters {
                    parameter_override,
                } => PhyChannelFrequencyInitCompletion::FrequencyRegistersConfigured {
                    parameter_override,
                },
                PhyChannelFrequencyInitAction::WriteMasked {
                    address,
                    high_bit,
                    low_bit,
                    ..
                } => PhyChannelFrequencyInitCompletion::MaskedWrite {
                    address,
                    high_bit,
                    low_bit,
                },
                PhyChannelFrequencyInitAction::WriteByte { address, .. } => {
                    PhyChannelFrequencyInitCompletion::ByteWrite { address }
                }
                PhyChannelFrequencyInitAction::ReadByte { address } => {
                    assert_eq!(address, i2c_address(0x63, 6));
                    PhyChannelFrequencyInitCompletion::ByteRead {
                        address,
                        value: 0xab,
                    }
                }
                PhyChannelFrequencyInitAction::Rfpll { point, action } => {
                    rfpll_actions[point_index(point)] += 1;
                    PhyChannelFrequencyInitCompletion::Rfpll(rfpll_completion(
                        point,
                        action,
                        &mut status_reads,
                    ))
                }
                PhyChannelFrequencyInitAction::Table(PhyFrequencyTableAction::WriteMemory {
                    entry_index,
                    word_index,
                    address,
                    ..
                }) => {
                    table_writes += 1;
                    PhyChannelFrequencyInitCompletion::Table(PhyFrequencyTableCompletion {
                        entry_index,
                        word_index,
                        address,
                    })
                }
                PhyChannelFrequencyInitAction::I2c(action) => {
                    PhyChannelFrequencyInitCompletion::I2c(i2c_completion(action))
                }
                PhyChannelFrequencyInitAction::Complete(outcome) => {
                    assert!(!outcome.table_was_initialized);
                    assert!(outcome.table_is_initialized);
                    let calibration = outcome.calibration.unwrap();
                    assert_eq!(calibration.nominal.final_cap, 0xc9);
                    assert_eq!(calibration.low.final_cap, 0x81);
                    assert_eq!(calibration.high.final_cap, 0xc1);
                    assert_eq!(calibration.table.entries_written, 85);
                    assert_eq!(calibration.table.low_frequency_cap, 0x81);
                    assert_eq!(calibration.table.high_frequency_cap, 0xc1);
                    assert_eq!(outcome.i2c.sdm_register_0, 0x8f);
                    break;
                }
                action => panic!("unexpected terminal channel action: {action:?}"),
            };
            transition.advance(completion).unwrap();
        }

        assert!(rfpll_actions.iter().all(|count| *count != 0));
        assert_eq!(status_reads, [4, 4, 4]);
        assert_eq!(table_writes, 85 * 3);
    }

    #[test]
    fn warm_channel_frequency_init_skips_calibration_but_refreshes_i2c_graph() {
        let mut transition =
            PhyChannelFrequencyInitTransition::new(PhyChannelFrequencyInitRequest {
                frequency_table_initialized: true,
                ..CHANNEL_REQUEST
            });
        assert_eq!(
            transition.action(),
            PhyChannelFrequencyInitAction::ConfigureFrequencyRegisters {
                parameter_override: false,
            }
        );
        transition
            .advance(
                PhyChannelFrequencyInitCompletion::FrequencyRegistersConfigured {
                    parameter_override: false,
                },
            )
            .unwrap();

        loop {
            match transition.action() {
                PhyChannelFrequencyInitAction::I2c(action) => transition
                    .advance(PhyChannelFrequencyInitCompletion::I2c(i2c_completion(
                        action,
                    )))
                    .unwrap(),
                PhyChannelFrequencyInitAction::Complete(outcome) => {
                    assert!(outcome.table_was_initialized);
                    assert!(outcome.table_is_initialized);
                    assert_eq!(outcome.calibration, None);
                    break;
                }
                action => panic!("warm path must not calibrate: {action:?}"),
            }
        }
    }
}
