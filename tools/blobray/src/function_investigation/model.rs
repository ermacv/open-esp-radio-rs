//! Serializable evidence models for focused function investigation.

use std::path::{Path, PathBuf};

use serde::Serialize;

use super::ReplacementEvidence;
use crate::{
    BlockerResolutionRoute, FactAccuracy, FactCompleteness, FactProvenance, Result, artifact,
    artifacts,
};

#[derive(Clone, Debug)]
pub struct StoredLinkedIrRecord(Box<serde_json::value::RawValue>);

impl StoredLinkedIrRecord {
    pub(super) fn from_function(function: &artifacts::StoredFunction) -> Result<Self> {
        Ok(Self(serde_json::value::RawValue::from_string(
            serde_json::to_string(function)?,
        )?))
    }
}

impl PartialEq for StoredLinkedIrRecord {
    fn eq(&self, other: &Self) -> bool {
        self.0.get() == other.0.get()
    }
}

impl Eq for StoredLinkedIrRecord {}

impl Serialize for StoredLinkedIrRecord {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.0.serialize(serializer)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FunctionInvestigationReport {
    pub schema_version: u32,
    pub command: &'static str,
    pub source: String,
    pub symbol: String,
    pub runtime: artifact::FunctionBody,
    pub origin: Option<OriginFunctionEvidence>,
    pub semantics: Vec<SemanticFunctionEvidence>,
    pub reviewed_preconditions: Vec<ReviewedPreconditionEvidence>,
    pub reviewed_paths: Vec<ReviewedPathEvidence>,
    pub cfg_path: Option<CfgPathEvidence>,
    pub proof_ledger: Vec<InvestigationLedgerEntry>,
    pub replacements: Vec<ReplacementEvidence>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CfgPathEvidence {
    pub from_address: u64,
    pub to_address: u64,
    pub from_block: usize,
    pub to_block: usize,
    pub structurally_reachable: bool,
    /// Always false: graph reachability alone does not prove satisfiable
    /// branch predicates or a realizable runtime state.
    pub feasibility_claim: bool,
    pub blocks: Vec<usize>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReviewedPreconditionEvidence {
    pub id: String,
    pub expression: String,
    pub rationale: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReviewedPathEvidence {
    pub id: String,
    pub class: String,
    pub summary: String,
    pub evidence: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OriginFunctionEvidence {
    pub association: &'static str,
    pub inventory_report: Option<String>,
    /// Authoritative address selected by the already generated link unit.
    /// This remains an association claim, not a reconstruction of linker
    /// selection, but lets an archive inspection reuse the matching linked IR.
    pub linked_address: Option<u64>,
    pub linked_member: Option<String>,
    /// Relocation-backed dependencies retained by the relocatable archive
    /// member. These are never projected onto linked instruction addresses by
    /// offset arithmetic: linker relaxation can change both instruction count
    /// and offsets, so an offset-only association would be unsound.
    pub relocation_dependencies: Vec<OriginRelocationDependency>,
    /// Monotonic structural correspondence between relocation-bearing origin
    /// instructions and linked instructions. This is navigation evidence,
    /// never an execution or semantic-equivalence claim.
    pub instruction_correspondence: Vec<OriginInstructionCorrespondence>,
    pub body: artifact::FunctionBody,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OriginRelocationDependency {
    pub symbol: String,
    pub references: usize,
    pub instruction_offsets: Vec<u64>,
    pub kinds: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OriginInstructionCorrespondence {
    pub origin_offsets: Vec<u64>,
    pub runtime_address: u64,
    pub runtime_offset: u64,
    pub kind: &'static str,
    pub relocation_symbols: Vec<String>,
    /// Always false. Structural instruction alignment helps investigation but
    /// does not prove identical runtime semantics after linker rewriting.
    pub semantic_equivalence_claim: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SemanticFunctionEvidence {
    pub profile: String,
    pub report: String,
    pub complete: bool,
    pub exact: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reviewed_function: Option<ReviewedFunctionEvidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reviewed_signature: Option<ReviewedFunctionSignatureEvidence>,
    pub pseudo: String,
    pub blockers: Vec<BlockerExplanationEvidence>,
    pub instruction_evidence: Vec<InstructionEvidence>,
    pub calls: Vec<CallKnowledgeEvidence>,
    pub reachable_functions: Vec<String>,
    pub call_graph_edges: Vec<CallGraphEdgeEvidence>,
    pub graph_limits: InvestigationGraphLimits,
    pub event_dispatches: Vec<EventDispatchEvidence>,
    pub reviewed_event_routes: Vec<ReviewedEventRouteEvidence>,
    /// Complete schema-validated persistent record, included only for an
    /// explicitly lossless investigation. Normal JSON remains a compact
    /// evidence report and points at `report` for indexed retrieval.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub linked_ir: Option<StoredLinkedIrRecord>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReviewedFunctionEvidence {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub accept_incomplete: bool,
    pub provenance: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReviewedFunctionSignatureEvidence {
    pub name: String,
    pub arguments: Vec<ReviewedFunctionArgumentEvidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub return_abi: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub return_role: Option<String>,
    pub provenance: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReviewedFunctionArgumentEvidence {
    pub index: u8,
    pub name: String,
    pub abi: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct InvestigationGraphLimits {
    pub max_depth: usize,
    pub max_visited_nodes: usize,
    pub max_examined_edges: usize,
    pub visited_nodes: usize,
    pub examined_edges: usize,
    pub reached: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReviewedEventRouteEvidence {
    pub id: String,
    pub mechanism: String,
    pub selector_role: String,
    pub selector_value: u32,
    pub receiver: Option<String>,
    pub execution_context: String,
    pub consumer_profile: String,
    pub consumer_source: String,
    pub consumer_entry: String,
    pub delivery_operation: String,
    pub delivery_output_role: String,
    pub delivery_selector_offset: u32,
    pub delivery_selector_width: u8,
    pub delivery_encoding: String,
    pub case_handler_profile: Option<String>,
    pub case_handler_source: Option<String>,
    pub case_handler: Option<String>,
    pub rationale: String,
    pub dispatch_constraint_matched: bool,
    pub consumer_analysis: Option<EventHandlerAnalysisEvidence>,
    pub case_handler_analysis: Option<EventHandlerAnalysisEvidence>,
    pub blockers: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct EventHandlerAnalysisEvidence {
    pub identity: String,
    pub complete: bool,
    pub exact: bool,
    pub direct_instruction_effects: usize,
    pub direct_calls: usize,
    pub reachable_functions: usize,
    pub reachability_depth: usize,
    pub reachability_limit: Option<String>,
    pub blockers: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BlockerExplanationEvidence {
    pub root_id: String,
    pub layer: String,
    pub kind: String,
    pub site: Option<u32>,
    pub message: String,
    pub resolution_route: BlockerResolutionRoute,
    pub relocation_candidates: Vec<String>,
    pub provenance: FactProvenance,
    pub accuracy: FactAccuracy,
    pub completeness: FactCompleteness,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct InstructionEvidence {
    pub address: u64,
    pub block: Option<usize>,
    pub effects: Vec<InstructionEffectEvidence>,
    pub call_targets: Vec<String>,
    pub semantic_operations: Vec<String>,
    pub blocker_ids: Vec<String>,
    pub decode_blocker: Option<String>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct InstructionEffectEvidence {
    pub kind: &'static str,
    pub access: String,
    pub width: u8,
    pub target: String,
    pub paths: Vec<String>,
    pub guards: Vec<String>,
    pub value: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct EventDispatchEvidence {
    pub semantic_action_index: usize,
    pub mechanism: String,
    pub execution_context: String,
    pub receiver: Option<String>,
    pub interface_complete: bool,
    pub bindings: Vec<EventDispatchBindingEvidence>,
    pub blockers: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct EventDispatchBindingEvidence {
    pub role: String,
    pub value: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CallGraphEdgeEvidence {
    pub caller: String,
    pub callee: String,
    pub site: Option<u32>,
    pub kind: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CallKnowledgeEvidence {
    pub kind: String,
    pub target: String,
    pub site: Option<u32>,
    pub target_status: &'static str,
    pub target_candidates: Vec<String>,
    pub target_blocker: Option<String>,
    pub knowledge: &'static str,
    pub semantic_operation: Option<String>,
    pub execution_model: Option<String>,
    /// ABI argument expressions recovered at this exact call site. Multiple
    /// branch shapes are retained as an explicit domain by linked IR.
    pub arguments: Vec<String>,
    pub argument_evidence: Vec<CallArgumentEvidence>,
    pub argument_shapes: usize,
    pub guards: Vec<String>,
    pub provenance: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CallArgumentEvidence {
    pub position: usize,
    pub status: &'static str,
    pub value: String,
    pub provenance: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct InvestigationLedgerEntry {
    pub layer: &'static str,
    pub status: &'static str,
    pub detail: String,
}

pub(crate) struct FunctionInvestigationRequest<'a> {
    pub(crate) source: &'a str,
    pub(crate) symbol: &'a str,
    pub(crate) runtime_address: Option<u64>,
    pub(crate) artifact: &'a Path,
    pub(crate) inventories: &'a [PathBuf],
    pub(crate) member: Option<&'a str>,
    pub(crate) origin_member: Option<&'a str>,
    pub(crate) graph_depth: usize,
    pub(crate) include_callers: bool,
    pub(crate) cfg_path: Option<&'a str>,
    pub(crate) include_linked_ir_record: bool,
}
