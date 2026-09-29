//! Saved register observations are separate from physical hardware declarations.
use crate::*;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterQuery {
    pub scope: NavigationScope,
    /// Explicit candidate intervals; empty selects all numeric memory addresses.
    /// These are a query filter, never an accepted MMIO classification.
    pub ranges: Vec<ImageRegion>,
}
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
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum RegisterRecord {
    Scope {
        record: Box<NavigationRecord>,
    },
    Observation {
        function: NavigationFunction,
        record: u64,
        fact: Box<FunctionRecord>,
        /// None retains an unresolved or nonnumeric address; never silently absent.
        address: Option<u32>,
        alternative: Option<u8>,
        mask: Option<RegisterMask>,
    },
    /// Aggregated candidates. Access widths are instruction widths in bytes;
    /// no physical register width is synthesized from them.
    Address {
        address: u32,
        access_widths: Vec<u8>,
        local_accesses: u64,
        mask_observations: u64,
    },
}
/// A function one captured library input defines.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibraryFunction {
    /// Position of the defining input among the analyzed inputs.
    pub input: u64,
    pub symbol: SymbolId,
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterAccessSummary {
    pub schema: u32,
    /// Explicit candidate intervals; empty selects every numeric address.
    pub ranges: Vec<ImageRegion>,
    pub functions: u64,
    /// Analyzed functions whose coverage or value semantics are incomplete.
    pub partial_functions: u64,
    pub blocked_functions: u64,
    pub gaps: u64,
    pub observations: u64,
    pub unresolved_addresses: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterSummary {
    pub schema: u32,
    pub request: RegisterQuery,
    pub selected_analyses: u64,
    pub partial_analyses: u64,
    pub unavailable_entries: u64,
    pub observations: u64,
    pub unresolved_addresses: u64,
    pub alternative_observations: u64,
    pub candidate_addresses: u64,
}
