//! Physical roots and bounded access paths shared by interfaces and navigation.
use crate::*;
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum AccessRoot {
    /// Physical section location, including captured data with no symbol.
    Section { section: u32, offset: u64 },
    /// Physical symbol address; dereferencing stored pointer bytes is a path step.
    Symbol { symbol: SymbolId, addend: i64 },
    /// Physical incoming ABI word of this exact function: 0..7=a0..a7, then stack words.
    /// This is not a logical signature argument ordinal.
    EntryWord {
        function: FunctionSelector,
        word: u8,
    },
    /// Literal RV32 address scoped by the occurrence; not a file/section offset or host pointer.
    Address { address: u32 },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum AccessStep {
    Offset { bytes: i32 },
    LoadPointer { offset: i32 },
    Index { word: u8, stride: u32 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AccessIssue {
    MissingCallInputs,
    UnknownValue,
    UnsupportedExpression,
    UnmodeledCallResult,
    AbiRequired,
    UnsupportedArgument,
    NonPointerLoad,
    PathLimit,
    OffsetOutOfRange,
    NonzeroCallDisplacement,
    NoPointerPath,
    ForeignOccurrence,
    UnresolvedPointer,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccessPath {
    pub root: AccessRoot,
    pub path: Vec<AccessStep>,
    pub offset: i32,
}
