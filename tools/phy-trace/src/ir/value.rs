//! Symbolic RV32 values and proven indexed-MMIO domains.

use std::collections::BTreeMap;

use crate::{Rv32CallArguments, entry_contract, external_abi};

pub(crate) const PRIVATE_STACK_READ_TOKEN_FLAG: u32 = 1 << 31;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SymbolicValue {
    Unknown,
    Constant(u32),
    InputConstant {
        index: u8,
        value: u32,
    },
    StackAddress(i32),
    SymbolAddress {
        member: Option<String>,
        symbol: String,
        hi_addend: i64,
        lo_addend: Option<i64>,
        post_offset: i64,
    },
    CallResult(u32),
    ExternalTable(external_abi::Table),
    ExternalFunction {
        table: external_abi::Table,
        function: external_abi::Function,
    },
    FunctionTable(entry_contract::FunctionTable),
    FunctionPointer {
        table: entry_contract::FunctionTable,
        target: u32,
    },
    ExternalResult(u32),
    Expression {
        operation: ExpressionOperation,
        left: Box<SymbolicValue>,
        right: Box<SymbolicValue>,
    },
    WideSignedDivide {
        dividend_low: Box<SymbolicValue>,
        dividend_high: Box<SymbolicValue>,
        divisor_low: Box<SymbolicValue>,
        divisor_high: Box<SymbolicValue>,
        high_word: bool,
    },
    RegisterImage {
        read_token: u32,
        address: u32,
        and_mask: u32,
        or_mask: u32,
    },
    IndexedRegisterImage {
        read_token: u32,
        and_mask: u32,
        or_mask: u32,
    },
    MemoryImage {
        read_token: u32,
        and_mask: u32,
        or_mask: u32,
    },
    Bits(Box<[BitSource; 32]>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ExpressionOperation {
    Add,
    Subtract,
    Multiply,
    DivideSigned,
    DivideUnsigned,
    RemainderSigned,
    RemainderUnsigned,
    BitAnd,
    BitOr,
    BitXor,
    ShiftLeft,
    ShiftRight,
    ShiftRightArithmetic,
    Equal,
    LessThanSigned,
    LessThanUnsigned,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BitSource {
    Unknown,
    Constant(bool),
    Input {
        index: u8,
        bit: u8,
        inverted: bool,
    },
    Register {
        read_token: u32,
        address: u32,
        bit: u8,
        inverted: bool,
    },
    IndexedRegister {
        read_token: u32,
        bit: u8,
        inverted: bool,
    },
    Memory {
        read_token: u32,
        bit: u8,
        inverted: bool,
    },
    PrivateStack {
        read_token: u32,
        bit: u8,
        inverted: bool,
    },
    CallResult {
        call_token: u32,
        bit: u8,
        inverted: bool,
    },
    ExternalResult {
        call_token: u32,
        bit: u8,
        inverted: bool,
    },
}

impl BitSource {
    pub(crate) fn inverted(self) -> Self {
        match self {
            Self::Unknown => Self::Unknown,
            Self::Constant(value) => Self::Constant(!value),
            Self::Input {
                index,
                bit,
                inverted,
            } => Self::Input {
                index,
                bit,
                inverted: !inverted,
            },
            Self::Register {
                read_token,
                address,
                bit,
                inverted,
            } => Self::Register {
                read_token,
                address,
                bit,
                inverted: !inverted,
            },
            Self::IndexedRegister {
                read_token,
                bit,
                inverted,
            } => Self::IndexedRegister {
                read_token,
                bit,
                inverted: !inverted,
            },
            Self::Memory {
                read_token,
                bit,
                inverted,
            } => Self::Memory {
                read_token,
                bit,
                inverted: !inverted,
            },
            Self::PrivateStack {
                read_token,
                bit,
                inverted,
            } => Self::PrivateStack {
                read_token,
                bit,
                inverted: !inverted,
            },
            Self::CallResult {
                call_token,
                bit,
                inverted,
            } => Self::CallResult {
                call_token,
                bit,
                inverted: !inverted,
            },
            Self::ExternalResult {
                call_token,
                bit,
                inverted,
            } => Self::ExternalResult {
                call_token,
                bit,
                inverted: !inverted,
            },
        }
    }
}

impl SymbolicValue {
    pub(crate) fn input(index: u8) -> Self {
        Self::Bits(Box::new(core::array::from_fn(|bit| BitSource::Input {
            index,
            bit: bit as u8,
            inverted: false,
        })))
    }

    pub(crate) fn bits(&self) -> [BitSource; 32] {
        match self {
            Self::Unknown => [BitSource::Unknown; 32],
            Self::Constant(value) => {
                core::array::from_fn(|bit| BitSource::Constant(value & (1 << bit) != 0))
            }
            Self::InputConstant { index, .. } => core::array::from_fn(|bit| BitSource::Input {
                index: *index,
                bit: bit as u8,
                inverted: false,
            }),
            Self::StackAddress(_)
            | Self::SymbolAddress { .. }
            | Self::ExternalTable(_)
            | Self::ExternalFunction { .. }
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

    pub(crate) fn register_read(read_token: u32, address: u32, width: u8, signed: bool) -> Self {
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

    pub(crate) fn indexed_register_read(read_token: u32, width: u8, signed: bool) -> Self {
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

    pub(crate) fn memory_read(read_token: u32, width: u8, signed: bool) -> Self {
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

    pub(crate) fn private_stack_read(read_token: u32, width: u8, signed: bool) -> Self {
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

    pub(crate) fn substitute(
        &self,
        arguments: &Rv32CallArguments,
        read_tokens: &[u32],
        memory_read_tokens: &[u32],
        external_tokens: &[u32],
    ) -> std::result::Result<Self, String> {
        if let Some(index) = self.direct_input_index() {
            return arguments
                .get(usize::from(index))
                .cloned()
                .ok_or_else(|| format!("call argument {index} is outside the RV32 ABI"));
        }
        if let Self::SymbolAddress { lo_addend, .. } = self {
            return lo_addend
                .is_some()
                .then(|| self.clone())
                .ok_or_else(|| "incomplete relocation escaped across a call boundary".to_owned());
        }
        if matches!(self, Self::FunctionTable(_) | Self::FunctionPointer { .. }) {
            return Ok(self.clone());
        }
        if let Self::Expression {
            operation,
            left,
            right,
        } = self
        {
            return Ok(Self::Expression {
                operation: *operation,
                left: Box::new(left.substitute(
                    arguments,
                    read_tokens,
                    memory_read_tokens,
                    external_tokens,
                )?),
                right: Box::new(right.substitute(
                    arguments,
                    read_tokens,
                    memory_read_tokens,
                    external_tokens,
                )?),
            });
        }
        if let Self::WideSignedDivide {
            dividend_low,
            dividend_high,
            divisor_low,
            divisor_high,
            high_word,
        } = self
        {
            return Ok(Self::WideSignedDivide {
                dividend_low: Box::new(dividend_low.substitute(
                    arguments,
                    read_tokens,
                    memory_read_tokens,
                    external_tokens,
                )?),
                dividend_high: Box::new(dividend_high.substitute(
                    arguments,
                    read_tokens,
                    memory_read_tokens,
                    external_tokens,
                )?),
                divisor_low: Box::new(divisor_low.substitute(
                    arguments,
                    read_tokens,
                    memory_read_tokens,
                    external_tokens,
                )?),
                divisor_high: Box::new(divisor_high.substitute(
                    arguments,
                    read_tokens,
                    memory_read_tokens,
                    external_tokens,
                )?),
                high_word: *high_word,
            });
        }
        if let Self::StackAddress(_) = self {
            return Ok(self.clone());
        }
        if matches!(self, Self::ExternalTable(_) | Self::ExternalFunction { .. }) {
            return Err("non-scalar value escaped across a call boundary".to_owned());
        }
        let bits = self.bits();
        let mut substituted = [BitSource::Unknown; 32];
        for (destination, source) in bits.into_iter().enumerate() {
            substituted[destination] = match source {
                BitSource::Unknown => BitSource::Unknown,
                BitSource::Constant(value) => BitSource::Constant(value),
                BitSource::Input {
                    index,
                    bit,
                    inverted,
                } => {
                    let argument = arguments
                        .get(usize::from(index))
                        .ok_or_else(|| format!("call argument {index} is outside the RV32 ABI"))?;
                    let source = argument.bits()[usize::from(bit)];
                    if inverted { source.inverted() } else { source }
                }
                BitSource::Register {
                    read_token,
                    address,
                    bit,
                    inverted,
                } => BitSource::Register {
                    read_token: *read_tokens.get(read_token as usize).ok_or_else(|| {
                        format!("callee MMIO read token {read_token} has no caller mapping")
                    })?,
                    address,
                    bit,
                    inverted,
                },
                BitSource::IndexedRegister {
                    read_token,
                    bit,
                    inverted,
                } => BitSource::IndexedRegister {
                    read_token: *read_tokens.get(read_token as usize).ok_or_else(|| {
                        format!("callee MMIO read token {read_token} has no caller mapping")
                    })?,
                    bit,
                    inverted,
                },
                BitSource::Memory {
                    read_token,
                    bit,
                    inverted,
                } => {
                    let read_token =
                        *memory_read_tokens.get(read_token as usize).ok_or_else(|| {
                            format!("callee memory read token {read_token} has no caller mapping")
                        })?;
                    if read_token & PRIVATE_STACK_READ_TOKEN_FLAG != 0 {
                        BitSource::PrivateStack {
                            read_token: read_token & !PRIVATE_STACK_READ_TOKEN_FLAG,
                            bit,
                            inverted,
                        }
                    } else {
                        BitSource::Memory {
                            read_token,
                            bit,
                            inverted,
                        }
                    }
                }
                BitSource::PrivateStack { .. } => {
                    return Err(
                        "callee private-stack read escaped across a call boundary".to_owned()
                    );
                }
                BitSource::CallResult {
                    call_token,
                    bit,
                    inverted,
                } => BitSource::CallResult {
                    call_token,
                    bit,
                    inverted,
                },
                BitSource::ExternalResult {
                    call_token,
                    bit,
                    inverted,
                } => BitSource::ExternalResult {
                    call_token: *external_tokens.get(call_token as usize).ok_or_else(|| {
                        format!("callee external-call token {call_token} has no caller mapping")
                    })?,
                    bit,
                    inverted,
                },
            };
        }
        Ok(Self::from_bits(substituted))
    }

    pub(crate) fn rewrite_call_context(
        &self,
        read_tokens: &[u32],
        memory_read_tokens: &[u32],
        external_tokens: &[u32],
        call_results: &BTreeMap<u32, SymbolicValue>,
        private_stack_reads: &BTreeMap<u32, SymbolicValue>,
    ) -> std::result::Result<Self, String> {
        if let Self::SymbolAddress { lo_addend, .. } = self {
            return lo_addend
                .is_some()
                .then(|| self.clone())
                .ok_or_else(|| "incomplete relocation escaped across a call boundary".to_owned());
        }
        if matches!(self, Self::FunctionTable(_) | Self::FunctionPointer { .. }) {
            return Ok(self.clone());
        }
        if let Self::Expression {
            operation,
            left,
            right,
        } = self
        {
            return Ok(Self::Expression {
                operation: *operation,
                left: Box::new(left.rewrite_call_context(
                    read_tokens,
                    memory_read_tokens,
                    external_tokens,
                    call_results,
                    private_stack_reads,
                )?),
                right: Box::new(right.rewrite_call_context(
                    read_tokens,
                    memory_read_tokens,
                    external_tokens,
                    call_results,
                    private_stack_reads,
                )?),
            });
        }
        if let Self::WideSignedDivide {
            dividend_low,
            dividend_high,
            divisor_low,
            divisor_high,
            high_word,
        } = self
        {
            return Ok(Self::WideSignedDivide {
                dividend_low: Box::new(dividend_low.rewrite_call_context(
                    read_tokens,
                    memory_read_tokens,
                    external_tokens,
                    call_results,
                    private_stack_reads,
                )?),
                dividend_high: Box::new(dividend_high.rewrite_call_context(
                    read_tokens,
                    memory_read_tokens,
                    external_tokens,
                    call_results,
                    private_stack_reads,
                )?),
                divisor_low: Box::new(divisor_low.rewrite_call_context(
                    read_tokens,
                    memory_read_tokens,
                    external_tokens,
                    call_results,
                    private_stack_reads,
                )?),
                divisor_high: Box::new(divisor_high.rewrite_call_context(
                    read_tokens,
                    memory_read_tokens,
                    external_tokens,
                    call_results,
                    private_stack_reads,
                )?),
                high_word: *high_word,
            });
        }
        if let Self::StackAddress(_) = self {
            return Ok(self.clone());
        }
        if matches!(self, Self::ExternalTable(_) | Self::ExternalFunction { .. }) {
            return Err("non-scalar value escaped across a call boundary".to_owned());
        }
        let bits = self.bits();
        let mut rewritten = [BitSource::Unknown; 32];
        for (destination, source) in bits.into_iter().enumerate() {
            rewritten[destination] = match source {
                BitSource::Unknown => BitSource::Unknown,
                BitSource::Constant(value) => BitSource::Constant(value),
                BitSource::Input {
                    index,
                    bit,
                    inverted,
                } => BitSource::Input {
                    index,
                    bit,
                    inverted,
                },
                BitSource::Register {
                    read_token,
                    address,
                    bit,
                    inverted,
                } => BitSource::Register {
                    read_token: *read_tokens.get(read_token as usize).ok_or_else(|| {
                        format!("caller MMIO read token {read_token} has no flattened mapping")
                    })?,
                    address,
                    bit,
                    inverted,
                },
                BitSource::IndexedRegister {
                    read_token,
                    bit,
                    inverted,
                } => BitSource::IndexedRegister {
                    read_token: *read_tokens.get(read_token as usize).ok_or_else(|| {
                        format!("caller MMIO read token {read_token} has no flattened mapping")
                    })?,
                    bit,
                    inverted,
                },
                BitSource::Memory {
                    read_token,
                    bit,
                    inverted,
                } => BitSource::Memory {
                    read_token: *memory_read_tokens.get(read_token as usize).ok_or_else(|| {
                        format!("caller memory read token {read_token} has no flattened mapping")
                    })?,
                    bit,
                    inverted,
                },
                BitSource::PrivateStack {
                    read_token,
                    bit,
                    inverted,
                } => {
                    let value = private_stack_reads.get(&read_token).ok_or_else(|| {
                        format!("private-stack read {read_token} is not available")
                    })?;
                    let source = value.bits()[usize::from(bit)];
                    if inverted { source.inverted() } else { source }
                }
                BitSource::CallResult {
                    call_token,
                    bit,
                    inverted,
                } => {
                    let result = call_results
                        .get(&call_token)
                        .ok_or_else(|| format!("call result {call_token} is not available"))?;
                    let source = result.bits()[usize::from(bit)];
                    if inverted { source.inverted() } else { source }
                }
                BitSource::ExternalResult {
                    call_token,
                    bit,
                    inverted,
                } => BitSource::ExternalResult {
                    call_token: *external_tokens.get(call_token as usize).ok_or_else(|| {
                        format!("caller external-call token {call_token} has no flattened mapping")
                    })?,
                    bit,
                    inverted,
                },
            };
        }
        Ok(Self::from_bits(rewritten))
    }

    pub(crate) fn rewrite_private_stack_context(
        &self,
        private_stack_reads: &BTreeMap<u32, SymbolicValue>,
    ) -> std::result::Result<Self, String> {
        if let Self::Expression {
            operation,
            left,
            right,
        } = self
        {
            return Ok(Self::Expression {
                operation: *operation,
                left: Box::new(left.rewrite_private_stack_context(private_stack_reads)?),
                right: Box::new(right.rewrite_private_stack_context(private_stack_reads)?),
            });
        }
        if let Self::WideSignedDivide {
            dividend_low,
            dividend_high,
            divisor_low,
            divisor_high,
            high_word,
        } = self
        {
            return Ok(Self::WideSignedDivide {
                dividend_low: Box::new(
                    dividend_low.rewrite_private_stack_context(private_stack_reads)?,
                ),
                dividend_high: Box::new(
                    dividend_high.rewrite_private_stack_context(private_stack_reads)?,
                ),
                divisor_low: Box::new(
                    divisor_low.rewrite_private_stack_context(private_stack_reads)?,
                ),
                divisor_high: Box::new(
                    divisor_high.rewrite_private_stack_context(private_stack_reads)?,
                ),
                high_word: *high_word,
            });
        }
        if matches!(
            self,
            Self::SymbolAddress { .. }
                | Self::StackAddress(_)
                | Self::ExternalTable(_)
                | Self::ExternalFunction { .. }
                | Self::FunctionTable(_)
                | Self::FunctionPointer { .. }
        ) {
            return Ok(self.clone());
        }
        let mut rewritten = [BitSource::Unknown; 32];
        for (destination, source) in self.bits().into_iter().enumerate() {
            rewritten[destination] = match source {
                BitSource::PrivateStack {
                    read_token,
                    bit,
                    inverted,
                } => {
                    let value = private_stack_reads.get(&read_token).ok_or_else(|| {
                        format!("private-stack read {read_token} is not available")
                    })?;
                    let source = value.bits()[usize::from(bit)];
                    if inverted { source.inverted() } else { source }
                }
                other => other,
            };
        }
        Ok(Self::from_bits(rewritten))
    }

    pub(crate) fn from_bits(bits: [BitSource; 32]) -> Self {
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
        Self::Bits(Box::new(bits))
    }

    pub(crate) fn and(self, constant: u32) -> Self {
        if matches!(&self, Self::Expression { .. }) {
            return Self::expression(ExpressionOperation::BitAnd, self, Self::Constant(constant));
        }
        Self::from_bits(core::array::from_fn(|bit| {
            if constant & (1 << bit) == 0 {
                BitSource::Constant(false)
            } else {
                self.bits()[bit]
            }
        }))
    }

    pub(crate) fn or(self, constant: u32) -> Self {
        if matches!(&self, Self::Expression { .. }) {
            return Self::expression(ExpressionOperation::BitOr, self, Self::Constant(constant));
        }
        Self::from_bits(core::array::from_fn(|bit| {
            if constant & (1 << bit) != 0 {
                BitSource::Constant(true)
            } else {
                self.bits()[bit]
            }
        }))
    }

    pub(crate) fn bitand(self, other: Self) -> Self {
        if let Some(constant) = self.as_constant() {
            return other.and(constant);
        }
        if let Some(constant) = other.as_constant() {
            return self.and(constant);
        }
        if matches!(&self, Self::Expression { .. }) || matches!(&other, Self::Expression { .. }) {
            return Self::expression(ExpressionOperation::BitAnd, self, other);
        }
        let left = self.bits();
        let right = other.bits();
        let simplified =
            Self::from_bits(core::array::from_fn(|bit| match (left[bit], right[bit]) {
                (BitSource::Constant(false), _) | (_, BitSource::Constant(false)) => {
                    BitSource::Constant(false)
                }
                (BitSource::Constant(true), source) | (source, BitSource::Constant(true)) => source,
                (left, right) if left == right => left,
                _ => BitSource::Unknown,
            }));
        if simplified.is_resolved() {
            simplified
        } else {
            Self::expression(ExpressionOperation::BitAnd, self, other)
        }
    }

    pub(crate) fn bitor(self, other: Self) -> Self {
        if let Some(constant) = self.as_constant() {
            return other.or(constant);
        }
        if let Some(constant) = other.as_constant() {
            return self.or(constant);
        }
        if matches!(&self, Self::Expression { .. }) || matches!(&other, Self::Expression { .. }) {
            return Self::expression(ExpressionOperation::BitOr, self, other);
        }
        let left = self.bits();
        let right = other.bits();
        let simplified =
            Self::from_bits(core::array::from_fn(|bit| match (left[bit], right[bit]) {
                (BitSource::Constant(true), _) | (_, BitSource::Constant(true)) => {
                    BitSource::Constant(true)
                }
                (BitSource::Constant(false), source) | (source, BitSource::Constant(false)) => {
                    source
                }
                (left, right) if left == right => left,
                _ => BitSource::Unknown,
            }));
        if simplified.is_resolved() {
            simplified
        } else {
            Self::expression(ExpressionOperation::BitOr, self, other)
        }
    }

    pub(crate) fn shift_left(self, amount: u32) -> Self {
        if matches!(&self, Self::Expression { .. }) {
            return Self::expression(ExpressionOperation::ShiftLeft, self, Self::Constant(amount));
        }
        let source = self.bits();
        Self::from_bits(core::array::from_fn(|bit| {
            bit.checked_sub(amount as usize)
                .map_or(BitSource::Constant(false), |source_bit| source[source_bit])
        }))
    }

    pub(crate) fn shift_right(self, amount: u32) -> Self {
        if matches!(&self, Self::Expression { .. }) {
            return Self::expression(
                ExpressionOperation::ShiftRight,
                self,
                Self::Constant(amount),
            );
        }
        let source = self.bits();
        Self::from_bits(core::array::from_fn(|bit| {
            source
                .get(bit + amount as usize)
                .copied()
                .unwrap_or(BitSource::Constant(false))
        }))
    }

    pub(crate) fn add_constant(self, constant: u32) -> Self {
        if constant == 0 {
            return self;
        }
        let field_sum = |and_mask: u32, or_mask: u32| {
            or_mask
                .checked_add(constant)
                .filter(|sum| (sum ^ or_mask) & and_mask == 0)
        };
        match &self {
            Self::RegisterImage {
                read_token,
                address,
                and_mask,
                or_mask,
            } if field_sum(*and_mask, *or_mask).is_some() => {
                return Self::RegisterImage {
                    read_token: *read_token,
                    address: *address,
                    and_mask: *and_mask,
                    or_mask: field_sum(*and_mask, *or_mask).unwrap(),
                };
            }
            Self::IndexedRegisterImage {
                read_token,
                and_mask,
                or_mask,
            } if field_sum(*and_mask, *or_mask).is_some() => {
                return Self::IndexedRegisterImage {
                    read_token: *read_token,
                    and_mask: *and_mask,
                    or_mask: field_sum(*and_mask, *or_mask).unwrap(),
                };
            }
            Self::MemoryImage {
                read_token,
                and_mask,
                or_mask,
            } if field_sum(*and_mask, *or_mask).is_some() => {
                return Self::MemoryImage {
                    read_token: *read_token,
                    and_mask: *and_mask,
                    or_mask: field_sum(*and_mask, *or_mask).unwrap(),
                };
            }
            _ => {}
        }
        // Compilers freely select ADDI instead of ORI after clearing the
        // destination field. If every set bit in the addend is proven zero in
        // the symbolic value, addition cannot carry and is exactly the same
        // field insertion as bitwise OR. Canonicalize both instruction
        // selections to the existing mask/or representation.
        let bits = self.bits();
        if bits.iter().enumerate().all(|(bit, source)| {
            constant & (1_u32 << bit) == 0 || *source == BitSource::Constant(false)
        }) {
            return self.or(constant);
        }
        if let Self::Constant(value) = self {
            return Self::Constant(value.wrapping_add(constant));
        }
        if let Self::StackAddress(offset) = self {
            return Self::StackAddress(offset.wrapping_add(constant as i32));
        }
        if let Self::SymbolAddress {
            member,
            symbol,
            hi_addend,
            lo_addend,
            post_offset,
        } = self
        {
            return Self::SymbolAddress {
                member,
                symbol,
                hi_addend,
                lo_addend,
                post_offset: post_offset.wrapping_add(i64::from(constant as i32)),
            };
        }
        Self::expression(ExpressionOperation::Add, self, Self::Constant(constant))
    }

    pub(crate) fn direct_input_index(&self) -> Option<u8> {
        if let Self::InputConstant { index, .. } = self {
            return Some(*index);
        }
        let Self::Bits(bits) = self else {
            return None;
        };
        let mut index = None;
        for (destination, source) in bits.iter().enumerate() {
            let BitSource::Input {
                index: source_index,
                bit,
                inverted: false,
            } = source
            else {
                return None;
            };
            if usize::from(*bit) != destination {
                return None;
            }
            match index {
                Some(index) if index != *source_index => return None,
                Some(_) => {}
                None => index = Some(*source_index),
            }
        }
        index
    }

    pub(crate) fn caller_memory_address(&self) -> bool {
        if self.direct_input_index().is_some() {
            return true;
        }
        match self {
            Self::Expression {
                operation: ExpressionOperation::Add,
                left,
                right,
            } => {
                (left.caller_memory_address() && matches!(right.as_ref(), Self::Constant(_)))
                    || (right.caller_memory_address() && matches!(left.as_ref(), Self::Constant(_)))
            }
            Self::Expression {
                operation: ExpressionOperation::Subtract,
                left,
                right,
            } => left.caller_memory_address() && matches!(right.as_ref(), Self::Constant(_)),
            _ => false,
        }
    }

    /// Returns the byte offset when this is an affine address into the current
    /// function's private stack frame.
    ///
    /// This remains deliberately narrower than general expression evaluation:
    /// only a stack base plus or minus a constant is accepted. It is used while
    /// composing calls so private scratch memory never becomes a generated host
    /// pointer.
    pub(crate) fn private_stack_offset(&self) -> Option<i32> {
        match self {
            Self::StackAddress(offset) => Some(*offset),
            Self::Expression {
                operation: ExpressionOperation::Add,
                left,
                right,
            } => match (left.private_stack_offset(), right.as_constant()) {
                (Some(offset), Some(constant)) => Some(offset.wrapping_add(constant as i32)),
                _ => match (right.private_stack_offset(), left.as_constant()) {
                    (Some(offset), Some(constant)) => Some(offset.wrapping_add(constant as i32)),
                    _ => None,
                },
            },
            Self::Expression {
                operation: ExpressionOperation::Subtract,
                left,
                right,
            } => match (left.private_stack_offset(), right.as_constant()) {
                (Some(offset), Some(constant)) => Some(offset.wrapping_sub(constant as i32)),
                _ => None,
            },
            _ => None,
        }
    }

    pub(crate) fn depends_on_private_stack_read(&self) -> bool {
        match self {
            Self::Expression { left, right, .. } => {
                left.depends_on_private_stack_read() || right.depends_on_private_stack_read()
            }
            Self::WideSignedDivide {
                dividend_low,
                dividend_high,
                divisor_low,
                divisor_high,
                ..
            } => {
                dividend_low.depends_on_private_stack_read()
                    || dividend_high.depends_on_private_stack_read()
                    || divisor_low.depends_on_private_stack_read()
                    || divisor_high.depends_on_private_stack_read()
            }
            Self::Bits(bits) => bits
                .iter()
                .any(|source| matches!(source, BitSource::PrivateStack { .. })),
            _ => false,
        }
    }

    #[cfg(test)]
    pub(crate) fn not(self) -> Self {
        Self::from_bits(self.bits().map(|source| match source {
            BitSource::Constant(value) => BitSource::Constant(!value),
            BitSource::Input {
                index,
                bit,
                inverted,
            } => BitSource::Input {
                index,
                bit,
                inverted: !inverted,
            },
            BitSource::Register {
                read_token,
                address,
                bit,
                inverted,
            } => BitSource::Register {
                read_token,
                address,
                bit,
                inverted: !inverted,
            },
            BitSource::IndexedRegister {
                read_token,
                bit,
                inverted,
            } => BitSource::IndexedRegister {
                read_token,
                bit,
                inverted: !inverted,
            },
            BitSource::Memory {
                read_token,
                bit,
                inverted,
            } => BitSource::Memory {
                read_token,
                bit,
                inverted: !inverted,
            },
            BitSource::PrivateStack {
                read_token,
                bit,
                inverted,
            } => BitSource::PrivateStack {
                read_token,
                bit,
                inverted: !inverted,
            },
            BitSource::CallResult {
                call_token,
                bit,
                inverted,
            } => BitSource::CallResult {
                call_token,
                bit,
                inverted: !inverted,
            },
            BitSource::ExternalResult {
                call_token,
                bit,
                inverted,
            } => BitSource::ExternalResult {
                call_token,
                bit,
                inverted: !inverted,
            },
            BitSource::Unknown => BitSource::Unknown,
        }))
    }

    pub(crate) fn xor(self, constant: u32) -> Self {
        if matches!(&self, Self::Expression { .. }) {
            return Self::expression(ExpressionOperation::BitXor, self, Self::Constant(constant));
        }
        let bits = self.bits();
        Self::from_bits(core::array::from_fn(|bit| {
            if constant & (1 << bit) == 0 {
                bits[bit]
            } else {
                match bits[bit] {
                    BitSource::Constant(value) => BitSource::Constant(!value),
                    BitSource::Input {
                        index,
                        bit,
                        inverted,
                    } => BitSource::Input {
                        index,
                        bit,
                        inverted: !inverted,
                    },
                    BitSource::Register {
                        read_token,
                        address,
                        bit,
                        inverted,
                    } => BitSource::Register {
                        read_token,
                        address,
                        bit,
                        inverted: !inverted,
                    },
                    BitSource::IndexedRegister {
                        read_token,
                        bit,
                        inverted,
                    } => BitSource::IndexedRegister {
                        read_token,
                        bit,
                        inverted: !inverted,
                    },
                    BitSource::Memory {
                        read_token,
                        bit,
                        inverted,
                    } => BitSource::Memory {
                        read_token,
                        bit,
                        inverted: !inverted,
                    },
                    BitSource::PrivateStack {
                        read_token,
                        bit,
                        inverted,
                    } => BitSource::PrivateStack {
                        read_token,
                        bit,
                        inverted: !inverted,
                    },
                    BitSource::CallResult {
                        call_token,
                        bit,
                        inverted,
                    } => BitSource::CallResult {
                        call_token,
                        bit,
                        inverted: !inverted,
                    },
                    BitSource::ExternalResult {
                        call_token,
                        bit,
                        inverted,
                    } => BitSource::ExternalResult {
                        call_token,
                        bit,
                        inverted: !inverted,
                    },
                    BitSource::Unknown => BitSource::Unknown,
                }
            }
        }))
    }

    pub(crate) fn bitxor(self, other: Self) -> Self {
        if self == other {
            return Self::Constant(0);
        }
        match (self, other) {
            (Self::Constant(constant), value) | (value, Self::Constant(constant)) => {
                value.xor(constant)
            }
            (left, right) => Self::expression(ExpressionOperation::BitXor, left, right),
        }
    }

    pub(crate) fn expression(operation: ExpressionOperation, left: Self, right: Self) -> Self {
        if !left.is_resolved() || !right.is_resolved() {
            return Self::Unknown;
        }
        Self::Expression {
            operation,
            left: Box::new(left),
            right: Box::new(right),
        }
    }

    // This becomes live when the exact, digest-gated __divdi3 call summary is
    // connected; keeping construction centralized prevents low/high drift.
    #[allow(dead_code)]
    pub(crate) fn wide_signed_divide_words(
        dividend_low: Self,
        dividend_high: Self,
        divisor_low: Self,
        divisor_high: Self,
    ) -> (Self, Self) {
        if !dividend_low.is_resolved()
            || !dividend_high.is_resolved()
            || !divisor_low.is_resolved()
            || !divisor_high.is_resolved()
        {
            return (Self::Unknown, Self::Unknown);
        }
        let word = |high_word| Self::WideSignedDivide {
            dividend_low: Box::new(dividend_low.clone()),
            dividend_high: Box::new(dividend_high.clone()),
            divisor_low: Box::new(divisor_low.clone()),
            divisor_high: Box::new(divisor_high.clone()),
            high_word,
        };
        (word(false), word(true))
    }

    pub(crate) fn as_constant(&self) -> Option<u32> {
        match self {
            Self::Constant(value) => Some(*value),
            Self::InputConstant { value, .. } => Some(*value),
            _ => None,
        }
    }

    pub(crate) fn seqz(self) -> Self {
        if let Self::Constant(value) = &self {
            return Self::Constant((*value == 0) as u32);
        }
        let mut nonzero = self
            .bits()
            .into_iter()
            .filter(|source| *source != BitSource::Constant(false));
        let source = nonzero.next();
        if nonzero.next().is_some() {
            return Self::expression(ExpressionOperation::Equal, self, Self::Constant(0));
        }
        let inverse = match source {
            None => BitSource::Constant(true),
            Some(BitSource::Constant(true)) => BitSource::Constant(false),
            Some(BitSource::Input {
                index,
                bit,
                inverted,
            }) => BitSource::Input {
                index,
                bit,
                inverted: !inverted,
            },
            Some(BitSource::Register {
                read_token,
                address,
                bit,
                inverted,
            }) => BitSource::Register {
                read_token,
                address,
                bit,
                inverted: !inverted,
            },
            Some(BitSource::IndexedRegister {
                read_token,
                bit,
                inverted,
            }) => BitSource::IndexedRegister {
                read_token,
                bit,
                inverted: !inverted,
            },
            Some(BitSource::Memory {
                read_token,
                bit,
                inverted,
            }) => BitSource::Memory {
                read_token,
                bit,
                inverted: !inverted,
            },
            Some(BitSource::PrivateStack {
                read_token,
                bit,
                inverted,
            }) => BitSource::PrivateStack {
                read_token,
                bit,
                inverted: !inverted,
            },
            Some(BitSource::CallResult {
                call_token,
                bit,
                inverted,
            }) => BitSource::CallResult {
                call_token,
                bit,
                inverted: !inverted,
            },
            Some(BitSource::ExternalResult {
                call_token,
                bit,
                inverted,
            }) => BitSource::ExternalResult {
                call_token,
                bit,
                inverted: !inverted,
            },
            Some(BitSource::Unknown) => BitSource::Unknown,
            Some(BitSource::Constant(false)) => unreachable!(),
        };
        Self::from_bits(core::array::from_fn(|bit| {
            if bit == 0 {
                inverse
            } else {
                BitSource::Constant(false)
            }
        }))
    }

    pub(crate) fn is_resolved(&self) -> bool {
        match self {
            Self::Expression { left, right, .. } => left.is_resolved() && right.is_resolved(),
            Self::WideSignedDivide {
                dividend_low,
                dividend_high,
                divisor_low,
                divisor_high,
                ..
            } => {
                dividend_low.is_resolved()
                    && dividend_high.is_resolved()
                    && divisor_low.is_resolved()
                    && divisor_high.is_resolved()
            }
            Self::SymbolAddress { lo_addend, .. } => lo_addend.is_some(),
            Self::ExternalResult(_) => true,
            Self::ExternalTable(_)
            | Self::ExternalFunction { .. }
            | Self::FunctionTable(_)
            | Self::FunctionPointer { .. }
            | Self::StackAddress(_) => false,
            _ => !matches!(self, Self::Unknown) && !self.bits().contains(&BitSource::Unknown),
        }
    }

    pub(crate) fn canonical(&self) -> String {
        match self {
            Self::Unknown => "unknown".to_owned(),
            Self::Constant(value) => format!("const:{value:#010x}"),
            Self::InputConstant { index, .. } => Self::input(*index).canonical(),
            Self::StackAddress(offset) => format!("private-stack:{offset:+#x}"),
            Self::SymbolAddress {
                member,
                symbol,
                hi_addend,
                lo_addend,
                post_offset,
            } => format!(
                "symbol:{}::{symbol}:hi{hi_addend:+#x}:lo{}:post{post_offset:+#x}",
                member.as_deref().unwrap_or("<linked>"),
                lo_addend.map_or_else(|| "?".to_owned(), |addend| format!("{addend:+#x}"))
            ),
            Self::CallResult(call_token) => format!("call-result:{call_token}"),
            Self::ExternalTable(table) => {
                format!("external-table:{}", external_abi::table_spec(*table).id)
            }
            Self::ExternalFunction { table, function } => format!(
                "external-function:{}::{function:?}",
                external_abi::table_spec(*table).id
            ),
            Self::FunctionTable(table) => format!("function-table:{}", table.id()),
            Self::FunctionPointer { table, target } => {
                format!("function-pointer:{}::{target:#010x}", table.id())
            }
            Self::ExternalResult(call_token) => format!("external-result:{call_token}"),
            Self::Expression {
                operation,
                left,
                right,
            } => format!(
                "expr:{operation:?}({},{})",
                left.canonical(),
                right.canonical()
            ),
            Self::WideSignedDivide {
                dividend_low,
                dividend_high,
                divisor_low,
                divisor_high,
                high_word,
            } => format!(
                "wide-sdiv64:{}({},{},{},{})",
                if *high_word { "high" } else { "low" },
                dividend_low.canonical(),
                dividend_high.canonical(),
                divisor_low.canonical(),
                divisor_high.canonical(),
            ),
            Self::RegisterImage {
                read_token,
                address,
                and_mask,
                or_mask,
            } => format!("rmw:read{read_token}[{address:#010x}]&{and_mask:#010x}|{or_mask:#010x}"),
            Self::IndexedRegisterImage {
                read_token,
                and_mask,
                or_mask,
            } => format!("indexed-rmw:read{read_token}&{and_mask:#010x}|{or_mask:#010x}"),
            Self::MemoryImage {
                read_token,
                and_mask,
                or_mask,
            } => format!("ram:read{read_token}&{and_mask:#010x}|{or_mask:#010x}"),
            Self::Bits(bits) => {
                let terms = bits
                    .iter()
                    .enumerate()
                    .filter_map(|(bit, source)| match source {
                        BitSource::Constant(false) => None,
                        BitSource::Constant(true) => Some(format!("{bit}=1")),
                        BitSource::Input {
                            index,
                            bit: source,
                            inverted,
                        } => {
                            let inverse = if *inverted { "!" } else { "" };
                            Some(format!("{bit}={inverse}arg{index}.{source}"))
                        }
                        BitSource::Register {
                            read_token,
                            address,
                            bit: source,
                            inverted,
                        } => {
                            let inverse = if *inverted { "!" } else { "" };
                            Some(format!(
                                "{bit}={inverse}read{read_token}[{address:#010x}].{source}"
                            ))
                        }
                        BitSource::IndexedRegister {
                            read_token,
                            bit: source,
                            inverted,
                        } => {
                            let inverse = if *inverted { "!" } else { "" };
                            Some(format!("{bit}={inverse}indexed-read{read_token}.{source}"))
                        }
                        BitSource::Memory {
                            read_token,
                            bit: source,
                            inverted,
                        } => {
                            let inverse = if *inverted { "!" } else { "" };
                            Some(format!("{bit}={inverse}ramread{read_token}.{source}"))
                        }
                        BitSource::PrivateStack {
                            read_token,
                            bit: source,
                            inverted,
                        } => {
                            let inverse = if *inverted { "!" } else { "" };
                            Some(format!(
                                "{bit}={inverse}private-stack-read{read_token}.{source}"
                            ))
                        }
                        BitSource::CallResult {
                            call_token,
                            bit: source,
                            inverted,
                        } => {
                            let inverse = if *inverted { "!" } else { "" };
                            Some(format!("{bit}={inverse}call{call_token}.a0.{source}"))
                        }
                        BitSource::ExternalResult {
                            call_token,
                            bit: source,
                            inverted,
                        } => {
                            let inverse = if *inverted { "!" } else { "" };
                            Some(format!("{bit}={inverse}external{call_token}.{source}"))
                        }
                        BitSource::Unknown => Some(format!("{bit}=?")),
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                format!("bits:{terms}")
            }
        }
    }
}
