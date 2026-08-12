//! Complete owned DTO for linked-IR schema v51.

#![allow(
    dead_code,
    reason = "complete stored DTOs enforce every persistent schema field"
)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LinkedIrStoredDocument {
    schema_version: u32,
    command: String,
    analysis_mode: String,
    linkage_mode: String,
    project_call_linkage: String,
    selection_mode: String,
    include_reachable: bool,
    effect_summary_mode: String,
    context_projection_mode: String,
    memory_object_mode: String,
    instruction_effect_mode: String,
    data_object_mode: String,
    indexed_dispatch_mode: String,
    indexed_dispatch_completeness_claim: bool,
    semantic_action_mode: String,
    event_dispatch_mode: String,
    event_dispatch_effect_completeness_claim: bool,
    event_dispatch_receiver_inference_mode: String,
    mmio_field_candidate_mode: String,
    direct_mmio_predicate_completeness_claim: bool,
    scenario_suggestion_mode: String,
    scenario_suggestion_proof_claim: bool,
    pub(crate) mmio_field_semantics_claim: bool,
    cfg_guard_completeness_claim: bool,
    pub(crate) completeness_claim: bool,
    pub(crate) artifacts: Vec<StoredSourceArtifact>,
    companions: Vec<StoredArtifactIdentity>,
    symbol_prefix: String,
    entry_contract: String,
    pub(crate) summary: StoredReportSummary,
    pub(crate) data_objects: Vec<StoredDataObject>,
    pub(crate) mmio_registers: Vec<StoredMmioRegister>,
    semantic_boundaries: Vec<StoredSemanticBoundary>,
    trampoline_slots: Vec<StoredTrampolineSlot>,
    pub(crate) functions: Vec<StoredFunction>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredSourceArtifact {
    pub(crate) source: String,
    pub(crate) artifact: StoredArtifactIdentity,
    reviewed_code_boundaries: Vec<StoredReviewedCodeBoundary>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredReviewedCodeBoundary {
    member: Option<String>,
    section: String,
    name: String,
    start_offset: String,
    end_offset: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredArtifactIdentity {
    pub(crate) path: String,
    pub(crate) sha256: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredReportSummary {
    artifacts: usize,
    reviewed_code_boundaries: usize,
    pub(crate) functions: usize,
    decode_blocker_functions: usize,
    pub(crate) decode_blockers: usize,
    root_functions: usize,
    included_reachable_functions: usize,
    exported: usize,
    local: usize,
    pub(crate) mmio_registers: usize,
    mmio_functions: usize,
    mmio_access_shapes: usize,
    instruction_effects: usize,
    mmio_field_candidate_registers: usize,
    pub(crate) mmio_field_candidates: usize,
    direct_mmio_predicates: usize,
    direct_mmio_predicate_sources: usize,
    delay_functions: usize,
    delay_shapes: usize,
    context_functions: usize,
    context_fields: usize,
    context_accesses: usize,
    memory_functions: usize,
    memory_fields: usize,
    memory_accesses: usize,
    semantic_operations: usize,
    semantic_calls: usize,
    trampoline_slots: usize,
    trampoline_calls: usize,
    complete: usize,
    structured: usize,
    internal_calls: usize,
    indexed_dispatch_calls: usize,
    external_calls: usize,
    call_argument_shapes: usize,
    project_linked_calls: usize,
    ambiguous_project_calls: usize,
    unresolved_calls: usize,
    closed_effect_summaries: usize,
    recursive_effect_summaries: usize,
    complete_context_projections: usize,
    projected_context_fields: usize,
    projected_memory_fields: usize,
    exact_return_functions: usize,
    return_source_ranges: usize,
    mmio_return_sources: usize,
    guard_mmio_links: usize,
    transitive_guard_mmio_links: usize,
    scenario_suggestions: usize,
    data_objects: usize,
    initialized_data_objects: usize,
    data_object_relocations: usize,
    data_object_xrefs: usize,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredDataObject {
    pub(crate) source: String,
    pub(crate) member: Option<String>,
    section: String,
    pub(crate) symbol: String,
    aliases: Vec<String>,
    pub(crate) address: Option<String>,
    object_offset: String,
    pub(crate) size: u64,
    writable: bool,
    initialized: bool,
    synthetic_from_anchor: bool,
    exported: bool,
    initializer_hex: Option<String>,
    relocations: Vec<StoredDataObjectRelocation>,
    pub(crate) xrefs: Vec<StoredDataObjectXref>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredDataObjectRelocation {
    offset: String,
    elf_type: Option<u32>,
    target: String,
    addend: i64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredDataObjectXref {
    pub(crate) function: String,
    pub(crate) reads: usize,
    pub(crate) writes: usize,
    pub(crate) offsets: Vec<String>,
    indexed_by: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredFunction {
    pub(crate) source: String,
    pub(crate) identity: String,
    pub(crate) selection: String,
    pub(crate) member: Option<String>,
    pub(crate) symbol: String,
    binding: String,
    pub(crate) address: Option<u32>,
    pub(crate) object_offset: u32,
    size: usize,
    flow_kind: String,
    pub(crate) complete: bool,
    exact: bool,
    return_value: String,
    return_provenance: StoredReturnProvenance,
    dependencies: Vec<String>,
    pub(crate) projected_relocations: Vec<StoredProjectedRelocation>,
    pub(crate) local_value_flow: Vec<StoredLocalValueFlow>,
    pub(crate) calls: Vec<StoredCall>,
    direct_mmio_predicates: Vec<StoredDirectMmioPredicate>,
    pub(crate) mmio_accesses: Vec<StoredMmioAccess>,
    pub(crate) instruction_effects: Vec<StoredInstructionEffect>,
    pub(crate) delays: Vec<StoredDelay>,
    context_accesses: Vec<StoredContextAccess>,
    context_fields: Vec<StoredFunctionContextField>,
    memory_accesses: Vec<StoredMemoryAccess>,
    memory_fields: Vec<StoredFunctionMemoryField>,
    pub(crate) scenario_suggestions: Vec<StoredScenarioSuggestion>,
    pub(crate) effect_summary: StoredEffectSummary,
    call_graph_diagnostics: Vec<StoredDiagnostic>,
    direct_diagnostics: Vec<StoredDiagnostic>,
    reference_diagnostics: Vec<StoredDiagnostic>,
    pub(crate) decode_blockers: Vec<StoredDecodeBlocker>,
    pub(crate) pseudo: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredProjectedRelocation {
    pub(crate) site: u32,
    pub(crate) origin_member: Option<String>,
    pub(crate) origin_symbol: String,
    pub(crate) origin_offsets: Vec<u32>,
    pub(crate) kind: String,
    pub(crate) symbol: String,
    pub(crate) addend: i64,
    pub(crate) correspondence: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) enum StoredLocalValueFlow {
    StackStore {
        site: u32,
        offset: i32,
        width: u8,
        value: StoredFlowValue,
    },
    StackLoad {
        site: u32,
        token: u32,
        offset: i32,
        width: u8,
        signed: bool,
    },
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredFlowValue {
    pub(crate) expression: String,
    pub(crate) constant: Option<u32>,
    pub(crate) input: Option<u8>,
}

/// Allocation-bounded record from the dedicated function overview stream used
/// by project status and the TUI index.  Unlike the former projection over the
/// full function JSON, this schema is strict and contains no lossless IR body.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredFunctionReviewProjection {
    pub(crate) source: String,
    pub(crate) identity: String,
    pub(crate) selection: String,
    pub(crate) member: Option<String>,
    pub(crate) symbol: String,
    pub(crate) binding: String,
    pub(crate) complete: bool,
    pub(crate) dependencies: Vec<String>,
    pub(crate) direct_calls: usize,
    pub(crate) calls: Vec<StoredReviewCall>,
    pub(crate) mmio: Vec<StoredReviewMmio>,
    pub(crate) mmio_addresses: Vec<u32>,
    pub(crate) direct_context_fields: usize,
    pub(crate) direct_memory_fields: usize,
    pub(crate) diagnostics: Vec<StoredReviewDiagnostic>,
    pub(crate) effect_summary: StoredReviewEffectSummary,
    pub(crate) decode_blockers: Vec<StoredDecodeBlocker>,
}

impl StoredFunctionReviewProjection {
    pub(crate) fn is_exported(&self) -> bool {
        self.binding == "global-or-weak"
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredReviewCall {
    pub(crate) kind: String,
    pub(crate) target: String,
    pub(crate) site: Option<u32>,
    pub(crate) project_symbol: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredReviewMmio {
    pub(crate) address: u32,
    pub(crate) width: u8,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredReviewDiagnostic {
    pub(crate) channel: String,
    pub(crate) root_id: String,
    pub(crate) kind: String,
    pub(crate) site: Option<u32>,
    pub(crate) rendered: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredReviewEffectSummary {
    pub(crate) transitive_effects_materialized: bool,
    pub(crate) call_graph_closed: bool,
    pub(crate) context_projection_materialized: bool,
    pub(crate) context_projection_complete: bool,
    pub(crate) context_projection_blockers: Vec<String>,
    pub(crate) context_fields: Vec<StoredReviewContextField>,
    pub(crate) memory_fields: Vec<StoredReviewMemoryField>,
    pub(crate) semantic_operations: Vec<String>,
    pub(crate) trampoline_calls: usize,
    pub(crate) event_dispatches: Vec<StoredReviewEventDispatch>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredReviewContextField {
    pub(crate) argument: u8,
    pub(crate) offset: i32,
    pub(crate) width: u8,
    pub(crate) reads: usize,
    pub(crate) writes: usize,
    pub(crate) write_mask: u32,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredReviewMemoryField {
    pub(crate) object: StoredMemoryObject,
    pub(crate) offset: i64,
    pub(crate) width: u8,
    pub(crate) reads: usize,
    pub(crate) writes: usize,
    pub(crate) write_mask: u32,
    pub(crate) origins: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredReviewEventDispatch {
    pub(crate) mechanism: String,
    pub(crate) execution_context: String,
    pub(crate) receiver: Option<String>,
    pub(crate) interface_complete: bool,
    pub(crate) bindings: Vec<StoredReviewEventBinding>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredReviewEventBinding {
    pub(crate) role: String,
    pub(crate) value: String,
}

impl StoredFunction {
    pub(crate) const fn exact(&self) -> bool {
        self.exact
    }

    pub(crate) fn is_exported(&self) -> bool {
        self.binding == "global-or-weak"
    }

    pub(crate) fn dependencies(&self) -> &[String] {
        &self.dependencies
    }

    pub(crate) fn context_field_count(&self) -> usize {
        self.context_fields.len()
    }

    pub(crate) fn memory_field_count(&self) -> usize {
        self.memory_fields.len()
    }

    pub(crate) fn direct_instruction_effect_count(&self) -> usize {
        self.instruction_effects.len()
    }

    pub(crate) fn direct_call_count(&self) -> usize {
        self.calls.len()
    }

    pub(crate) fn blockers(&self) -> impl Iterator<Item = &str> {
        self.call_graph_diagnostics
            .iter()
            .chain(&self.direct_diagnostics)
            .chain(&self.reference_diagnostics)
            .map(|diagnostic| diagnostic.rendered.as_str())
    }

    pub(crate) fn direct_blocker_count(&self) -> usize {
        self.direct_diagnostics.len()
    }

    pub(crate) fn call_graph_blocker_count(&self) -> usize {
        self.call_graph_diagnostics.len()
    }

    pub(crate) fn reference_blocker_count(&self) -> usize {
        self.reference_diagnostics.len()
    }

    pub(crate) fn diagnostics(&self) -> impl Iterator<Item = (&'static str, &StoredDiagnostic)> {
        self.call_graph_diagnostics
            .iter()
            .map(|diagnostic| ("call-graph", diagnostic))
            .chain(
                self.direct_diagnostics
                    .iter()
                    .map(|diagnostic| ("direct", diagnostic)),
            )
            .chain(
                self.reference_diagnostics
                    .iter()
                    .map(|diagnostic| ("reference", diagnostic)),
            )
    }
}

impl LinkedIrStoredDocument {
    pub(crate) fn replace_bundle_payload(
        &mut self,
        functions: Vec<StoredFunction>,
        mmio_registers: Vec<StoredMmioRegister>,
        data_objects: Vec<StoredDataObject>,
    ) {
        self.functions = functions;
        self.mmio_registers = mmio_registers;
        self.data_objects = data_objects;
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredDecodeBlocker {
    pub(crate) address: u64,
    pub(crate) width: u8,
    pub(crate) raw: u32,
    pub(crate) class: String,
    pub(crate) linear_control_flow: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredReturnProvenance {
    exact: bool,
    known_zero_bits: u32,
    known_one_bits: u32,
    unknown_bits: u32,
    sources: Vec<StoredReturnBitSource>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredReturnBitSource {
    kind: String,
    output_lsb: u8,
    source_lsb: u8,
    width: u8,
    output_bits: u32,
    source_bits: u32,
    inverted: bool,
    argument: Option<u8>,
    token: Option<u32>,
    target: Option<String>,
    address: Option<u32>,
    register: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredCall {
    pub(crate) kind: String,
    pub(crate) target: String,
    pub(crate) site: Option<u32>,
    tail: bool,
    result_modeled: bool,
    execution_model: Option<StoredExternalExecutionModel>,
    semantics: Option<String>,
    pub(crate) semantic_operation: Option<String>,
    pub(crate) semantic_contract: Option<StoredSemanticContract>,
    replacement_hint: Option<String>,
    project_symbol: Option<String>,
    project_candidates: Vec<String>,
    trampoline: Option<StoredTrampoline>,
    argument_shapes: usize,
    pub(crate) arguments: Vec<String>,
    argument_bindings: Vec<StoredArgumentBinding>,
    typed_arguments: Vec<StoredCallArgument>,
    pub(crate) guard_paths: Option<Vec<StoredGuardPath>>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredExternalExecutionModel {
    id: String,
    return_model: String,
    outputs: Vec<StoredExternalOutputModel>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredExternalOutputModel {
    kind: String,
    pointer_argument: u8,
    width: u8,
}

impl StoredCall {
    pub(crate) fn project_symbol(&self) -> Option<&str> {
        self.project_symbol.as_deref()
    }

    pub(crate) fn project_candidates(&self) -> &[String] {
        &self.project_candidates
    }

    pub(crate) fn semantic_contract_provenance(&self) -> Option<String> {
        self.semantic_contract.as_ref().map(|contract| {
            format!(
                "{}:{} ({})",
                contract.source, contract.id, contract.evidence
            )
        })
    }

    pub(crate) fn trampoline_provenance(&self) -> Option<String> {
        self.trampoline.as_ref().map(|trampoline| {
            format!(
                "table={} pointer={} backing={} slot={:#x} function={}",
                trampoline.table,
                trampoline.pointer_symbol,
                trampoline.backing_symbol,
                trampoline.slot,
                trampoline.c_name
            )
        })
    }

    pub(crate) fn knowledge(&self) -> &'static str {
        if self.execution_model.is_some() {
            "executable"
        } else if self.semantic_operation.is_some() {
            "annotated"
        } else if matches!(self.kind.as_str(), "internal" | "indexed-dispatch") {
            "internal-code"
        } else if self.kind == "project-linked" {
            "linked-code"
        } else {
            "unknown"
        }
    }

    pub(crate) fn execution_model_id(&self) -> Option<&str> {
        self.execution_model.as_ref().map(|model| model.id.as_str())
    }

    pub(crate) fn models_output(&self, pointer_argument: usize, width: u8) -> bool {
        self.execution_model.as_ref().is_some_and(|model| {
            model.outputs.iter().any(|output| {
                usize::from(output.pointer_argument) == pointer_argument && output.width == width
            })
        })
    }

    pub(crate) const fn argument_shapes(&self) -> usize {
        self.argument_shapes
    }

    pub(crate) const fn tail(&self) -> bool {
        self.tail
    }

    pub(crate) fn guard_expressions(&self) -> Vec<String> {
        self.guard_paths
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|path| {
                if path.guards.is_empty() {
                    return "true".to_owned();
                }
                path.guards
                    .iter()
                    .map(|guard| {
                        if guard.taken {
                            format!("({})", guard.condition)
                        } else {
                            format!("!({})", guard.condition)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" && ")
            })
            .collect()
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredCallArgument {
    position: usize,
    name: String,
    c_type: String,
    direction: String,
    value: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredArgumentBinding {
    position: usize,
    caller_argument: u8,
    offset: i32,
    expression: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredTrampoline {
    table: String,
    pointer_symbol: String,
    backing_symbol: String,
    version: u32,
    magic: u32,
    table_size: u32,
    magic_offset: u32,
    function_id: String,
    slot: u32,
    c_name: String,
    argument_count: u8,
    return_model: String,
    operation: String,
    return_type: String,
    replacement_hint: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredSemanticContract {
    source: String,
    id: String,
    evidence: String,
    pub(crate) event_dispatch: Option<StoredEventDispatchContract>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredEventDispatchContract {
    pub(crate) mechanism: String,
    pub(crate) execution_context: String,
    pub(crate) receiver: Option<String>,
    pub(crate) argument_roles: Vec<StoredEventDispatchArgumentRole>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredEventDispatchArgumentRole {
    pub(crate) role: String,
    pub(crate) argument: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredGuardPath {
    pub(crate) guards: Vec<StoredGuard>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredGuard {
    site: u32,
    pub(crate) condition: String,
    operation: String,
    pub(crate) taken: bool,
    result_sources: Vec<StoredGuardResultSource>,
    direct_mmio_sources: Vec<StoredDirectMmioPredicateSource>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredGuardResultSource {
    kind: String,
    token: u32,
    target: Option<String>,
    operand: String,
    value_bits: Option<u32>,
    source_bits: u32,
    inverted: bool,
    comparison_value: Option<u32>,
    source_comparison_value: Option<u32>,
    producer_return_exact: Option<bool>,
    mmio_sources: Vec<StoredGuardMmioSource>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredGuardMmioSource {
    address: u32,
    register: String,
    producer_path: Vec<String>,
    result_bits: u32,
    register_bits: u32,
    inverted: bool,
    result_comparison_value: Option<u32>,
    register_comparison_value: Option<u32>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredDirectMmioPredicateSource {
    operand: String,
    read_token: u32,
    address: u32,
    register: String,
    value_bits: u32,
    register_bits: u32,
    inverted: bool,
    comparison_value: Option<u32>,
    register_comparison_value: Option<u32>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredDirectMmioPredicate {
    site: u32,
    condition: String,
    operation: String,
    sources: Vec<StoredDirectMmioPredicateSource>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredMmioAccess {
    ordinal: usize,
    pub(crate) address: u32,
    width: u8,
    register: String,
    access: String,
    mode: String,
    path: String,
    address_expression: Option<String>,
    guard: Option<String>,
    predicate_mask: Option<u32>,
    predicate_expected: Option<u32>,
    value: Option<String>,
    modified_mask: Option<u32>,
    preserved_mask: Option<u32>,
    inverted_mask: Option<u32>,
    forced_zero_mask: Option<u32>,
    forced_one_mask: Option<u32>,
    read_derived_mask: Option<u32>,
    dynamic_mask: Option<u32>,
}

impl StoredMmioAccess {
    pub(crate) const fn width(&self) -> u8 {
        self.width
    }

    pub(crate) fn register(&self) -> &str {
        &self.register
    }

    pub(crate) fn access(&self) -> &str {
        &self.access
    }

    pub(crate) fn value(&self) -> Option<&str> {
        self.value.as_deref()
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) enum StoredInstructionEffect {
    Mmio {
        site: u32,
        block: Option<usize>,
        access: String,
        width: u8,
        address: u32,
        register: String,
        mode: String,
        paths: Vec<String>,
        guards: Vec<String>,
        value: Option<String>,
        modified_mask: Option<u32>,
        preserved_mask: Option<u32>,
        forced_zero_mask: Option<u32>,
        forced_one_mask: Option<u32>,
    },
    Memory {
        site: u32,
        block: Option<usize>,
        access: String,
        width: u8,
        object: StoredMemoryObject,
        offset: i64,
        paths: Vec<String>,
        value: Option<String>,
        value_pseudo: Option<String>,
        write_mask: Option<u32>,
        preserved_mask: Option<u32>,
        forced_zero_mask: Option<u32>,
        forced_one_mask: Option<u32>,
    },
}

impl StoredInstructionEffect {
    pub(crate) fn mmio(&self) -> Option<(&str, u8, u32, &str, Option<&str>)> {
        match self {
            Self::Mmio {
                access,
                width,
                address,
                register,
                value,
                ..
            } => Some((access, *width, *address, register, value.as_deref())),
            Self::Memory { .. } => None,
        }
    }

    pub(crate) const fn site(&self) -> u32 {
        match self {
            Self::Mmio { site, .. } | Self::Memory { site, .. } => *site,
        }
    }

    pub(crate) const fn block(&self) -> Option<usize> {
        match self {
            Self::Mmio { block, .. } | Self::Memory { block, .. } => *block,
        }
    }

    pub(crate) fn investigation_fields(
        &self,
    ) -> (
        &'static str,
        &str,
        u8,
        String,
        &[String],
        &[String],
        Option<&str>,
    ) {
        match self {
            Self::Mmio {
                access,
                width,
                address,
                register,
                paths,
                guards,
                value,
                ..
            } => (
                "mmio",
                access,
                *width,
                format!("{register} ({address:#010x})"),
                paths,
                guards,
                value.as_deref(),
            ),
            Self::Memory {
                access,
                width,
                object,
                offset,
                paths,
                value_pseudo,
                value,
                ..
            } => (
                "memory",
                access,
                *width,
                format!("{} {offset:+#x}", object.display_name()),
                paths,
                &[],
                value_pseudo.as_deref().or(value.as_deref()),
            ),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredDelay {
    pub(crate) ordinal: usize,
    pub(crate) path: String,
    pub(crate) micros: String,
    pub(crate) constant_micros: Option<u32>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredContextAccess {
    argument: u8,
    offset: i32,
    access: String,
    width: u8,
    path: String,
    value: Option<String>,
    value_pseudo: Option<String>,
    write_mask: Option<u32>,
    preserved_mask: Option<u32>,
    forced_zero_mask: Option<u32>,
    forced_one_mask: Option<u32>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredFunctionContextField {
    argument: u8,
    offset: i32,
    width: u8,
    reads: usize,
    writes: usize,
    write_mask: u32,
    paths: Vec<String>,
    write_values: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredMemoryAccess {
    object: StoredMemoryObject,
    offset: i64,
    access: String,
    width: u8,
    path: String,
    value: Option<String>,
    value_pseudo: Option<String>,
    write_mask: Option<u32>,
    preserved_mask: Option<u32>,
    forced_zero_mask: Option<u32>,
    forced_one_mask: Option<u32>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredFunctionMemoryField {
    object: StoredMemoryObject,
    offset: i64,
    width: u8,
    reads: usize,
    writes: usize,
    write_mask: u32,
    paths: Vec<String>,
    write_values: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) enum StoredMemoryObject {
    Argument {
        index: u8,
    },
    Global {
        member: Option<String>,
        symbol: String,
    },
    Dereferenced {
        pointer: Box<StoredMemoryObject>,
        pointer_offset: i64,
    },
    Absolute {
        address_space: String,
        address: u32,
    },
    Indexed {
        object: Box<StoredMemoryObject>,
        argument: u8,
        stride: i64,
    },
    ZeroedAllocation {
        call_token: u32,
    },
    OpaqueExternalObject {
        call_token: u32,
    },
}

impl StoredMemoryObject {
    fn display_name(&self) -> String {
        match self {
            Self::Argument { index } => format!("arg{index}"),
            Self::Global { member, symbol } => member
                .as_deref()
                .map_or_else(|| symbol.clone(), |member| format!("{member}::{symbol}")),
            Self::Dereferenced {
                pointer,
                pointer_offset,
            } => format!("*({} {pointer_offset:+#x})", pointer.display_name()),
            Self::Absolute {
                address_space,
                address,
            } => format!("{address_space}:{address:#010x}"),
            Self::Indexed {
                object,
                argument,
                stride,
            } => format!("{}[arg{argument} * {stride:#x}]", object.display_name()),
            Self::ZeroedAllocation { call_token } => format!("calloc#{call_token}"),
            Self::OpaqueExternalObject { call_token } => format!("opaque-external#{call_token}"),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredEffectSummary {
    pub(crate) transitive_effects_materialized: bool,
    pub(crate) call_graph_closed: bool,
    max_depth: usize,
    pub(crate) reachable_function_count: usize,
    recursive: bool,
    pub(crate) mmio_registers: Vec<StoredSummaryMmio>,
    pub(crate) delays: Vec<StoredSummaryDelay>,
    pub(crate) semantic_operations: Vec<StoredSemanticOperation>,
    pub(crate) context_projection_materialized: bool,
    pub(crate) context_projection_complete: bool,
    context_projection_paths_materialized: bool,
    pub(crate) context_projection_blockers: Vec<String>,
    pub(crate) context_fields: Vec<StoredContextField>,
    pub(crate) memory_fields: Vec<StoredMemoryField>,
    pub(crate) trampoline_calls: Vec<StoredProjectedTrampolineCall>,
    semantic_action_count: usize,
    pub(crate) semantic_actions_materialized: bool,
    pub(crate) semantic_actions: Vec<StoredProjectedSemanticAction>,
    pub(crate) event_dispatches: Vec<StoredEventDispatch>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredSummaryMmio {
    pub(crate) address: u32,
    pub(crate) width: u8,
    pub(crate) access_shapes: usize,
    pub(crate) accesses: Vec<String>,
    pub(crate) modes: Vec<String>,
    pub(crate) origins: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredSummaryDelay {
    pub(crate) micros: String,
    pub(crate) constant_micros: Option<u32>,
    pub(crate) delay_shapes: usize,
    pub(crate) origins: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredSemanticOperation {
    pub(crate) operation: String,
    pub(crate) call_shapes: usize,
    pub(crate) targets: Vec<String>,
    pub(crate) replacement_hints: Vec<String>,
    pub(crate) origins: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredContextField {
    pub(crate) argument: u8,
    pub(crate) offset: i32,
    pub(crate) width: u8,
    pub(crate) reads: usize,
    pub(crate) writes: usize,
    pub(crate) write_mask: u32,
    origins: Vec<String>,
    paths: Vec<String>,
    write_values: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredMemoryField {
    pub(crate) object: StoredMemoryObject,
    pub(crate) offset: i64,
    pub(crate) width: u8,
    pub(crate) reads: usize,
    pub(crate) writes: usize,
    pub(crate) write_mask: u32,
    pub(crate) origins: Vec<String>,
    paths: Vec<String>,
    write_values: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredProjectedCallArgument {
    position: usize,
    name: String,
    c_type: String,
    direction: String,
    value: String,
    binding: String,
    root_argument: Option<u8>,
    root_offset: Option<i32>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredProjectedTrampolineCall {
    trampoline: StoredTrampoline,
    origin: String,
    path: String,
    argument_shapes: usize,
    arguments: Vec<StoredProjectedCallArgument>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredProjectedSemanticAction {
    pub(crate) site_path: Vec<Option<u32>>,
    pub(crate) operation: String,
    pub(crate) target: String,
    contract: Option<StoredSemanticContract>,
    pub(crate) replacement_hint: Option<String>,
    pub(crate) origin: String,
    pub(crate) path: String,
    pub(crate) site: Option<u32>,
    pub(crate) argument_shapes: usize,
    pub(crate) arguments: Vec<StoredProjectedCallArgument>,
    guard_scopes: Option<Vec<StoredGuardScope>>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredGuardScope {
    function: String,
    paths: Vec<StoredGuardPath>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredEventDispatch {
    pub(crate) semantic_action_index: usize,
    pub(crate) mechanism: String,
    pub(crate) execution_context: String,
    pub(crate) receiver: Option<String>,
    pub(crate) interface_complete: bool,
    pub(crate) blockers: Vec<String>,
    pub(crate) bindings: Vec<StoredEventDispatchBinding>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredEventDispatchBinding {
    pub(crate) role: String,
    pub(crate) argument: StoredProjectedCallArgument,
}

impl StoredProjectedCallArgument {
    pub(crate) fn value(&self) -> &str {
        &self.value
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredScenarioSuggestion {
    pub(crate) kind: String,
    pub(crate) site: Option<u32>,
    pub(crate) evidence: String,
    pub(crate) variants: Vec<StoredScenarioVariant>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredScenarioVariant {
    pub(crate) name: String,
    pub(crate) arguments: Vec<StoredScenarioArgument>,
    pub(crate) mmio_reads: Vec<StoredScenarioMmioRead>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredScenarioArgument {
    pub(crate) index: u8,
    pub(crate) value: u32,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredScenarioMmioRead {
    pub(crate) address: u32,
    pub(crate) mask: u32,
    pub(crate) expected: u32,
    pub(crate) values: Vec<u32>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredMmioRegister {
    pub(crate) address: u32,
    pub(crate) width: u8,
    pub(crate) names: Vec<String>,
    read_shapes: usize,
    write_shapes: usize,
    poll_shapes: usize,
    predicate_shapes: usize,
    static_shapes: usize,
    indexed_candidate_shapes: usize,
    whole_register_write_shapes: usize,
    whole_register_predicate_shapes: usize,
    whole_register_poll_shapes: usize,
    read_modify_write_shapes: usize,
    write_masks: Vec<u32>,
    predicate_masks: Vec<u32>,
    poll_masks: Vec<u32>,
    candidate_bit_ranges: Vec<StoredMmioBitRange>,
    pub(crate) field_candidates: Vec<StoredFieldCandidate>,
    pub(crate) functions: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredMmioBitRange {
    least_significant_bit: u8,
    most_significant_bit: u8,
    mask: u32,
    write_shapes: usize,
    functions: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredFieldCandidate {
    pub(crate) least_significant_bit: u8,
    pub(crate) most_significant_bit: u8,
    pub(crate) mask: u32,
    pub(crate) write_shapes: usize,
    pub(crate) predicate_shapes: usize,
    pub(crate) poll_shapes: usize,
    pub(crate) functions: Vec<String>,
    pub(crate) access_functions: Vec<String>,
    pub(crate) predicate_functions: Vec<String>,
    pub(crate) predicate_evidence: Vec<StoredPredicateEvidence>,
    pub(crate) semantic_operations: Vec<String>,
    pub(crate) semantic_roots: Vec<String>,
    pub(crate) semantic_evidence: Vec<StoredSemanticEvidence>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredPredicateEvidence {
    pub(crate) kind: String,
    pub(crate) function: String,
    producer: Option<String>,
    pub(crate) producer_path: Vec<String>,
    site: Option<u32>,
    path: Option<String>,
    pub(crate) condition: String,
    operation: String,
    taken: Option<bool>,
    pub(crate) effective_operation: Option<String>,
    operand: Option<String>,
    comparison_value: Option<u32>,
    pub(crate) register_comparison_value: Option<u32>,
    inverted: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredSemanticEvidence {
    pub(crate) kind: String,
    pub(crate) root: String,
    pub(crate) operation: String,
    pub(crate) action_target: String,
    pub(crate) action_origin: String,
    action_site: Option<u32>,
    action_site_path: Vec<Option<u32>>,
    action_path: String,
    pub(crate) predicate_function: String,
    producer: Option<String>,
    producer_path: Vec<String>,
    scope_index: usize,
    scope_alternatives: usize,
    path_index: usize,
    pub(crate) path_expression: String,
    path_guards: usize,
    guard_index: usize,
    pub(crate) residual_path_expression: String,
    site: u32,
    pub(crate) condition: String,
    taken: bool,
    pub(crate) effective_operation: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredSemanticBoundary {
    operation: String,
    call_shapes: usize,
    functions: Vec<String>,
    targets: Vec<String>,
    replacement_hints: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredTrampolineSlot {
    trampoline: StoredTrampoline,
    arguments: Vec<StoredCallArgument>,
    call_shapes: usize,
    functions: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredDiagnostic {
    pub(crate) root_id: String,
    pub(crate) kind: String,
    pub(crate) site: Option<u32>,
    pub(crate) rendered: String,
    original_fragments: usize,
    fragments: Vec<StoredDiagnosticFragment>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredDiagnosticFragment {
    first_ordinal: usize,
    occurrences: usize,
    message: String,
}

#[cfg(test)]
mod instruction_effect_tests {
    use super::*;

    #[test]
    fn instruction_effect_round_trip_preserves_origin_site_and_block() {
        let effect = StoredInstructionEffect::Mmio {
            site: 0x4008_1234,
            block: Some(7),
            access: "write".to_owned(),
            width: 32,
            address: 0x6000_8020,
            register: "WIFI_CTRL".to_owned(),
            mode: "direct".to_owned(),
            paths: vec!["entry -> enabled".to_owned()],
            guards: vec!["arg0 != 0".to_owned()],
            value: Some("arg1 | 0x4".to_owned()),
            modified_mask: Some(0x4),
            preserved_mask: Some(!0x4),
            forced_zero_mask: Some(0),
            forced_one_mask: Some(0x4),
        };

        let encoded = serde_json::to_value(&effect).unwrap();
        let decoded: StoredInstructionEffect = serde_json::from_value(encoded.clone()).unwrap();

        assert_eq!(decoded.site(), 0x4008_1234);
        assert_eq!(decoded.block(), Some(7));
        assert_eq!(serde_json::to_value(decoded).unwrap(), encoded);
    }
}
