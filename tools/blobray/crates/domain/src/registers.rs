//! Register-access observations are separate from physical hardware declarations.
use crate::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RegisterMaskKind {
    /// Bits retained by a saved load-and-constant expression.
    ReadSelection,
    /// Bits not preserved in `(load & preserve) | replacement`.
    /// This is an expression observation, not a claim of hardware RMW safety.
    WriteReplacement,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterMask {
    pub kind: RegisterMaskKind,
    pub bits: u32,
}
/// One run of bits of a stored value with a common source.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredBits {
    /// The run's lowest bit in the stored value.
    pub low: u8,
    pub width: u8,
    pub source: StoredBitsSource,
}

/// Where a run of stored bits comes from. These are expression observations
/// of one store, not a claim that the path reaching it runs.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum StoredBitsSource {
    /// Fixed bits; `value` is the run's bits, right-aligned.
    Constant { value: u32 },
    /// Bits `low..` of a register's value at the function's entry: an
    /// argument for `a0`..`a7`.
    EntryRegister { register: u8, low: u8 },
    /// Bits `low..` of a value the function loaded; `address` when it is one
    /// exact address, and `same_word` when that is the stored word itself
    /// (bits a read-modify-write keeps).
    Load {
        address: Option<u32>,
        width: u8,
        low: u8,
        same_word: bool,
    },
    /// No exact source.
    Unknown,
}

/// One compiler jump table a relocatable function dispatches through, as
/// its relocations prove it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JumpTable {
    /// The dispatching `jalr`'s offset.
    pub site: u64,
    /// The case value of entry zero: GCC subtracts the smallest case from
    /// the switch value before indexing. `None` when the index's shape does
    /// not show that subtraction, so no case value is claimed.
    pub first_case: Option<i64>,
    /// The case targets in index order: entry `i` is case `first_case + i`.
    pub entries: Vec<u64>,
}

/// A function one captured library input defines.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibraryFunction {
    /// Position of the defining input among the analyzed inputs.
    pub input: u64,
    pub symbol: SymbolId,
    #[serde(with = "oer_riscv_model::symbol_name::option")]
    pub name: Option<Vec<u8>>,
}

/// One outcome of analyzing every function of captured libraries for the
/// memory addresses they access, in input, object and symbol order.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum RegisterAccess {
    /// One candidate address an analyzed function's record accesses.
    Observation {
        function: LibraryFunction,
        record: u64,
        fact: Box<FunctionRecord>,
        /// None retains an unresolved or nonnumeric address; never silently absent.
        address: Option<u32>,
        alternative: Option<u8>,
        mask: Option<RegisterMask>,
        /// For a store: where each run of the stored bits comes from, low run
        /// first, covering the access width. Absent for loads.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stored: Option<Vec<StoredBits>>,
    },
    /// A function that cannot be analyzed; its accesses are unknown.
    Blocked {
        function: LibraryFunction,
        error: Error,
    },
    /// Code no function is selected or analyzed from.
    Gap {
        input: u64,
        object: Option<ObjectId>,
        reason: String,
    },
}

/// Why analyzed functions are partial. Each count is the number of partial
/// functions with that cause, so a function with several counts under each;
/// every partial function has at least one, `other` when none of the named
/// causes explains it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartialCauses {
    /// Code the decoder could not read.
    pub decoding: u64,
    /// Control flow the graph could not follow: an unexpanded indirect jump
    /// or conflicting instruction boundaries.
    pub control_flow: u64,
    /// Relocations the analysis could not resolve.
    pub references: u64,
    /// Calls whose callee is not analyzed: registers and memory after them
    /// are unknown. Common, and no gap in the function's own code.
    pub opaque_calls: u64,
    /// Values whose alternatives exceeded their bound.
    pub value_limits: u64,
    /// Partial for no named cause.
    pub other: u64,
}

impl PartialCauses {
    /// Add `other`'s counts.
    pub fn add(&mut self, other: PartialCauses) {
        self.decoding += other.decoding;
        self.control_flow += other.control_flow;
        self.references += other.references;
        self.opaque_calls += other.opaque_calls;
        self.value_limits += other.value_limits;
        self.other += other.other;
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterAccessSummary {
    pub schema: u32,
    /// Explicit candidate intervals; empty selects every numeric address.
    pub ranges: Vec<ImageRegion>,
    pub functions: u64,
    /// Analyzed functions whose coverage or value semantics are incomplete.
    pub partial_functions: u64,
    /// `partial_functions` by cause.
    pub partial_causes: PartialCauses,
    pub blocked_functions: u64,
    pub gaps: u64,
    pub observations: u64,
    pub unresolved_addresses: u64,
}
