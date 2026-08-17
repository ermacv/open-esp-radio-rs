use std::path::PathBuf;

use serde::Serialize;

#[derive(Clone, Debug)]
pub(crate) struct RustArtifactInput {
    pub(crate) suite: String,
    pub(crate) path: PathBuf,
}

#[derive(Debug, Serialize)]
pub(crate) struct RustComponentIndex {
    pub(crate) schema_version: u32,
    pub(crate) summary: RustComponentIndexSummary,
    pub(crate) artifacts: Vec<RustComponentArtifact>,
    pub(crate) components: Vec<RustComponentEvidence>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) diagnostics: Vec<String>,
}

#[derive(Debug, Default, Serialize)]
pub(crate) struct RustComponentIndexSummary {
    pub(crate) reviewed_components: usize,
    pub(crate) source_resolved: usize,
    pub(crate) source_ambiguous: usize,
    pub(crate) source_missing: usize,
    pub(crate) compiled_resolved: usize,
    pub(crate) compiled_missing: usize,
    pub(crate) dwarf_locations: usize,
    pub(crate) freshness_checked: usize,
    pub(crate) freshness_fresh: usize,
    pub(crate) freshness_stale: usize,
    pub(crate) freshness_unknown: usize,
    pub(crate) artifact_freshness_fresh: usize,
    pub(crate) artifact_freshness_stale: usize,
    pub(crate) artifact_freshness_unknown: usize,
}

#[derive(Debug, Serialize)]
pub(crate) struct RustComponentArtifact {
    pub(crate) path: String,
    pub(crate) sha256: String,
    pub(crate) suites: Vec<String>,
    pub(crate) rust_symbols: usize,
    pub(crate) dwarf_locations: usize,
    pub(crate) checked_source_files: usize,
    pub(crate) freshness_status: &'static str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) stale_source_files: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct RustComponentEvidence {
    pub(crate) component_id: String,
    pub(crate) source_status: &'static str,
    pub(crate) compiled_status: &'static str,
    pub(crate) freshness_status: &'static str,
    pub(crate) source_items: Vec<RustSourceItem>,
    pub(crate) compiled_symbols: Vec<RustCompiledSymbol>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub(crate) struct RustSourceItem {
    pub(crate) path: String,
    pub(crate) line: usize,
    pub(crate) kind: &'static str,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub(crate) struct RustCompiledSymbol {
    pub(crate) artifact: String,
    pub(crate) demangled: String,
    pub(crate) address: String,
    pub(crate) size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) source_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) source_line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) source_column: Option<u32>,
}
