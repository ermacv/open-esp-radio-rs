//! Public data-only application requests and snapshots.

use std::path::PathBuf;

use serde::Serialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiagnosticSeverity {
    Warning,
    Error,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FunctionReviewState {
    Unreviewed,
    Reviewed,
    Ignored,
}

impl FunctionReviewState {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Unreviewed => "unreviewed",
            Self::Reviewed => "reviewed",
            Self::Ignored => "ignored",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FunctionSelection {
    SymbolPrefixRoot,
    ReachableInternal,
}

impl FunctionSelection {
    pub const fn label(self) -> &'static str {
        match self {
            Self::SymbolPrefixRoot => "symbol-prefix-root",
            Self::ReachableInternal => "reachable-internal",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DiagnosticRecord {
    pub severity: DiagnosticSeverity,
    pub component: String,
    pub message: String,
    pub path: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CodeBoundaryReviewState {
    Unreviewed,
    Accepted,
    Rejected,
}

impl CodeBoundaryReviewState {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Unreviewed => "unreviewed",
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CodeBoundaryControlFlowSummary {
    pub caller: String,
    pub site_offset: u64,
    pub kind: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CodeBoundarySummary {
    pub source: String,
    pub artifact_sha256: String,
    pub member: Option<String>,
    pub object_kind: String,
    pub section: String,
    pub address: u64,
    pub entry_offset: u64,
    pub end_exclusive_offset: u64,
    pub end_limit_offset: u64,
    pub status: CodeBoundaryReviewState,
    pub name: Option<String>,
    pub reason: Option<String>,
    pub symbol_names: Vec<String>,
    pub direct_control_flow: Vec<CodeBoundaryControlFlowSummary>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CodeWorkspaceReport {
    pub configured: bool,
    pub facts: Option<PathBuf>,
    pub pack: Option<PathBuf>,
    pub review_output: Option<PathBuf>,
    pub observed_candidates: usize,
    pub accepted: usize,
    pub rejected: usize,
    pub unreviewed: usize,
    pub boundaries: Vec<CodeBoundarySummary>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FunctionContextFieldSummary {
    pub offset: i32,
    pub width: u8,
    pub reads: usize,
    pub writes: usize,
    pub write_mask: u32,
    pub name: Option<String>,
    pub display_type: Option<String>,
    pub description: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FunctionContextSummary {
    pub argument: u8,
    pub name: Option<String>,
    pub type_name: Option<String>,
    pub fields: Vec<FunctionContextFieldSummary>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FunctionSummary {
    pub profile: String,
    pub source: String,
    pub identity: String,
    pub symbol: String,
    pub member: Option<String>,
    pub selection: FunctionSelection,
    pub review_status: FunctionReviewState,
    pub reviewed_name: Option<String>,
    pub role: Option<String>,
    pub summary: Option<String>,
    pub complete: bool,
    pub blockers: Vec<String>,
    pub decode_blockers: usize,
    pub decode_blocker_classes: Vec<String>,
    pub semantic_operations: Vec<String>,
    pub registers: Vec<u32>,
    pub calls: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FunctionDecodeBlockerSummary {
    pub address: u64,
    pub width: u8,
    pub raw: u32,
    pub class: String,
    pub linear_control_flow: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FunctionDetailSummary {
    pub identity: String,
    pub registers: Vec<u32>,
    pub contexts: Vec<FunctionContextSummary>,
    pub memory_fields: Vec<FunctionMemoryFieldSummary>,
    pub decode_blockers: Vec<FunctionDecodeBlockerSummary>,
    pub scenario_suggestions: Vec<ScenarioSuggestionSummary>,
    pub profile_draft: Option<String>,
    pub pseudo_rust: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ScenarioArgumentSummary {
    pub index: u8,
    pub value: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ScenarioMmioReadSummary {
    pub address: u32,
    pub mask: u32,
    pub expected: u32,
    pub values: Vec<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ScenarioSuggestionVariantSummary {
    pub name: String,
    pub arguments: Vec<ScenarioArgumentSummary>,
    pub mmio_reads: Vec<ScenarioMmioReadSummary>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ScenarioSuggestionSummary {
    pub kind: String,
    pub site: Option<u32>,
    pub evidence: String,
    pub variants: Vec<ScenarioSuggestionVariantSummary>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FunctionMemoryFieldSummary {
    pub object: String,
    pub offset: i64,
    pub width: u8,
    pub reads: usize,
    pub writes: usize,
    pub write_mask: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LogicalTypeBindingSummary {
    pub profile: String,
    pub source: String,
    pub name: String,
    pub object: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LogicalTypeFieldSummary {
    pub offset: i64,
    pub width: u8,
    pub status: FunctionReviewState,
    pub name: Option<String>,
    pub display_type: Option<String>,
    pub description: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LogicalTypeSummary {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub bindings: Vec<LogicalTypeBindingSummary>,
    pub fields: Vec<LogicalTypeFieldSummary>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RegisterSummary {
    pub address: u32,
    pub name: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RegisterReviewState {
    Reviewed,
    Manual,
    Ignored,
    Unreviewed,
}

impl RegisterReviewState {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Reviewed => "reviewed",
            Self::Manual => "manual",
            Self::Ignored => "ignored",
            Self::Unreviewed => "unreviewed",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RegisterNameSource {
    Model,
    Catalog,
    Discovery,
    Address,
}

impl RegisterNameSource {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::Catalog => "catalog/SVD",
            Self::Discovery => "discovery",
            Self::Address => "address",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RegisterWritePatternSummary {
    pub occurrences: usize,
    pub modified_mask: u32,
    pub preserved_mask: u32,
    pub inverted_mask: u32,
    pub forced_zero_mask: u32,
    pub forced_one_mask: u32,
    pub read_derived_mask: u32,
    pub dynamic_mask: u32,
    pub functions: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RegisterPredicateSummary {
    pub kind: String,
    pub function: String,
    pub producer_path: Vec<String>,
    pub condition: String,
    pub effective_operation: Option<String>,
    pub register_comparison_value: Option<u32>,
    pub transitive: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RegisterFieldSummary {
    pub least_significant_bit: u8,
    pub most_significant_bit: u8,
    pub mask: u32,
    pub write_shapes: usize,
    pub predicate_shapes: usize,
    pub poll_shapes: usize,
    pub functions: Vec<String>,
    pub predicate_functions: Vec<String>,
    pub semantic_operations: Vec<String>,
    pub semantic_roots: Vec<String>,
    pub predicates: Vec<RegisterPredicateSummary>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RegisterDetailSummary {
    pub address: u32,
    pub width: Option<u8>,
    pub name: String,
    pub name_source: RegisterNameSource,
    pub review_status: RegisterReviewState,
    pub review_confidence: Option<String>,
    pub review_sources: Vec<String>,
    pub reads: usize,
    pub writes: usize,
    pub read_modify_writes: usize,
    pub read_functions: Vec<String>,
    pub write_functions: Vec<String>,
    pub functions: Vec<String>,
    pub write_patterns: Vec<RegisterWritePatternSummary>,
    pub fields: Vec<RegisterFieldSummary>,
    pub semantic_operations: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RegisterWorkspaceReport {
    pub configured: bool,
    pub model: Option<PathBuf>,
    pub ranges: usize,
    pub observed: usize,
    pub reviewed: usize,
    pub ignored: usize,
    pub manual: usize,
    pub unreviewed: usize,
    pub fields: usize,
    pub registers: Vec<RegisterSummary>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct InterfaceContractSummary {
    pub id: String,
    pub source: String,
    pub layout_version: String,
    pub pointer_width: u8,
    pub layout_size: u32,
    pub slot_stride: u8,
    pub guards: usize,
    pub execution_contract: Option<String>,
    pub slots: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct InterfaceSlotSummary {
    pub id: String,
    pub contract: String,
    pub offset: i32,
    pub width: u8,
    pub name: String,
    pub arguments: Vec<String>,
    pub return_type: String,
    pub variadic: bool,
    pub semantic: Option<String>,
    pub effects: Vec<String>,
    pub replacement: Option<String>,
    pub execution_model: Option<String>,
    pub functions: Vec<String>,
    pub call_sites: Vec<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct InterfaceWorkspaceReport {
    pub configured: bool,
    pub facts: Option<PathBuf>,
    pub pack: Option<PathBuf>,
    pub observed_slots: usize,
    pub reviewed_slots: usize,
    pub unreviewed_slots: usize,
    pub contracts: Vec<InterfaceContractSummary>,
    pub slots: Vec<InterfaceSlotSummary>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ComparisonProfileSummary {
    pub name: String,
    pub path: PathBuf,
    pub vendor_source: String,
    pub vendor_symbol: String,
    pub rust_symbol: String,
    pub scenarios: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct WorkspaceSnapshot {
    pub generation: u64,
    pub project_status: crate::ProjectStatusReport,
    pub code: CodeWorkspaceReport,
    pub functions: Vec<FunctionSummary>,
    pub logical_types: Vec<LogicalTypeSummary>,
    pub registers: RegisterWorkspaceReport,
    pub interfaces: InterfaceWorkspaceReport,
    pub comparisons: Vec<ComparisonProfileSummary>,
    pub diagnostics: Vec<DiagnosticRecord>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct AnalyzeRequest {
    pub artifact: PathBuf,
    pub member: Option<String>,
    pub symbol: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AnalysisReport {
    pub symbol: String,
    pub exact: bool,
    pub reference_codegen_eligible: bool,
    pub return_value: String,
    pub events: Vec<String>,
    pub blockers: Vec<String>,
    pub reference_blockers: Vec<String>,
    pub unnamed_mmio: Vec<u32>,
}

#[derive(Clone, Debug, Default)]
pub struct ComparisonScenario {
    pub name: String,
    pub scenario: crate::ExecutionScenario,
    pub vendor_table_instances: Vec<crate::ExecutionTableInstance>,
    pub rust_table_instances: Vec<crate::ExecutionTableInstance>,
}

#[derive(Clone, Debug)]
pub struct CompareRequest {
    pub vendor_artifact: PathBuf,
    pub vendor_companion: Option<PathBuf>,
    pub vendor_symbol: String,
    pub rust_artifact: PathBuf,
    pub rust_companion: Option<PathBuf>,
    pub rust_symbol: String,
    pub compare_return: bool,
    pub argument_domain: Vec<[Option<u32>; 8]>,
    pub scenarios: Vec<ComparisonScenario>,
}
