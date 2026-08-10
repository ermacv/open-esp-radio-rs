//! Symbolic inputs and bit-image constructors for observed reads.

use super::*;

impl SymbolicValue {
    pub fn input(index: u8) -> Self {
        Self::Input { index }
    }

    pub fn bits(&self) -> [BitSource; 32] {
        match self {
            Self::Unknown => [BitSource::Unknown; 32],
            Self::Constant(value) => {
                core::array::from_fn(|bit| BitSource::Constant(value & (1 << bit) != 0))
            }
            Self::Input { index } | Self::InputConstant { index, .. } => {
                core::array::from_fn(|bit| BitSource::Input {
                    index: *index,
                    bit: bit as u8,
                    inverted: false,
                })
            }
            Self::StackAddress(_)
            | Self::SymbolAddress { .. }
            | Self::ReviewedExternalTable(_)
            | Self::ReviewedExternalFunction { .. }
            | Self::FunctionTable(_)
            | Self::FunctionPointer { .. }
            | Self::Expression { .. }
            | Self::WideSignedDivide { .. } => [BitSource::Unknown; 32],
            Self::CallResult(call_token) => core::array::from_fn(|bit| BitSource::CallResult {
                call_token: *call_token,
                bit: bit as u8,
                inverted: false,
            }),
            Self::ExternalResult(call_token) => {
                core::array::from_fn(|bit| BitSource::ExternalResult {
                    call_token: *call_token,
                    bit: bit as u8,
                    inverted: false,
                })
            }
            Self::RegisterImage {
                read_token,
                address,
                and_mask,
                or_mask,
            } => core::array::from_fn(|bit| {
                if or_mask & (1 << bit) != 0 {
                    BitSource::Constant(true)
                } else if and_mask & (1 << bit) != 0 {
                    BitSource::Register {
                        read_token: *read_token,
                        address: *address,
                        bit: bit as u8,
                        inverted: false,
                    }
                } else {
                    BitSource::Constant(false)
                }
            }),
            Self::IndexedRegisterImage {
                read_token,
                and_mask,
                or_mask,
            } => core::array::from_fn(|bit| {
                if or_mask & (1 << bit) != 0 {
                    BitSource::Constant(true)
                } else if and_mask & (1 << bit) != 0 {
                    BitSource::IndexedRegister {
                        read_token: *read_token,
                        bit: bit as u8,
                        inverted: false,
                    }
                } else {
                    BitSource::Constant(false)
                }
            }),
            Self::MemoryImage {
                read_token,
                and_mask,
                or_mask,
            } => core::array::from_fn(|bit| {
                if or_mask & (1 << bit) != 0 {
                    BitSource::Constant(true)
                } else if and_mask & (1 << bit) != 0 {
                    BitSource::Memory {
                        read_token: *read_token,
                        bit: bit as u8,
                        inverted: false,
                    }
                } else {
                    BitSource::Constant(false)
                }
            }),
            Self::Bits(bits) => **bits,
        }
    }

    pub fn register_read(read_token: u32, address: u32, width: u8, signed: bool) -> Self {
        if width == 32 {
            return Self::RegisterImage {
                read_token,
                address,
                and_mask: u32::MAX,
                or_mask: 0,
            };
        }
        Self::from_bits(core::array::from_fn(|bit| {
            if bit < usize::from(width) {
                BitSource::Register {
                    read_token,
                    address,
                    bit: bit as u8,
                    inverted: false,
                }
            } else if signed {
                BitSource::Register {
                    read_token,
                    address,
                    bit: width - 1,
                    inverted: false,
                }
            } else {
                BitSource::Constant(false)
            }
        }))
    }

    pub fn indexed_register_read(read_token: u32, width: u8, signed: bool) -> Self {
        if width == 32 {
            return Self::IndexedRegisterImage {
                read_token,
                and_mask: u32::MAX,
                or_mask: 0,
            };
        }
        Self::from_bits(core::array::from_fn(|bit| {
            if bit < usize::from(width) {
                BitSource::IndexedRegister {
                    read_token,
                    bit: bit as u8,
                    inverted: false,
                }
            } else if signed {
                BitSource::IndexedRegister {
                    read_token,
                    bit: width - 1,
                    inverted: false,
                }
            } else {
                BitSource::Constant(false)
            }
        }))
    }

