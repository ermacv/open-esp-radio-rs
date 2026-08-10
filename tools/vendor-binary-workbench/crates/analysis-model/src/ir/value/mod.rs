//! Symbolic 32-bit values and proven indexed-MMIO domains.

use std::{collections::BTreeMap, sync::Arc};

use open_radio_vendor_contracts::{ExternalFunctionRef, ExternalTableRef, FunctionTableRef};

pub const PRIVATE_STACK_READ_TOKEN_FLAG: u32 = 1 << 31;

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
    ExternalTable(ExternalTableRef),
    ExternalFunction {
        table: ExternalTableRef,
        function: ExternalFunctionRef,
    },
    /// Function pointer loaded from a reviewed table slot without an
    /// executable call model.
    ReviewedExternalFunction {
        table: ExternalTableRef,
        offset: u32,
    },
    FunctionTable(FunctionTableRef),
    FunctionPointer {
        table: FunctionTableRef,
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
        }
    }
}

mod constructors;
mod inspect;
mod operations;
mod rewrite;
