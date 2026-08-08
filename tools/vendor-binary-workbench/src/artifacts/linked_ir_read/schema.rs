//! Complete owned DTO for linked-IR schema v35.

#![allow(
    dead_code,
    reason = "complete stored DTOs enforce every persistent schema field"
)]

use serde::Deserialize;

#[derive(Debug, Deserialize)]
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
    pub(crate) mmio_registers: Vec<StoredMmioRegister>,
    semantic_boundaries: Vec<StoredSemanticBoundary>,
    trampoline_slots: Vec<StoredTrampolineSlot>,
    pub(crate) functions: Vec<StoredFunction>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredSourceArtifact {
    pub(crate) source: String,
    pub(crate) artifact: StoredArtifactIdentity,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredArtifactIdentity {
    pub(crate) path: String,
    pub(crate) sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredReportSummary {
    artifacts: usize,
    pub(crate) functions: usize,
    root_functions: usize,
    included_reachable_functions: usize,
    exported: usize,
    local: usize,
    pub(crate) mmio_registers: usize,
    mmio_functions: usize,
    mmio_access_shapes: usize,
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
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredFunction {
    pub(crate) source: String,
    pub(crate) identity: String,
    pub(crate) selection: String,
    pub(crate) member: Option<String>,
    pub(crate) symbol: String,
    binding: String,
    address: Option<u32>,
    pub(crate) object_offset: u32,
    size: usize,
    flow_kind: String,
    pub(crate) complete: bool,
    exact: bool,
    return_value: String,
    return_provenance: StoredReturnProvenance,
    dependencies: Vec<String>,
    pub(crate) calls: Vec<StoredCall>,
    direct_mmio_predicates: Vec<StoredDirectMmioPredicate>,
    pub(crate) mmio_accesses: Vec<StoredMmioAccess>,
    delays: Vec<StoredDelay>,
    context_accesses: Vec<StoredContextAccess>,
    context_fields: Vec<StoredFunctionContextField>,
    memory_accesses: Vec<StoredMemoryAccess>,
    memory_fields: Vec<StoredFunctionMemoryField>,
    pub(crate) scenario_suggestions: Vec<StoredScenarioSuggestion>,
    pub(crate) effect_summary: StoredEffectSummary,
    call_graph_diagnostics: Vec<StoredDiagnostic>,
    direct_diagnostics: Vec<StoredDiagnostic>,
    reference_diagnostics: Vec<StoredDiagnostic>,
    call_graph_blockers: Vec<String>,
    direct_blockers: Vec<String>,
    reference_blockers: Vec<String>,
    pub(crate) pseudo: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredReturnProvenance {
    exact: bool,
    known_zero_bits: u32,
    known_one_bits: u32,
    unknown_bits: u32,
    sources: Vec<StoredReturnBitSource>,
}

#[derive(Debug, Deserialize)]
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredCall {
    pub(crate) kind: String,
    pub(crate) target: String,
    pub(crate) site: Option<u32>,
    tail: bool,
    result_modeled: bool,
    semantics: Option<String>,
    pub(crate) semantic_operation: Option<String>,
    semantic_contract: Option<StoredSemanticContract>,
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredCallArgument {
    position: usize,
    name: String,
    c_type: String,
    direction: String,
    value: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredArgumentBinding {
    position: usize,
    caller_argument: u8,
    offset: i32,
    expression: String,
}

#[derive(Debug, Deserialize)]
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredSemanticContract {
    source: String,
    id: String,
    evidence: String,
    event_dispatch: Option<StoredEventDispatchContract>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredEventDispatchContract {
    mechanism: String,
    execution_context: String,
    receiver: Option<String>,
    argument_roles: Vec<StoredEventDispatchArgumentRole>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredEventDispatchArgumentRole {
    role: String,
    argument: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredGuardPath {
    pub(crate) guards: Vec<StoredGuard>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredGuard {
    site: u32,
    pub(crate) condition: String,
    operation: String,
    pub(crate) taken: bool,
    result_sources: Vec<StoredGuardResultSource>,
    direct_mmio_sources: Vec<StoredDirectMmioPredicateSource>,
}

#[derive(Debug, Deserialize)]
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

#[derive(Debug, Deserialize)]
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

#[derive(Debug, Deserialize)]
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredDirectMmioPredicate {
    site: u32,
    condition: String,
    operation: String,
    sources: Vec<StoredDirectMmioPredicateSource>,
}

#[derive(Debug, Deserialize)]
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredDelay {
    ordinal: usize,
    path: String,
    micros: String,
    constant_micros: Option<u32>,
}

#[derive(Debug, Deserialize)]
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

#[derive(Debug, Deserialize)]
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

#[derive(Debug, Deserialize)]
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

#[derive(Debug, Deserialize)]
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

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) enum StoredMemoryObject {
    Argument {
        index: u8,
    },
    Global {
        member: Option<String>,
        symbol: String,
    },
    DereferencedGlobal {
        member: Option<String>,
        symbol: String,
        pointer_offset: i64,
    },
    Absolute {
        address_space: String,
        address: u32,
    },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredEffectSummary {
    pub(crate) call_graph_closed: bool,
    max_depth: usize,
    pub(crate) reachable_functions: Vec<String>,
    recursive_functions: Vec<String>,
    blockers: Vec<String>,
    mmio_registers: Vec<StoredSummaryMmio>,
    delays: Vec<StoredSummaryDelay>,
    pub(crate) semantic_operations: Vec<StoredSemanticOperation>,
    pub(crate) context_projection_complete: bool,
    pub(crate) context_projection_blockers: Vec<String>,
    pub(crate) context_fields: Vec<StoredContextField>,
    pub(crate) memory_fields: Vec<StoredMemoryField>,
    pub(crate) trampoline_calls: Vec<StoredProjectedTrampolineCall>,
    semantic_actions: Vec<StoredProjectedSemanticAction>,
    pub(crate) event_dispatches: Vec<StoredEventDispatch>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredSummaryMmio {
    address: u32,
    width: u8,
    access_shapes: usize,
    accesses: Vec<String>,
    modes: Vec<String>,
    origins: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredSummaryDelay {
    micros: String,
    constant_micros: Option<u32>,
    delay_shapes: usize,
    origins: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredSemanticOperation {
    pub(crate) operation: String,
    call_shapes: usize,
    targets: Vec<String>,
    replacement_hints: Vec<String>,
    origins: Vec<String>,
}

#[derive(Debug, Deserialize)]
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredMemoryField {
    pub(crate) object: StoredMemoryObject,
    pub(crate) offset: i64,
    pub(crate) width: u8,
    pub(crate) reads: usize,
    pub(crate) writes: usize,
    pub(crate) write_mask: u32,
    origins: Vec<String>,
    paths: Vec<String>,
    write_values: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredProjectedCallArgument {
    position: usize,
    name: String,
    c_type: String,
    direction: String,
    value: String,
    binding: String,
    root_argument: Option<u8>,
    root_offset: Option<i32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredProjectedTrampolineCall {
    trampoline: StoredTrampoline,
    origin: String,
    path: String,
    argument_shapes: usize,
    arguments: Vec<StoredProjectedCallArgument>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredProjectedSemanticAction {
    site_path: Vec<Option<u32>>,
    operation: String,
    target: String,
    contract: Option<StoredSemanticContract>,
    replacement_hint: Option<String>,
    origin: String,
    path: String,
    site: Option<u32>,
    argument_shapes: usize,
    arguments: Vec<StoredProjectedCallArgument>,
    guard_scopes: Option<Vec<StoredGuardScope>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredGuardScope {
    function: String,
    paths: Vec<StoredGuardPath>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredEventDispatch {
    semantic_action_index: usize,
    mechanism: String,
    execution_context: String,
    receiver: Option<String>,
    interface_complete: bool,
    blockers: Vec<String>,
    bindings: Vec<StoredEventDispatchBinding>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredEventDispatchBinding {
    role: String,
    argument: StoredProjectedCallArgument,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredScenarioSuggestion {
    pub(crate) kind: String,
    pub(crate) site: Option<u32>,
    pub(crate) evidence: String,
    pub(crate) variants: Vec<StoredScenarioVariant>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredScenarioVariant {
    pub(crate) name: String,
    pub(crate) arguments: Vec<StoredScenarioArgument>,
    pub(crate) mmio_reads: Vec<StoredScenarioMmioRead>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredScenarioArgument {
    pub(crate) index: u8,
    pub(crate) value: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredScenarioMmioRead {
    pub(crate) address: u32,
    pub(crate) mask: u32,
    pub(crate) expected: u32,
    pub(crate) values: Vec<u32>,
}

#[derive(Debug, Deserialize)]
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredMmioBitRange {
    least_significant_bit: u8,
    most_significant_bit: u8,
    mask: u32,
    write_shapes: usize,
    functions: Vec<String>,
}

#[derive(Debug, Deserialize)]
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

#[derive(Debug, Deserialize)]
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

#[derive(Debug, Deserialize)]
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredSemanticBoundary {
    operation: String,
    call_shapes: usize,
    functions: Vec<String>,
    targets: Vec<String>,
    replacement_hints: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredTrampolineSlot {
    trampoline: StoredTrampoline,
    arguments: Vec<StoredCallArgument>,
    call_shapes: usize,
    functions: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredDiagnostic {
    rendered: String,
    original_fragments: usize,
    fragments: Vec<StoredDiagnosticFragment>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredDiagnosticFragment {
    first_ordinal: usize,
    occurrences: usize,
    message: String,
}
