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
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RegisterMatchKind {
    ContainedAccess,
    CrossingAccess,
    Region,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum RegisterRecord {
    Scope {
        record: Box<NavigationRecord>,
    },
    /// Includes exact applicability, state and evidence. Rejected declarations
    /// remain visible; they never classify an observation as accepted.
    Declaration {
        entry: Box<KnowledgeEntry>,
    },
    Conflict {
        left: AssertionId,
        right: AssertionId,
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
    Binding {
        function: NavigationFunction,
        record: u64,
        address: u32,
        assertion: AssertionId,
        state: AssertionState,
        relation: RegisterMatchKind,
    },
    /// Aggregated candidates. Access widths are instruction widths in bytes;
    /// no physical register width is synthesized from them.
    Address {
        address: u32,
        access_widths: Vec<u8>,
        local_accesses: u64,
        composed_effects: u64,
        mask_observations: u64,
    },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterSummary {
    pub schema: u32,
    pub request: RegisterQuery,
    pub selected_analyses: u64,
    pub partial_analyses: u64,
    pub unavailable_entries: u64,
    pub declarations: u64,
    pub conflicts: u64,
    pub observations: u64,
    pub unresolved_addresses: u64,
    pub alternative_observations: u64,
    pub candidate_addresses: u64,
    pub matched_accepted: u64,
}
