//! Complete owned DTO for linked-IR schema v66.

#![allow(
    dead_code,
    reason = "complete stored DTOs enforce every persistent schema field"
)]

use serde::{Deserialize, Serialize};

fn deserialize_required_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

mod mmio;
pub(crate) use mmio::*;

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
    pub(crate) inventories: Vec<StoredSourceInputArtifact>,
    pub(crate) companions: Vec<StoredArtifactIdentity>,
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
pub(crate) struct StoredSourceInputArtifact {
    pub(crate) source: String,
    pub(crate) artifact: StoredArtifactIdentity,
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
    body_complete: usize,
    call_targets_complete: usize,
    transitive_effects_complete: usize,
    executable_complete: usize,
    structured: usize,
    loop_functions: usize,
    loop_regions: usize,
    counted_loop_candidates: usize,
    irreducible_loop_regions: usize,
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
    pub(crate) aliases: Vec<String>,
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
    pub(crate) artifact_sha256: String,
    pub(crate) identity: String,
    pub(crate) selection: String,
    pub(crate) member: Option<String>,
    pub(crate) symbol: String,
    binding: String,
    pub(crate) address: Option<u32>,
    pub(crate) object_offset: u32,
    size: usize,
    flow_kind: String,
    pub(crate) loops: Vec<StoredFunctionLoop>,
    pub(crate) completeness: StoredFunctionCompleteness,
    exact: bool,
    return_value: String,
    return_provenance: StoredReturnProvenance,
    #[serde(deserialize_with = "deserialize_required_option")]
    return_frontier: Option<StoredGuardedReturnFrontier>,
    call_result_frontiers: Vec<StoredCallResultFrontier>,
    dependencies: Vec<String>,
    pub(crate) projected_relocations: Vec<StoredProjectedRelocation>,
    pub(crate) local_value_flow: Vec<StoredLocalValueFlow>,
    pub(crate) indexed_dispatches: Vec<StoredIndexedDispatch>,
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
pub(crate) struct StoredIndexedDispatch {
    pub(crate) table: String,
    pub(crate) table_address: Option<u32>,
    pub(crate) site: u32,
    pub(crate) stride: u8,
    pub(crate) entries: Vec<StoredIndexedDispatchEntry>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredIndexedDispatchEntry {
    pub(crate) selector: u32,
    pub(crate) case_target: String,
    pub(crate) case_address: u32,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredFunctionLoop {
    pub(crate) id: usize,
    pub(crate) kind: String,
    pub(crate) header_block: Option<usize>,
    pub(crate) latch_blocks: Vec<usize>,
    pub(crate) body_blocks: Vec<usize>,
    pub(crate) exit_blocks: Vec<usize>,
    pub(crate) parent: Option<usize>,
    pub(crate) depth: usize,
    pub(crate) counted: Option<StoredCountedLoop>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredCountedLoop {
    pub(crate) induction_register: String,
    pub(crate) initial: u32,
    pub(crate) step: i32,
    pub(crate) bound: u32,
    pub(crate) comparison: String,
    pub(crate) trip_count: u32,
    pub(crate) execution_proof: bool,
}

pub(crate) fn validate_function_loops(
    identity: &str,
    loops: &[StoredFunctionLoop],
) -> crate::Result<()> {
    fn sorted_unique(values: &[usize]) -> bool {
        values.windows(2).all(|pair| pair[0] < pair[1])
    }

    for (index, region) in loops.iter().enumerate() {
        if region.id != index
            || region.body_blocks.is_empty()
            || !sorted_unique(&region.latch_blocks)
            || !sorted_unique(&region.body_blocks)
            || !sorted_unique(&region.exit_blocks)
        {
            return Err(crate::Error::invalid(format!(
                "linked-IR function {identity:?} has a malformed loop region {index}"
            )));
        }
        match region.kind.as_str() {
            "natural"
                if region
                    .header_block
                    .is_some_and(|header| region.body_blocks.contains(&header))
                    && !region.latch_blocks.is_empty()
                    && region
                        .latch_blocks
                        .iter()
                        .all(|latch| region.body_blocks.contains(latch)) => {}
            "irreducible"
                if region.header_block.is_none()
                    && region.latch_blocks.is_empty()
                    && region.counted.is_none() => {}
            "natural" | "irreducible" => {
                return Err(crate::Error::invalid(format!(
                    "linked-IR function {identity:?} has inconsistent {} loop region {index}",
                    region.kind
                )));
            }
            _ => {
                return Err(crate::Error::invalid(format!(
                    "linked-IR function {identity:?} has unknown loop kind {:?}",
                    region.kind
                )));
            }
        }
        if let Some(counted) = &region.counted
            && (counted.execution_proof
                || counted.step == 0
                || counted.trip_count == 0
                || counted.comparison != "not-equal")
        {
            return Err(crate::Error::invalid(format!(
                "linked-IR function {identity:?} has an invalid counted-loop candidate {index}"
            )));
        }

        let mut expected_depth = 0;
        let mut parent = region.parent;
        let mut visited = std::collections::BTreeSet::new();
        while let Some(parent_id) = parent {
            if parent_id >= loops.len()
                || parent_id == index
                || !visited.insert(parent_id)
                || loops[parent_id].body_blocks.len() <= region.body_blocks.len()
                || !region
                    .body_blocks
                    .iter()
                    .all(|block| loops[parent_id].body_blocks.contains(block))
            {
                return Err(crate::Error::invalid(format!(
                    "linked-IR function {identity:?} has an invalid loop parent for region {index}"
                )));
            }
            expected_depth += 1;
            parent = loops[parent_id].parent;
        }
        if region.depth != expected_depth {
            return Err(crate::Error::invalid(format!(
                "linked-IR function {identity:?} has depth {} for loop region {index}, expected {expected_depth}",
                region.depth
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod loop_validation_tests {
    use super::*;

    fn counted_region() -> StoredFunctionLoop {
        StoredFunctionLoop {
            id: 0,
            kind: "natural".to_owned(),
            header_block: Some(1),
            latch_blocks: vec![2],
            body_blocks: vec![1, 2],
            exit_blocks: vec![3],
            parent: None,
            depth: 0,
            counted: Some(StoredCountedLoop {
                induction_register: "s4".to_owned(),
                initial: 0,
                step: 1,
                bound: 10,
                comparison: "not-equal".to_owned(),
                trip_count: 10,
                execution_proof: false,
            }),
        }
    }

    #[test]
    fn persistent_loop_candidate_cannot_claim_execution_proof() {
        let valid = counted_region();
        validate_function_loops("source::function", &[valid]).unwrap();

        let mut invalid = counted_region();
        invalid.counted.as_mut().unwrap().execution_proof = true;
        assert!(
            validate_function_loops("source::function", &[invalid])
                .unwrap_err()
                .to_string()
                .contains("invalid counted-loop candidate")
        );
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredFunctionCompleteness {
    pub(crate) body_complete: bool,
    pub(crate) call_targets_complete: bool,
    pub(crate) transitive_effects_complete: bool,
    pub(crate) executable_complete: bool,
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
    pub(crate) artifact_sha256: String,
    pub(crate) identity: String,
    pub(crate) selection: String,
    pub(crate) member: Option<String>,
    pub(crate) symbol: String,
    pub(crate) binding: String,
    pub(crate) loops: Vec<StoredFunctionLoop>,
    pub(crate) completeness: StoredFunctionCompleteness,
    pub(crate) dependencies: Vec<String>,
    pub(crate) direct_calls: usize,
    pub(crate) calls: Vec<StoredReviewCall>,
    pub(crate) mmio: Vec<StoredReviewMmio>,
    pub(crate) mmio_addresses: Vec<u32>,
    pub(crate) direct_context_fields: usize,
    pub(crate) direct_memory_fields: usize,
    pub(crate) direct_effects: Vec<StoredReviewDirectEffect>,
    pub(crate) diagnostics: Vec<StoredReviewDiagnostic>,
    pub(crate) effect_summary: StoredReviewEffectSummary,
    pub(crate) decode_blockers: Vec<StoredDecodeBlocker>,
}

/// Canonical direct observable effect used to close a feature review surface.
///
/// `site` is evidence for navigation, but feature fingerprints intentionally
/// omit it: relinking may move an otherwise identical transaction.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredReviewDirectEffect {
    pub(crate) kind: String,
    pub(crate) site: Option<u32>,
    pub(crate) operation: String,
    pub(crate) target: String,
    pub(crate) width: Option<u8>,
    pub(crate) value: Option<String>,
    pub(crate) modified_mask: Option<u32>,
    pub(crate) preserved_mask: Option<u32>,
    pub(crate) forced_zero_mask: Option<u32>,
    pub(crate) forced_one_mask: Option<u32>,
    pub(crate) arguments: Vec<String>,
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
    pub(crate) direct: bool,
    pub(crate) project_symbol: Option<String>,
    pub(crate) semantic_operation: Option<String>,
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

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredReturnProvenance {
    exact: bool,
    known_zero_bits: u32,
    known_one_bits: u32,
    unknown_bits: u32,
    sources: Vec<StoredReturnBitSource>,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
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

impl StoredReturnProvenance {
    fn has_epoch_sensitive_source(&self) -> bool {
        self.sources.iter().any(|source| {
            matches!(
                source.kind.as_str(),
                "memory-read" | "mmio-read" | "indexed-mmio-read"
            )
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredReturnGuard {
    pub(crate) site: u32,
    pub(crate) operation: String,
    pub(crate) taken: bool,
    pub(crate) left: String,
    pub(crate) right: String,
    pub(crate) left_exact: bool,
    pub(crate) right_exact: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredReturnGuardPath {
    pub(crate) guards: Vec<StoredReturnGuard>,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredGuardedReturnLeaf {
    pub(crate) value: String,
    pub(crate) exact: bool,
    epoch_sensitive_dependency: bool,
    provenance: StoredReturnProvenance,
    pub(crate) guard_path: StoredReturnGuardPath,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredGuardedFailStop {
    pub(crate) site: u32,
    pub(crate) function: String,
    pub(crate) guard_path: StoredReturnGuardPath,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredGuardedReturnFrontier {
    pub(crate) structurally_complete: bool,
    pub(crate) leaves: Vec<StoredGuardedReturnLeaf>,
    pub(crate) fail_stops: Vec<StoredGuardedFailStop>,
    pub(crate) blockers: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredCallResultFrontier {
    producer: StoredCallResultProvenance,
    pub(crate) frontier: StoredGuardedReturnFrontier,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GuardedReturnClassification {
    Exact,
    Conditional,
    NoMatch,
    Incomplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct GuardedReturnMatch {
    pub(crate) classification: GuardedReturnClassification,
    /// The value structurally depends on a RAM or volatile MMIO observation.
    /// Its site-local token is not an object-lifetime or cross-call epoch proof.
    pub(crate) epoch_sensitive_dependency: bool,
}

impl StoredGuardedReturnFrontier {
    pub(crate) fn classify_call_argument(
        &self,
        call: &StoredCall,
        position: usize,
    ) -> GuardedReturnMatch {
        let epoch_sensitive_dependency = self.leaves.iter().any(|leaf| {
            leaf.epoch_sensitive_dependency || leaf.provenance.has_epoch_sensitive_source()
        });
        let Some(expected) = call.arguments.get(position) else {
            return GuardedReturnMatch {
                classification: GuardedReturnClassification::Incomplete,
                epoch_sensitive_dependency,
            };
        };
        if !call.argument_is_exact(position)
            || !self.structurally_complete
            || !self.blockers.is_empty()
            || self.leaves.is_empty()
            || self.leaves.iter().any(|leaf| !leaf.exact)
        {
            return GuardedReturnMatch {
                classification: GuardedReturnClassification::Incomplete,
                epoch_sensitive_dependency,
            };
        }
        let matches = self
            .leaves
            .iter()
            .filter(|leaf| leaf.value == expected.as_str())
            .count();
        let classification = if matches == self.leaves.len() {
            if epoch_sensitive_dependency {
                // A canonical value match at two sites does not prove that a
                // mutable object was observed in the same lifetime/epoch.
                GuardedReturnClassification::Incomplete
            } else {
                GuardedReturnClassification::Exact
            }
        } else if matches == 0 {
            GuardedReturnClassification::NoMatch
        } else {
            GuardedReturnClassification::Conditional
        };
        GuardedReturnMatch {
            classification,
            epoch_sensitive_dependency,
        }
    }
}

impl StoredFunction {
    pub(crate) fn return_frontier(&self) -> Option<&StoredGuardedReturnFrontier> {
        self.return_frontier.as_ref()
    }

    pub(crate) fn call_result_frontier(
        &self,
        producer: (&str, &str, u32, &str, Option<&str>),
    ) -> Option<&StoredGuardedReturnFrontier> {
        self.call_result_frontiers
            .iter()
            .find(|candidate| {
                candidate.producer.kind == producer.0
                    && candidate.producer.function == producer.1
                    && candidate.producer.site == producer.2
                    && candidate.producer.target == producer.3
                    && candidate.producer.operation.as_deref() == producer.4
            })
            .map(|candidate| &candidate.frontier)
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredCall {
    pub(crate) kind: String,
    pub(crate) target: String,
    pub(crate) site: Option<u32>,
    direct: bool,
    tail: bool,
    result_modeled: bool,
    result_provenance: Option<StoredCallResultProvenance>,
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
    argument_exact: Vec<bool>,
    argument_result_provenance: Vec<StoredCallArgumentResultProvenance>,
    argument_bindings: Vec<StoredArgumentBinding>,
    typed_arguments: Vec<StoredCallArgument>,
    pub(crate) guard_paths: Option<Vec<StoredGuardPath>>,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredCallResultProvenance {
    kind: String,
    function: String,
    site: u32,
    target: String,
    operation: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredCallArgumentResultProvenance {
    position: usize,
    producer: StoredCallResultProvenance,
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
    byte_offset: u16,
    width: u8,
}

impl StoredCall {
    /// Whether selecting this stored target identifies one authenticated call
    /// target rather than one candidate from an indirect/indexed dispatch.
    pub(crate) fn publication_selector_exact(&self) -> bool {
        self.direct
            && !matches!(
                self.kind.as_str(),
                "unresolved"
                    | "ambiguous-project"
                    | "indexed-dispatch"
                    | "indexed-dispatch-unresolved"
            )
    }

    pub(crate) const fn direct(&self) -> bool {
        self.direct
    }

    pub(crate) const fn result_modeled(&self) -> bool {
        self.result_modeled
    }

    pub(crate) fn result_provenance(&self) -> Option<(&str, &str, u32, &str, Option<&str>)> {
        self.result_provenance.as_ref().map(|provenance| {
            (
                provenance.kind.as_str(),
                provenance.function.as_str(),
                provenance.site,
                provenance.target.as_str(),
                provenance.operation.as_deref(),
            )
        })
    }

    pub(crate) fn argument_result_provenance(
        &self,
        position: usize,
    ) -> Option<(&str, &str, u32, &str, Option<&str>)> {
        self.argument_result_provenance
            .iter()
            .find(|provenance| provenance.position == position)
            .map(|provenance| {
                (
                    provenance.producer.kind.as_str(),
                    provenance.producer.function.as_str(),
                    provenance.producer.site,
                    provenance.producer.target.as_str(),
                    provenance.producer.operation.as_deref(),
                )
            })
    }

    pub(crate) fn argument_is_result_of(&self, position: usize, producer: &Self) -> bool {
        producer.result_modeled
            && producer.result_provenance.as_ref().is_some_and(|expected| {
                self.argument_result_provenance.iter().any(|provenance| {
                    provenance.position == position && provenance.producer == *expected
                })
            })
    }

    pub(crate) fn argument_is_exact(&self, position: usize) -> bool {
        self.argument_exact.get(position).copied() == Some(true)
    }

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
                usize::from(output.pointer_argument) == pointer_argument
                    && output.byte_offset == 0
                    && output.width == width
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

pub(crate) fn validate_call_arguments(identity: &str, calls: &[StoredCall]) -> crate::Result<()> {
    let producers = calls
        .iter()
        .filter_map(|call| call.result_provenance.clone())
        .fold(
            std::collections::BTreeMap::new(),
            |mut output, provenance| {
                *output.entry(provenance).or_insert(0_usize) += 1;
                output
            },
        );
    for call in calls {
        if call.argument_exact.len() != call.arguments.len() {
            return Err(crate::Error::invalid(format!(
                "linked-IR function {identity:?} call to {:?} has {} arguments but {} exactness records",
                call.target,
                call.arguments.len(),
                call.argument_exact.len()
            )));
        }
        if let Some(provenance) = &call.result_provenance
            && (!call.result_modeled
                || !matches!(provenance.kind.as_str(), "call-result" | "external-result")
                || provenance.function != identity
                || call.site != Some(provenance.site)
                || provenance.target != call.target
                || provenance.operation != call.semantic_operation)
        {
            return Err(crate::Error::invalid(format!(
                "linked-IR function {identity:?} call to {:?} has invalid modeled-result provenance",
                call.target
            )));
        }
        let mut previous = None;
        for provenance in &call.argument_result_provenance {
            if provenance.position >= call.arguments.len()
                || previous.is_some_and(|position| position >= provenance.position)
                || !matches!(
                    provenance.producer.kind.as_str(),
                    "call-result" | "external-result"
                )
                || producers.get(&provenance.producer).copied() != Some(1)
            {
                return Err(crate::Error::invalid(format!(
                    "linked-IR function {identity:?} call to {:?} has invalid argument-result provenance at position {}",
                    call.target, provenance.position
                )));
            }
            previous = Some(provenance.position);
        }
    }
    Ok(())
}

fn validate_guard_path(identity: &str, path: &StoredReturnGuardPath) -> crate::Result<()> {
    if path.guards.len() > 32 {
        return Err(crate::Error::invalid(format!(
            "linked-IR function {identity:?} exceeds the guarded-return predicate limit"
        )));
    }
    for guard in &path.guards {
        if !matches!(
            guard.operation.as_str(),
            "equal"
                | "not-equal"
                | "less-signed"
                | "greater-equal-signed"
                | "less-unsigned"
                | "greater-equal-unsigned"
        ) {
            return Err(crate::Error::invalid(format!(
                "linked-IR function {identity:?} has an unknown guarded-return operation {:?}",
                guard.operation
            )));
        }
    }
    Ok(())
}

fn validate_return_frontier(
    identity: &str,
    frontier: &StoredGuardedReturnFrontier,
    producer_result: bool,
) -> crate::Result<()> {
    const MAX_TERMINALS: usize = 64;
    let leaves_are_ordered = frontier.leaves.windows(2).all(|pair| pair[0] < pair[1]);
    let fail_stops_are_ordered = frontier.fail_stops.windows(2).all(|pair| pair[0] < pair[1]);
    let blockers_are_ordered = frontier.blockers.windows(2).all(|pair| pair[0] < pair[1]);
    if !leaves_are_ordered || !fail_stops_are_ordered || !blockers_are_ordered {
        return Err(crate::Error::invalid(format!(
            "linked-IR function {identity:?} has unsorted or duplicate guarded-return records"
        )));
    }
    let terminal_count = frontier.leaves.len() + frontier.fail_stops.len();
    if (frontier.structurally_complete && terminal_count == 0) || terminal_count > MAX_TERMINALS {
        return Err(crate::Error::invalid(format!(
            "linked-IR function {identity:?} has an empty or over-limit guarded-return frontier"
        )));
    }
    if producer_result && frontier.leaves.is_empty() {
        return Err(crate::Error::invalid(format!(
            "linked-IR function {identity:?} attaches a guarded-return frontier with no returning leaf to a call result"
        )));
    }
    let mut terminal_paths = std::collections::BTreeSet::new();
    for leaf in &frontier.leaves {
        if leaf.provenance.exact != (leaf.provenance.unknown_bits == 0)
            || (leaf.provenance.has_epoch_sensitive_source() && !leaf.epoch_sensitive_dependency)
        {
            return Err(crate::Error::invalid(format!(
                "linked-IR function {identity:?} has contradictory guarded-return leaf exactness"
            )));
        }
        validate_return_provenance(identity, &leaf.provenance)?;
        validate_guard_path(identity, &leaf.guard_path)?;
        if !terminal_paths.insert(leaf.guard_path.clone()) {
            return Err(crate::Error::invalid(format!(
                "linked-IR function {identity:?} attaches conflicting terminals to one guarded-return path"
            )));
        }
    }
    for fail_stop in &frontier.fail_stops {
        validate_guard_path(identity, &fail_stop.guard_path)?;
        if !terminal_paths.insert(fail_stop.guard_path.clone()) {
            return Err(crate::Error::invalid(format!(
                "linked-IR function {identity:?} attaches conflicting terminals to one guarded-return path"
            )));
        }
    }
    if frontier.structurally_complete != frontier.blockers.is_empty() {
        return Err(crate::Error::invalid(format!(
            "linked-IR function {identity:?} has contradictory guarded-return structural completeness"
        )));
    }
    Ok(())
}

fn validate_return_provenance(
    identity: &str,
    provenance: &StoredReturnProvenance,
) -> crate::Result<()> {
    let fixed_bits = provenance.known_zero_bits | provenance.known_one_bits;
    if provenance.known_zero_bits & provenance.known_one_bits != 0
        || fixed_bits & provenance.unknown_bits != 0
    {
        return Err(crate::Error::invalid(format!(
            "linked-IR function {identity:?} has overlapping return-provenance bit classes"
        )));
    }
    let mut covered = fixed_bits | provenance.unknown_bits;
    let mut previous_end = 0_u8;
    for source in &provenance.sources {
        let Some(end) = source.output_lsb.checked_add(source.width) else {
            return Err(crate::Error::invalid(format!(
                "linked-IR function {identity:?} has an invalid return-provenance source range"
            )));
        };
        let Some(source_end) = source.source_lsb.checked_add(source.width) else {
            return Err(crate::Error::invalid(format!(
                "linked-IR function {identity:?} has an invalid return-provenance source range"
            )));
        };
        if source.width == 0
            || end > 32
            || source_end > 32
            || source.output_lsb < previous_end
            || source.output_bits != bit_range_mask(source.output_lsb, source.width)
            || source.source_bits != bit_range_mask(source.source_lsb, source.width)
            || covered & source.output_bits != 0
        {
            return Err(crate::Error::invalid(format!(
                "linked-IR function {identity:?} has contradictory return-provenance source bits"
            )));
        }
        let fields_valid = match source.kind.as_str() {
            "argument" => {
                source.argument.is_some()
                    && source.token.is_none()
                    && source.target.is_none()
                    && source.address.is_none()
                    && source.register.is_none()
            }
            "mmio-read" => {
                source.argument.is_none()
                    && source.token.is_some()
                    && source.target.is_none()
                    && source.address.is_some()
                    && source.register.is_some()
            }
            "indexed-mmio-read" | "memory-read" | "private-stack-read" => {
                source.argument.is_none()
                    && source.token.is_some()
                    && source.target.is_none()
                    && source.address.is_none()
                    && source.register.is_none()
            }
            "call-result" | "external-result" | "external-result-high" => {
                source.argument.is_none()
                    && source.token.is_some()
                    && source.address.is_none()
                    && source.register.is_none()
            }
            "external-output" => {
                source.argument.is_some()
                    && source.token.is_some()
                    && source.address.is_none()
                    && source.register.is_none()
            }
            _ => false,
        };
        if !fields_valid {
            return Err(crate::Error::invalid(format!(
                "linked-IR function {identity:?} has an invalid return-provenance source"
            )));
        }
        covered |= source.output_bits;
        previous_end = end;
    }
    if covered != u32::MAX {
        return Err(crate::Error::invalid(format!(
            "linked-IR function {identity:?} has incomplete return-provenance bit coverage"
        )));
    }
    Ok(())
}

const fn bit_range_mask(lsb: u8, width: u8) -> u32 {
    if width == 32 {
        u32::MAX
    } else {
        ((1_u32 << width) - 1) << lsb
    }
}

pub(crate) fn validate_return_frontiers(function: &StoredFunction) -> crate::Result<()> {
    if let Some(frontier) = &function.return_frontier {
        validate_return_frontier(&function.identity, frontier, false)?;
    }
    validate_call_result_frontiers(
        &function.identity,
        &function.calls,
        &function.call_result_frontiers,
    )
}

fn validate_call_result_frontiers(
    identity: &str,
    calls: &[StoredCall],
    frontiers: &[StoredCallResultFrontier],
) -> crate::Result<()> {
    let producers = calls
        .iter()
        .filter_map(|call| call.result_provenance.as_ref())
        .collect::<std::collections::BTreeSet<_>>();
    let mut previous = None;
    for result in frontiers {
        if previous
            .as_ref()
            .is_some_and(|value| value >= &result.producer)
            || !producers.contains(&result.producer)
        {
            return Err(crate::Error::invalid(format!(
                "linked-IR function {identity:?} has duplicate or orphaned call-result frontier"
            )));
        }
        validate_return_frontier(identity, &result.frontier, true)?;
        previous = Some(result.producer.clone());
    }
    if producers.iter().any(|producer| {
        producer.kind == "call-result"
            && !frontiers.iter().any(|result| &result.producer == *producer)
    }) {
        return Err(crate::Error::invalid(format!(
            "linked-IR function {identity:?} has an internal call-result provenance without its guarded-return frontier"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod call_provenance_tests {
    use super::*;

    fn modeled_producer() -> StoredCall {
        StoredCall {
            kind: "modeled-direct-external".to_owned(),
            target: "receive".to_owned(),
            site: Some(0x120),
            direct: true,
            tail: false,
            result_modeled: true,
            result_provenance: Some(StoredCallResultProvenance {
                kind: "external-result".to_owned(),
                function: "fixture::worker".to_owned(),
                site: 0x120,
                target: "receive".to_owned(),
                operation: Some("queue.receive".to_owned()),
            }),
            execution_model: None,
            semantics: None,
            semantic_operation: Some("queue.receive".to_owned()),
            semantic_contract: None,
            replacement_hint: None,
            project_symbol: None,
            project_candidates: Vec::new(),
            trampoline: None,
            argument_shapes: 1,
            arguments: Vec::new(),
            argument_exact: Vec::new(),
            argument_result_provenance: Vec::new(),
            argument_bindings: Vec::new(),
            typed_arguments: Vec::new(),
            guard_paths: None,
        }
    }

    fn consumer(producer: StoredCallResultProvenance) -> StoredCall {
        StoredCall {
            kind: "modeled-direct-external".to_owned(),
            target: "run".to_owned(),
            site: Some(0x180),
            direct: true,
            tail: false,
            result_modeled: false,
            result_provenance: None,
            execution_model: None,
            semantics: None,
            semantic_operation: Some("event.run".to_owned()),
            semantic_contract: None,
            replacement_hint: None,
            project_symbol: None,
            project_candidates: Vec::new(),
            trampoline: None,
            argument_shapes: 146,
            arguments: vec!["varies-across-146-shapes".to_owned()],
            argument_exact: vec![false],
            argument_result_provenance: vec![StoredCallArgumentResultProvenance {
                position: 0,
                producer,
            }],
            argument_bindings: Vec::new(),
            typed_arguments: Vec::new(),
            guard_paths: None,
        }
    }

    #[test]
    fn modeled_result_provenance_must_describe_its_containing_call() {
        let valid = modeled_producer();
        assert!(validate_call_arguments("fixture::worker", &[valid]).is_ok());

        let mut wrong_site = modeled_producer();
        wrong_site
            .result_provenance
            .as_mut()
            .expect("fixture provenance")
            .site = 0x124;
        assert!(validate_call_arguments("fixture::worker", &[wrong_site]).is_err());
    }

    #[test]
    fn consumer_matches_only_the_complete_modeled_producer_identity() {
        let producer = modeled_producer();
        let matching = consumer(
            producer
                .result_provenance
                .clone()
                .expect("fixture provenance"),
        );
        assert!(matching.argument_is_result_of(0, &producer));
        assert!(!matching.argument_is_result_of(1, &producer));

        let mut wrong_identity = producer
            .result_provenance
            .clone()
            .expect("fixture provenance");
        wrong_identity.operation = Some("different.receive".to_owned());
        assert!(!consumer(wrong_identity).argument_is_result_of(0, &producer));

        let mut unmodeled = modeled_producer();
        unmodeled.result_modeled = false;
        assert!(!matching.argument_is_result_of(0, &unmodeled));
    }

    fn constant_provenance() -> StoredReturnProvenance {
        StoredReturnProvenance {
            exact: true,
            known_zero_bits: u32::MAX,
            known_one_bits: 0,
            unknown_bits: 0,
            sources: Vec::new(),
        }
    }

    fn exact_frontier(values: &[&str]) -> StoredGuardedReturnFrontier {
        StoredGuardedReturnFrontier {
            structurally_complete: true,
            leaves: values
                .iter()
                .map(|value| StoredGuardedReturnLeaf {
                    value: (*value).to_owned(),
                    exact: true,
                    epoch_sensitive_dependency: false,
                    provenance: constant_provenance(),
                    guard_path: StoredReturnGuardPath { guards: Vec::new() },
                })
                .collect(),
            fail_stops: Vec::new(),
            blockers: Vec::new(),
        }
    }

    fn exact_argument(value: &str, exact: bool) -> StoredCall {
        let mut call = modeled_producer();
        call.arguments = vec![value.to_owned()];
        call.argument_exact = vec![exact];
        call
    }

    #[test]
    fn classifier_distinguishes_exact_conditional_no_match_and_incomplete() {
        let exact = exact_frontier(&["queue-a"]);
        assert_eq!(
            exact
                .classify_call_argument(&exact_argument("queue-a", true), 0)
                .classification,
            GuardedReturnClassification::Exact
        );
        let conditional = exact_frontier(&["queue-a", "queue-b"]);
        assert_eq!(
            conditional
                .classify_call_argument(&exact_argument("queue-a", true), 0)
                .classification,
            GuardedReturnClassification::Conditional
        );
        assert_eq!(
            conditional
                .classify_call_argument(&exact_argument("queue-c", true), 0)
                .classification,
            GuardedReturnClassification::NoMatch
        );
        assert_eq!(
            exact
                .classify_call_argument(&exact_argument("queue-a", false), 0)
                .classification,
            GuardedReturnClassification::Incomplete
        );
        assert_eq!(
            exact_frontier(&[])
                .classify_call_argument(&exact_argument("queue-a", true), 0)
                .classification,
            GuardedReturnClassification::Incomplete
        );
        let mut unresolved_leaf = exact_frontier(&["unknown"]);
        unresolved_leaf.leaves[0].exact = false;
        assert_eq!(
            unresolved_leaf
                .classify_call_argument(&exact_argument("unknown", true), 0)
                .classification,
            GuardedReturnClassification::Incomplete
        );
    }

    #[test]
    fn epoch_sensitive_reads_are_typed_evidence_and_fail_closed() {
        let mut frontier = exact_frontier(&["queue-a", "queue-b"]);
        frontier.leaves[0]
            .provenance
            .sources
            .push(StoredReturnBitSource {
                kind: "memory-read".to_owned(),
                output_lsb: 0,
                source_lsb: 0,
                width: 32,
                output_bits: u32::MAX,
                source_bits: u32::MAX,
                inverted: false,
                argument: None,
                token: Some(7),
                target: None,
                address: None,
                register: None,
            });
        frontier.leaves[0].provenance.known_zero_bits = 0;
        frontier.leaves[0].epoch_sensitive_dependency = true;
        let classification = frontier.classify_call_argument(&exact_argument("queue-a", true), 0);
        assert_eq!(
            classification.classification,
            GuardedReturnClassification::Conditional
        );
        assert!(classification.epoch_sensitive_dependency);

        let mut all_matching = exact_frontier(&["queue-a"]);
        all_matching.leaves[0]
            .provenance
            .sources
            .push(frontier.leaves[0].provenance.sources[0].clone());
        all_matching.leaves[0].provenance.known_zero_bits = 0;
        all_matching.leaves[0].epoch_sensitive_dependency = true;
        let classification =
            all_matching.classify_call_argument(&exact_argument("queue-a", true), 0);
        assert_eq!(
            classification.classification,
            GuardedReturnClassification::Incomplete
        );
        assert!(classification.epoch_sensitive_dependency);
    }

    #[test]
    fn volatile_mmio_and_unknown_ram_require_an_object_epoch_witness() {
        for kind in ["mmio-read", "indexed-mmio-read", "memory-read"] {
            let mut frontier = exact_frontier(&["queue-a"]);
            frontier.leaves[0]
                .provenance
                .sources
                .push(StoredReturnBitSource {
                    kind: kind.to_owned(),
                    output_lsb: 0,
                    source_lsb: 0,
                    width: 32,
                    output_bits: u32::MAX,
                    source_bits: u32::MAX,
                    inverted: false,
                    argument: None,
                    token: Some(9),
                    target: None,
                    address: (kind == "mmio-read").then_some(0x6000_1000),
                    register: (kind == "mmio-read").then(|| "QUEUE.SELECT".to_owned()),
                });
            frontier.leaves[0].provenance.known_zero_bits = 0;
            if kind == "memory-read" {
                frontier.leaves[0].epoch_sensitive_dependency = true;
            }

            let result = frontier.classify_call_argument(&exact_argument("queue-a", true), 0);
            assert_eq!(
                result.classification,
                GuardedReturnClassification::Incomplete,
                "{kind} must not establish a cross-site queue epoch"
            );
            assert!(
                result.epoch_sensitive_dependency,
                "missing {kind} dependency"
            );
        }
    }

    #[test]
    fn frontier_validation_rejects_contradictions_and_duplicate_producers() {
        let mut contradictory = exact_frontier(&["queue-a"]);
        contradictory.leaves[0].provenance.exact = false;
        assert!(validate_return_frontier("fixture::worker", &contradictory, true).is_err());

        let producer = modeled_producer();
        let provenance = producer
            .result_provenance
            .clone()
            .expect("fixture provenance");
        let result = StoredCallResultFrontier {
            producer: provenance,
            frontier: exact_frontier(&["queue-a"]),
        };
        assert!(
            validate_call_result_frontiers(
                "fixture::worker",
                &[producer],
                &[result.clone(), result]
            )
            .is_err()
        );
    }

    #[test]
    fn frontier_validation_rejects_impossible_structural_and_provenance_states() {
        let mut empty = exact_frontier(&[]);
        assert!(validate_return_frontier("fixture::worker", &empty, false).is_err());

        let mut blocked_empty = empty.clone();
        blocked_empty.structurally_complete = false;
        blocked_empty.blockers = vec!["structured flow unavailable".to_owned()];
        assert!(validate_return_frontier("fixture::worker", &blocked_empty, false).is_ok());

        empty.leaves.push(StoredGuardedReturnLeaf {
            value: "queue-a".to_owned(),
            exact: true,
            epoch_sensitive_dependency: false,
            provenance: constant_provenance(),
            guard_path: StoredReturnGuardPath { guards: Vec::new() },
        });
        empty.structurally_complete = false;
        assert!(validate_return_frontier("fixture::worker", &empty, false).is_err());

        let mut conflicting = exact_frontier(&["queue-a", "queue-b"]);
        assert!(validate_return_frontier("fixture::worker", &conflicting, false).is_err());
        conflicting.leaves.truncate(1);
        conflicting.leaves[0].guard_path.guards = (0..33)
            .map(|site| StoredReturnGuard {
                site,
                operation: "equal".to_owned(),
                taken: true,
                left: "arg0".to_owned(),
                right: "const:0x00000000".to_owned(),
                left_exact: true,
                right_exact: true,
            })
            .collect();
        assert!(validate_return_frontier("fixture::worker", &conflicting, false).is_err());

        let mut invalid_source = exact_frontier(&["queue-a"]);
        invalid_source.leaves[0].provenance.known_zero_bits = 0;
        invalid_source.leaves[0]
            .provenance
            .sources
            .push(StoredReturnBitSource {
                kind: "forged-memory".to_owned(),
                output_lsb: 0,
                source_lsb: 0,
                width: 32,
                output_bits: u32::MAX,
                source_bits: u32::MAX,
                inverted: false,
                argument: None,
                token: Some(7),
                target: None,
                address: None,
                register: None,
            });
        assert!(validate_return_frontier("fixture::worker", &invalid_source, false).is_err());

        let mut internal = modeled_producer();
        internal.kind = "internal".to_owned();
        internal
            .result_provenance
            .as_mut()
            .expect("fixture provenance")
            .kind = "call-result".to_owned();
        assert!(validate_call_result_frontiers("fixture::worker", &[internal], &[]).is_err());
    }

    #[test]
    fn optional_frontier_field_requires_explicit_null() {
        #[derive(Deserialize)]
        struct Fixture {
            #[serde(deserialize_with = "deserialize_required_option")]
            frontier: Option<u8>,
        }

        assert!(serde_json::from_str::<Fixture>(r#"{}"#).is_err());
        assert!(
            serde_json::from_str::<Fixture>(r#"{"frontier":null}"#)
                .unwrap()
                .frontier
                .is_none()
        );
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
    body_policy: String,
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
    Allocation {
        call_token: u32,
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
            Self::Allocation { call_token } => format!("allocation#{call_token}"),
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
