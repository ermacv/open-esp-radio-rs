//! Symbolic 32-bit values and proven indexed-MMIO domains.

use std::{collections::BTreeMap, sync::Arc};

use open_radio_vendor_contracts::FunctionTableRef;

pub const PRIVATE_STACK_READ_TOKEN_FLAG: u32 = 1 << 31;
/// Marks an [`SymbolicValue::ExternalResult`] as the base of a reviewed fresh
/// zeroed allocation while preserving the call token in the remaining bits.
pub const ALLOCATED_EXTERNAL_RESULT_TOKEN_FLAG: u32 = 1 << 30;
/// Marks an external result as a reviewed pointer to an opaque runtime-owned
/// object while retaining the call identity in the remaining bits.
pub const OPAQUE_POINTER_EXTERNAL_RESULT_TOKEN_FLAG: u32 = 1 << 29;
/// Marks an [`SymbolicValue::ExternalResult`] as the base of a reviewed fresh
/// uninitialized allocation while preserving the call token.
pub const UNINITIALIZED_ALLOCATION_EXTERNAL_RESULT_TOKEN_FLAG: u32 = 1 << 28;

/// Return the real call identity carried by an external-result token.
///
/// Allocation provenance uses one otherwise-unused bit internally. Consumers
/// that render or index calls must never expose that implementation detail.
pub const fn external_result_call_token(token: u32) -> u32 {
    token
        & !(ALLOCATED_EXTERNAL_RESULT_TOKEN_FLAG
            | OPAQUE_POINTER_EXTERNAL_RESULT_TOKEN_FLAG
            | UNINITIALIZED_ALLOCATION_EXTERNAL_RESULT_TOKEN_FLAG)
}

/// Stable root of an affine memory address recovered from machine code.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum MemoryObjectRoot {
    Argument {
        index: u8,
    },
    RelocatedSymbol {
        member: Option<String>,
        symbol: String,
    },
    /// Storage reached through a pointer loaded from another known memory
    /// object. The pointer cell remains explicit so an absolute RAM cell,
    /// context field and relocated global never collapse into one object.
    Dereferenced {
        pointer: Arc<MemoryObjectRoot>,
        pointer_offset: i64,
    },
    /// Statically known RAM address without a symbol identity.
    Absolute {
        address: u32,
    },
    /// A family of objects selected by one ABI argument and a fixed byte
    /// stride. This preserves array provenance without claiming an index
    /// domain or a nominal element type.
    Indexed {
        root: Arc<MemoryObjectRoot>,
        argument: u8,
        stride: i64,
    },
    /// Fresh memory returned by one reviewed allocator call in this function.
    Allocation {
        call_token: u32,
    },
    ZeroedAllocation {
        call_token: u32,
    },
    OpaqueExternalObject {
        call_token: u32,
    },
}

