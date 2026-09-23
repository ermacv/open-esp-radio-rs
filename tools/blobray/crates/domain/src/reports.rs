//! Read-only observations. Neither report strengthens a publication's claim.
use crate::*;
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileUsage {
    pub files: u64,
    pub logical_bytes: u64,
    pub non_regular_entries: u64,
}
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StorageUsage {
    pub cas: FileUsage,
    pub metadata: FileUsage,
    pub staging: FileUsage,
    pub revisions: u64,
    pub analyses: u64,
    pub publications: u64,
    pub images: u64,
    pub knowledge_revisions: u64,
    pub runs: u64,
    /// Filesystem observations span the walk; they are not an atomic disk snapshot.
    pub atomic_filesystem_snapshot: bool,
    /// No reachability or reclaimable-space calculation is performed.
    pub reachability_assessed: bool,
}
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtentCoverageSummary {
    pub objects: u64,
    pub executable_bytes: u64,
    pub selected_extent_bytes: u64,
    pub outside_selected_extent_bytes: u64,
    pub unknowns: u64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExtentClassification {
    SelectedExtent,
    OutsideSelectedExtents,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ExtentCoverageRecord {
    Section {
        source: FunctionSource,
        object: ObjectId,
        section: u32,
        address: u64,
        size: u64,
        file_backed: bool,
    },
    /// Range uses section-relative offsets, regardless of ET_REL/ET_EXEC.
    Interval {
        source: FunctionSource,
        object: ObjectId,
        section: u32,
        range: CodeRange,
        classification: ExtentClassification,
    },
    Unknown {
        source: Option<FunctionSource>,
        object: Option<ObjectId>,
        reason: String,
    },
}