    pub fn memory_read(read_token: u32, width: u8, signed: bool) -> Self {
        if width == 32 {
            return Self::MemoryImage {
                read_token,
                and_mask: u32::MAX,
                or_mask: 0,
            };
        }
        Self::from_bits(core::array::from_fn(|bit| {
            if bit < usize::from(width) {
                BitSource::Memory {
                    read_token,
                    bit: bit as u8,
                    inverted: false,
                }
            } else if signed {
                BitSource::Memory {
                    read_token,
                    bit: width - 1,
                    inverted: false,
                }
            } else {
                BitSource::Constant(false)
            }
        }))
    }

    pub fn private_stack_read(read_token: u32, width: u8, signed: bool) -> Self {
        Self::from_bits(core::array::from_fn(|bit| {
            if bit < usize::from(width) {
                BitSource::PrivateStack {
                    read_token,
                    bit: bit as u8,
                    inverted: false,
                }
            } else if signed {
                BitSource::PrivateStack {
                    read_token,
                    bit: width - 1,
                    inverted: false,
                }
            } else {
                BitSource::Constant(false)
            }
        }))
    }

    pub fn from_bits(bits: [BitSource; 32]) -> Self {
        let mut constant = 0_u32;
        let mut all_constant = true;
        for (bit, source) in bits.iter().enumerate() {
            match source {
                BitSource::Constant(true) => constant |= 1 << bit,
                BitSource::Constant(false) => {}
                _ => all_constant = false,
            }
        }
        if all_constant {
            return Self::Constant(constant);
        }

        let register = bits.iter().find_map(|source| match source {
            BitSource::Register {
                read_token,
                address,
                ..
            } => Some((*read_token, *address)),
            _ => None,
        });
        if let Some((read_token, address)) = register {
            let mut and_mask = 0_u32;
            let mut or_mask = 0_u32;
            let mut register_image = true;
            for (bit, source) in bits.iter().enumerate() {
                match source {
                    BitSource::Register {
                        read_token: source_token,
                        address: source_address,
                        bit: source_bit,
                        inverted: false,
                    } if *source_token == read_token
                        && *source_address == address
                        && usize::from(*source_bit) == bit =>
                    {
                        and_mask |= 1 << bit;
                    }
                    BitSource::Constant(true) => or_mask |= 1 << bit,
                    BitSource::Constant(false) => {}
                    _ => register_image = false,
                }
            }
            if register_image {
                return Self::RegisterImage {
                    read_token,
                    address,
                    and_mask,
                    or_mask,
                };
            }
        }

        let indexed_register = bits.iter().find_map(|source| match source {
            BitSource::IndexedRegister { read_token, .. } => Some(*read_token),
            _ => None,
        });
        if let Some(read_token) = indexed_register {
            let mut and_mask = 0_u32;
            let mut or_mask = 0_u32;
            let mut register_image = true;
            for (bit, source) in bits.iter().enumerate() {
                match source {
                    BitSource::IndexedRegister {
                        read_token: source_token,
                        bit: source_bit,
                        inverted: false,
                    } if *source_token == read_token && usize::from(*source_bit) == bit => {
                        and_mask |= 1 << bit;
                    }
                    BitSource::Constant(true) => or_mask |= 1 << bit,
                    BitSource::Constant(false) => {}
                    _ => register_image = false,
                }
            }
            if register_image {
                return Self::IndexedRegisterImage {
                    read_token,
                    and_mask,
                    or_mask,
                };
            }
        }

        let memory = bits.iter().find_map(|source| match source {
            BitSource::Memory { read_token, .. } => Some(*read_token),
            _ => None,
        });
        if let Some(read_token) = memory {
            let mut and_mask = 0_u32;
            let mut or_mask = 0_u32;
            let mut memory_image = true;
            for (bit, source) in bits.iter().enumerate() {
                match source {
                    BitSource::Memory {
                        read_token: source_token,
                        bit: source_bit,
                        inverted: false,
                    } if *source_token == read_token && usize::from(*source_bit) == bit => {
                        and_mask |= 1 << bit;
                    }
                    BitSource::Constant(true) => or_mask |= 1 << bit,
                    BitSource::Constant(false) => {}
                    _ => memory_image = false,
                }
            }
            if memory_image {
                return Self::MemoryImage {
                    read_token,
                    and_mask,
                    or_mask,
                };
            }
        }
        let value = Self::Bits(Box::new(bits));
        if let Some(index) = value.direct_input_index() {
            return Self::Input { index };
        }
        value
    }
}