/// A byte offset within a recovered memory object.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct MemoryObjectLocation {
    pub root: MemoryObjectRoot,
    pub offset: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SymbolicValue {
    Unknown,
    Constant(u32),
    /// Unmodified ABI input. Keeping this canonical avoids repeatedly
    /// rediscovering the same identity from 32 individual bit sources.
    Input {
        index: u8,
    },
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
    ReviewedExternalTable(String),
    /// Function pointer loaded from a reviewed table slot. Executable
    /// behavior, when present, is carried by the reviewed call candidate.
    ReviewedExternalFunction {
        contract: String,
        offset: u32,
    },
    FunctionTable(FunctionTableRef),
    FunctionPointer {
        table: FunctionTableRef,
        target: u32,
    },
    ExternalResult(u32),
    ExternalResultHigh(u32),
    ExternalOutput {
        call_token: u32,
        output_index: u8,
    },
    Expression {
        operation: ExpressionOperation,
        left: Arc<SymbolicValue>,
        right: Arc<SymbolicValue>,
        /// Cached affine caller-memory identity. Expression nodes are
        /// immutable, so deriving this once at construction is equivalent to
        /// recursively rediscovering it at every memory access and call
        /// projection.
        caller_memory_location: Option<(u8, i32)>,
    },
    /// Bit-level result of one reviewed IEEE-754 instruction. Operands are
    /// stored as raw 32-bit register images; no host floating-point assumption
    /// is folded into the observation.
    FloatingPoint {
        operation: FloatingPointOperation,
        rounding: FloatingRoundingMode,
        operands: Box<[SymbolicValue]>,
    },
    WideSignedDivide {
        dividend_low: Arc<SymbolicValue>,
        dividend_high: Arc<SymbolicValue>,
        divisor_low: Arc<SymbolicValue>,
        divisor_high: Arc<SymbolicValue>,
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

/// Depth-first view of one symbolic evidence tree, including its root.
///
/// Keeping child discovery here prevents provenance consumers from silently
/// overlooking a newly introduced compound value variant. Leaf-specific data
/// such as [`BitSource`] records remains the caller's responsibility.
pub struct SymbolicValueTree<'a> {
    pending: Vec<&'a SymbolicValue>,
}

impl<'a> Iterator for SymbolicValueTree<'a> {
    type Item = &'a SymbolicValue;

    fn next(&mut self) -> Option<Self::Item> {
        let value = self.pending.pop()?;
        match value {
            SymbolicValue::Expression { left, right, .. } => {
                self.pending.push(right);
                self.pending.push(left);
            }
            SymbolicValue::FloatingPoint { operands, .. } => {
                self.pending.extend(operands.iter().rev());
            }
            SymbolicValue::WideSignedDivide {
                dividend_low,
                dividend_high,
                divisor_low,
                divisor_high,
                ..
            } => {
                self.pending.push(divisor_high);
                self.pending.push(divisor_low);
                self.pending.push(dividend_high);
                self.pending.push(dividend_low);
            }
            _ => {}
        }
        Some(value)
    }
}

impl SymbolicValue {
    /// Visit this value and every nested symbolic operand in stable source
    /// order. Consumers should use this instead of duplicating recursive
    /// matches over compound value variants.
    pub fn tree(&self) -> SymbolicValueTree<'_> {
        SymbolicValueTree {
            pending: vec![self],
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExpressionOperation {
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
    CountLeadingZeros,
    CountTrailingZeros,
    PopulationCount,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FloatingPointOperation {
    SignedWordToSingle,
    SubtractSingle,
    DivideSingle,
    FusedMultiplyAddSingle,
    SingleToSignedWord,
}

impl FloatingPointOperation {
    pub const fn operand_count(self) -> usize {
        match self {
            Self::SignedWordToSingle | Self::SingleToSignedWord => 1,
            Self::SubtractSingle | Self::DivideSingle => 2,
            Self::FusedMultiplyAddSingle => 3,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FloatingRoundingMode {
    NearestEven,
    TowardZero,
    Down,
    Up,
    NearestMaxMagnitude,
    Dynamic,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BitSource {
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
    ExternalResultHigh {
        call_token: u32,
        bit: u8,
        inverted: bool,
    },
    ExternalOutput {
        call_token: u32,
        output_index: u8,
        bit: u8,
        inverted: bool,
    },
}

impl BitSource {
    pub fn inverted(self) -> Self {
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
            Self::ExternalResultHigh {
                call_token,
                bit,
                inverted,
            } => Self::ExternalResultHigh {
                call_token,
                bit,
                inverted: !inverted,
            },
            Self::ExternalOutput {
                call_token,
                output_index,
                bit,
                inverted,
            } => Self::ExternalOutput {
                call_token,
                output_index,
                bit,
                inverted: !inverted,
            },
        }
    }
}

mod constructors;
mod inspect;
mod operations;
mod rewrite;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbolic_tree_visits_floating_operands_and_nested_expressions() {
        let value = SymbolicValue::FloatingPoint {
            operation: FloatingPointOperation::SubtractSingle,
            rounding: FloatingRoundingMode::Dynamic,
            operands: vec![
                SymbolicValue::Input { index: 2 },
                SymbolicValue::expression(
                    ExpressionOperation::Add,
                    SymbolicValue::CallResult(7),
                    SymbolicValue::MemoryImage {
                        read_token: 11,
                        and_mask: u32::MAX,
                        or_mask: 0,
                    },
                ),
            ]
            .into_boxed_slice(),
        };

        let visited = value.tree().collect::<Vec<_>>();
        assert_eq!(visited.len(), 5);
        assert!(matches!(visited[1], SymbolicValue::Input { index: 2 }));
        assert!(matches!(visited[3], SymbolicValue::CallResult(7)));
        assert!(matches!(
            visited[4],
            SymbolicValue::MemoryImage { read_token: 11, .. }
        ));
    }
}
