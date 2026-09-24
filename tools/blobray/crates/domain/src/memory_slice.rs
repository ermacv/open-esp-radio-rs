//! Local write definitions reaching a selected saved publication anchor.
use crate::*;
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemorySliceQuery {
    pub analysis: FunctionAnalysisId,
    pub anchor: u64,
    pub abi: Option<CallAbi>,
    /// Empty discovers known local write spans; a selected read can expose incoming state.
    pub locations: Vec<MemorySliceSelection>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum MemorySliceSelection {
    Access { record: u64 },
    Address { address: u32, width: u8 },
    Stack { offset: i64, width: u8 },
    Argument { word: u8, offset: i32, width: u8 },
}
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum SliceAddress {
    /// Immutable saved scalar value in this analysis; never a host pointer.
    Value {
        expression: u32,
        offset: i32,
    },
    Stack {
        offset: i64,
    },
    Path {
        path: AccessPath,
    },
    Scoped {
        source: FunctionSource,
        object: ObjectId,
        address: u32,
    },
}
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SliceLocation {
    pub address: SliceAddress,
    pub width: u8,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IncomingState {
    Possible,
    Overwritten,
    Unknown,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DefinitionClass {
    Must,
    Alternative,
    Candidate,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SliceIssue {
    PartialControlFlow,
    UnknownWrite,
    MayAlias,
    PartialOverlap,
    DynamicIdentity,
    CallClobber,
    UnsupportedSemantics,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum MemorySliceRecord {
    Location {
        index: u64,
        location: SliceLocation,
        incoming: IncomingState,
        definitions: u64,
        issues: Vec<SliceIssue>,
    },
    Definition {
        location: u64,
        record: u64,
        offset: u64,
        class: DefinitionClass,
        fact: Box<FunctionRecord>,
        witness: Vec<u64>,
    },
    Barrier {
        location: u64,
        record: Option<u64>,
        offset: u64,
        issue: SliceIssue,
    },
    UnknownLocation {
        record: u64,
        issue: Option<AccessIssue>,
    },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemorySliceSummary {
    pub schema: u32,
    pub request: MemorySliceQuery,
    pub anchor_offset: u64,
    pub locations: u64,
    pub definitions: u64,
    pub incoming_locations: u64,
    pub unknown_incoming_locations: u64,
    pub barriers: u64,
    pub unknown_locations: u64,
    pub coverage: FunctionCoverage,
}
