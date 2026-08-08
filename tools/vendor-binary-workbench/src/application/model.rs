//! Public data-only application requests and snapshots.

use std::{collections::BTreeMap, path::PathBuf};

use serde::Serialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkspaceReadiness {
    Ready,
    Incomplete,
    NotConfigured,
    Invalid,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct WorkspaceComponentSnapshot {
    pub name: String,
    pub status: WorkspaceReadiness,
    pub details: BTreeMap<String, serde_json::Value>,
    pub diagnostic: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct WorkspacePhaseSnapshot {
    pub name: String,
    pub status: WorkspaceReadiness,
    pub components: Vec<WorkspaceComponentSnapshot>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProjectStatusSnapshot {
    pub project_id: String,
    pub manifest: String,
    pub target_id: String,
    pub architecture: String,
    pub calling_convention: String,
    pub harness: Option<String>,
    pub overall: WorkspaceReadiness,
    pub phases: Vec<WorkspacePhaseSnapshot>,
}

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
    pub semantic_operations: Vec<String>,
    pub registers: Vec<u32>,
    pub calls: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FunctionDetailSummary {
    pub identity: String,
    pub registers: Vec<u32>,
    pub contexts: Vec<FunctionContextSummary>,
    pub memory_fields: Vec<FunctionMemoryFieldSummary>,
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RegisterWorkspaceReport {
    pub configured: bool,
    pub model: Option<PathBuf>,
    pub ranges: usize,
    pub observed: usize,
    pub reviewed: usize,
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
    pub project_status: ProjectStatusSnapshot,
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
