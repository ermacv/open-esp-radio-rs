//! Best-effort linked function/call IR for manual vendor-code analysis.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fmt::Write as _,
};

use crate::{
    BitSource, BranchCondition, BranchOperation, DraftReferenceEvent, DraftReferenceFlow,
    DraftReferenceTerminator, ExpressionOperation, ExternalReturnModel, FunctionAnalysis,
    MemoryAccess, MmioRegisterMap, ObservableEvent, ReferenceResolver, SymbolicValue, artifact,
    direct,
};

const MAX_CALL_GRAPH_STATES: usize = 127;
const MAX_CALL_GRAPH_BRANCH_DECISIONS: usize = 12;
const MAX_CONTEXT_PROJECTION_STATES: usize = 4_096;
const LINKED_CONTEXT_ARGUMENTS: u8 = 16;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct LinkedCallArgument {
    pub(crate) position: usize,
    pub(crate) name: String,
    pub(crate) c_type: String,
    pub(crate) direction: &'static str,
    pub(crate) value: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct LinkedArgumentBinding {
    pub(crate) position: usize,
    pub(crate) caller_argument: u8,
    pub(crate) offset: i32,
    pub(crate) expression: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct LinkedTrampoline {
    pub(crate) table: String,
    pub(crate) pointer_symbol: String,
    pub(crate) backing_symbol: String,
    pub(crate) version: u32,
    pub(crate) magic: u32,
    pub(crate) table_size: u32,
    pub(crate) magic_offset: u32,
    pub(crate) function_id: String,
    pub(crate) slot: u32,
    pub(crate) c_name: String,
    pub(crate) argument_count: u8,
    pub(crate) return_model: String,
    pub(crate) operation: String,
    pub(crate) return_type: String,
    pub(crate) replacement_hint: Option<String>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct LinkedEventDispatchArgumentRole {
    pub(crate) role: &'static str,
    pub(crate) argument: &'static str,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct LinkedEventDispatchContract {
    pub(crate) mechanism: &'static str,
    pub(crate) execution_context: &'static str,
    pub(crate) receiver: Option<&'static str>,
    pub(crate) argument_roles: Vec<LinkedEventDispatchArgumentRole>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct LinkedSemanticContract {
    pub(crate) source: &'static str,
    pub(crate) id: String,
    pub(crate) evidence: String,
    pub(crate) event_dispatch: Option<LinkedEventDispatchContract>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct LinkedReturnBitSource {
    pub(crate) kind: &'static str,
    pub(crate) output_lsb: u8,
    pub(crate) source_lsb: u8,
    pub(crate) width: u8,
    pub(crate) output_bits: u32,
    pub(crate) source_bits: u32,
    pub(crate) inverted: bool,
    pub(crate) argument: Option<u8>,
    pub(crate) token: Option<u32>,
    pub(crate) target: Option<String>,
    pub(crate) address: Option<u32>,
    pub(crate) register: Option<String>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct LinkedReturnProvenance {
    pub(crate) exact: bool,
    pub(crate) known_zero_bits: u32,
    pub(crate) known_one_bits: u32,
    pub(crate) unknown_bits: u32,
    pub(crate) sources: Vec<LinkedReturnBitSource>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct LinkedCallGuardMmioSource {
    pub(crate) address: u32,
    pub(crate) register: String,
    pub(crate) producer_path: Vec<String>,
    pub(crate) result_bits: u32,
    pub(crate) register_bits: u32,
    pub(crate) inverted: bool,
    pub(crate) result_comparison_value: Option<u32>,
    pub(crate) register_comparison_value: Option<u32>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct LinkedCallGuardResultSource {
    pub(crate) kind: &'static str,
    pub(crate) token: u32,
    pub(crate) target: Option<String>,
    pub(crate) operand: &'static str,
    pub(crate) value_bits: Option<u32>,
    pub(crate) source_bits: u32,
    pub(crate) inverted: bool,
    pub(crate) comparison_value: Option<u32>,
    pub(crate) source_comparison_value: Option<u32>,
    pub(crate) producer_return_exact: Option<bool>,
    pub(crate) mmio_sources: Vec<LinkedCallGuardMmioSource>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct LinkedDirectMmioPredicateSource {
    pub(crate) operand: &'static str,
    pub(crate) read_token: u32,
    pub(crate) address: u32,
    pub(crate) register: String,
    pub(crate) value_bits: u32,
    pub(crate) register_bits: u32,
    pub(crate) inverted: bool,
    pub(crate) comparison_value: Option<u32>,
    pub(crate) register_comparison_value: Option<u32>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct LinkedDirectMmioPredicate {
    pub(crate) site: u32,
    pub(crate) condition: String,
    pub(crate) operation: &'static str,
    pub(crate) sources: Vec<LinkedDirectMmioPredicateSource>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct LinkedCallGuard {
    pub(crate) site: u32,
    pub(crate) condition: String,
    pub(crate) operation: &'static str,
    pub(crate) taken: bool,
    pub(crate) result_sources: Vec<LinkedCallGuardResultSource>,
    pub(crate) direct_mmio_sources: Vec<LinkedDirectMmioPredicateSource>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct LinkedCallGuardPath {
    pub(crate) guards: Vec<LinkedCallGuard>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct LinkedCallGuardScope {
    pub(crate) function: String,
    pub(crate) paths: Vec<LinkedCallGuardPath>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct LinkedCall {
    pub(crate) kind: &'static str,
    pub(crate) target: String,
    pub(crate) site: Option<u32>,
    pub(crate) tail: bool,
    pub(crate) result_modeled: bool,
    pub(crate) semantics: Option<String>,
    pub(crate) semantic_operation: Option<String>,
    pub(crate) semantic_contract: Option<LinkedSemanticContract>,
    pub(crate) replacement_hint: Option<String>,
    pub(crate) project_symbol: Option<String>,
    pub(crate) project_candidates: Vec<String>,
    pub(crate) trampoline: Option<LinkedTrampoline>,
    pub(crate) argument_shapes: usize,
    pub(crate) arguments: Vec<String>,
    pub(crate) argument_bindings: Vec<LinkedArgumentBinding>,
    pub(crate) typed_arguments: Vec<LinkedCallArgument>,
    pub(crate) guard_paths: Option<Vec<LinkedCallGuardPath>>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct ContextAccess {
    pub(crate) argument: u8,
    pub(crate) offset: i32,
    pub(crate) access: &'static str,
    pub(crate) width: u8,
    pub(crate) path: String,
    pub(crate) value: Option<String>,
    pub(crate) value_pseudo: Option<String>,
    pub(crate) write_mask: Option<u32>,
    pub(crate) preserved_mask: Option<u32>,
    pub(crate) forced_zero_mask: Option<u32>,
    pub(crate) forced_one_mask: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ContextField {
    pub(crate) argument: u8,
    pub(crate) offset: i32,
    pub(crate) width: u8,
    pub(crate) reads: usize,
    pub(crate) writes: usize,
    pub(crate) write_mask: u32,
    pub(crate) paths: Vec<String>,
    pub(crate) write_values: Vec<String>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct LinkedMmioAccess {
    pub(crate) ordinal: usize,
    pub(crate) address: u32,
    pub(crate) width: u8,
    pub(crate) register: String,
    pub(crate) access: &'static str,
    pub(crate) mode: &'static str,
    pub(crate) path: String,
    pub(crate) address_expression: Option<String>,
    pub(crate) guard: Option<String>,
    pub(crate) predicate_mask: Option<u32>,
    pub(crate) predicate_expected: Option<u32>,
    pub(crate) value: Option<String>,
    pub(crate) modified_mask: Option<u32>,
    pub(crate) preserved_mask: Option<u32>,
    pub(crate) inverted_mask: Option<u32>,
    pub(crate) forced_zero_mask: Option<u32>,
    pub(crate) forced_one_mask: Option<u32>,
    pub(crate) read_derived_mask: Option<u32>,
    pub(crate) dynamic_mask: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LinkedMmioBitRange {
    pub(crate) least_significant_bit: u8,
    pub(crate) most_significant_bit: u8,
    pub(crate) mask: u32,
    pub(crate) write_shapes: usize,
    pub(crate) functions: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LinkedMmioFieldCandidate {
    pub(crate) least_significant_bit: u8,
    pub(crate) most_significant_bit: u8,
    pub(crate) mask: u32,
    pub(crate) write_shapes: usize,
    pub(crate) predicate_shapes: usize,
    pub(crate) poll_shapes: usize,
    pub(crate) functions: Vec<String>,
    pub(crate) access_functions: Vec<String>,
    pub(crate) predicate_functions: Vec<String>,
    pub(crate) predicate_evidence: Vec<LinkedMmioFieldPredicateEvidence>,
    pub(crate) semantic_operations: Vec<String>,
    pub(crate) semantic_roots: Vec<String>,
    pub(crate) semantic_evidence: Vec<LinkedMmioFieldSemanticEvidence>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct LinkedMmioFieldPredicateEvidence {
    pub(crate) kind: &'static str,
    pub(crate) function: String,
    pub(crate) producer: Option<String>,
    pub(crate) producer_path: Vec<String>,
    pub(crate) site: Option<u32>,
    pub(crate) path: Option<String>,
    pub(crate) condition: String,
    pub(crate) operation: &'static str,
    pub(crate) taken: Option<bool>,
    pub(crate) effective_operation: Option<&'static str>,
    pub(crate) operand: Option<&'static str>,
    pub(crate) comparison_value: Option<u32>,
    pub(crate) register_comparison_value: Option<u32>,
    pub(crate) inverted: bool,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct LinkedMmioFieldSemanticEvidence {
    pub(crate) kind: &'static str,
    pub(crate) root: String,
    pub(crate) operation: String,
    pub(crate) action_target: String,
    pub(crate) action_origin: String,
    pub(crate) action_site: Option<u32>,
    pub(crate) action_site_path: Vec<Option<u32>>,
    pub(crate) action_path: String,
    pub(crate) predicate_function: String,
    pub(crate) producer: Option<String>,
    pub(crate) producer_path: Vec<String>,
    pub(crate) scope_index: usize,
    pub(crate) scope_alternatives: usize,
    pub(crate) path_index: usize,
    pub(crate) path_expression: String,
    pub(crate) path_guards: usize,
    pub(crate) guard_index: usize,
    pub(crate) residual_path_expression: String,
    pub(crate) site: u32,
    pub(crate) condition: String,
    pub(crate) taken: bool,
    pub(crate) effective_operation: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LinkedMmioRegister {
    pub(crate) address: u32,
    pub(crate) width: u8,
    pub(crate) names: Vec<String>,
    pub(crate) read_shapes: usize,
    pub(crate) write_shapes: usize,
    pub(crate) poll_shapes: usize,
    pub(crate) predicate_shapes: usize,
    pub(crate) static_shapes: usize,
    pub(crate) indexed_candidate_shapes: usize,
    pub(crate) whole_register_write_shapes: usize,
    pub(crate) whole_register_predicate_shapes: usize,
    pub(crate) whole_register_poll_shapes: usize,
    pub(crate) read_modify_write_shapes: usize,
    pub(crate) write_masks: Vec<u32>,
    pub(crate) predicate_masks: Vec<u32>,
    pub(crate) poll_masks: Vec<u32>,
    pub(crate) candidate_bit_ranges: Vec<LinkedMmioBitRange>,
    pub(crate) field_candidates: Vec<LinkedMmioFieldCandidate>,
    pub(crate) functions: Vec<String>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct LinkedDelay {
    pub(crate) ordinal: usize,
    pub(crate) path: String,
    pub(crate) micros: String,
    pub(crate) constant_micros: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LinkedSummaryMmio {
    pub(crate) address: u32,
    pub(crate) width: u8,
    pub(crate) access_shapes: usize,
    pub(crate) accesses: Vec<&'static str>,
    pub(crate) modes: Vec<&'static str>,
    pub(crate) origins: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LinkedSummaryDelay {
    pub(crate) micros: String,
    pub(crate) constant_micros: Option<u32>,
    pub(crate) delay_shapes: usize,
    pub(crate) origins: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LinkedSummarySemantic {
    pub(crate) operation: String,
    pub(crate) call_shapes: usize,
    pub(crate) targets: Vec<String>,
    pub(crate) replacement_hints: Vec<String>,
    pub(crate) origins: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LinkedSummaryContextField {
    pub(crate) argument: u8,
    pub(crate) offset: i32,
    pub(crate) width: u8,
    pub(crate) reads: usize,
    pub(crate) writes: usize,
    pub(crate) write_mask: u32,
    pub(crate) origins: Vec<String>,
    pub(crate) paths: Vec<String>,
    pub(crate) write_values: Vec<String>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct LinkedProjectedCallArgument {
    pub(crate) position: usize,
    pub(crate) name: String,
    pub(crate) c_type: String,
    pub(crate) direction: &'static str,
    pub(crate) value: String,
    pub(crate) binding: &'static str,
    pub(crate) root_argument: Option<u8>,
    pub(crate) root_offset: Option<i32>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct LinkedProjectedTrampolineCall {
    pub(crate) trampoline: LinkedTrampoline,
    pub(crate) origin: String,
    pub(crate) path: String,
    pub(crate) argument_shapes: usize,
    pub(crate) arguments: Vec<LinkedProjectedCallArgument>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct LinkedProjectedSemanticAction {
    pub(crate) site_path: Vec<Option<u32>>,
    pub(crate) operation: String,
    pub(crate) target: String,
    pub(crate) contract: Option<LinkedSemanticContract>,
    pub(crate) replacement_hint: Option<String>,
    pub(crate) origin: String,
    pub(crate) path: String,
    pub(crate) site: Option<u32>,
    pub(crate) argument_shapes: usize,
    pub(crate) arguments: Vec<LinkedProjectedCallArgument>,
    pub(crate) guard_scopes: Option<Vec<LinkedCallGuardScope>>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct LinkedEventDispatchBinding {
    pub(crate) role: &'static str,
    pub(crate) argument: LinkedProjectedCallArgument,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LinkedEventDispatch {
    pub(crate) semantic_action_index: usize,
    pub(crate) mechanism: &'static str,
    pub(crate) execution_context: &'static str,
    pub(crate) receiver: Option<String>,
    pub(crate) interface_complete: bool,
    pub(crate) blockers: Vec<String>,
    pub(crate) bindings: Vec<LinkedEventDispatchBinding>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LinkedTrampolineSlot {
    pub(crate) trampoline: LinkedTrampoline,
    pub(crate) arguments: Vec<LinkedCallArgument>,
    pub(crate) call_shapes: usize,
    pub(crate) functions: Vec<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct LinkedEffectSummary {
    pub(crate) call_graph_closed: bool,
    pub(crate) max_depth: usize,
    pub(crate) reachable_functions: Vec<String>,
    pub(crate) recursive_functions: Vec<String>,
    pub(crate) blockers: Vec<String>,
    pub(crate) mmio_registers: Vec<LinkedSummaryMmio>,
    pub(crate) delays: Vec<LinkedSummaryDelay>,
    pub(crate) semantic_operations: Vec<LinkedSummarySemantic>,
    pub(crate) context_projection_complete: bool,
    pub(crate) context_projection_blockers: Vec<String>,
    pub(crate) context_fields: Vec<LinkedSummaryContextField>,
    pub(crate) trampoline_calls: Vec<LinkedProjectedTrampolineCall>,
    pub(crate) semantic_actions: Vec<LinkedProjectedSemanticAction>,
    pub(crate) event_dispatches: Vec<LinkedEventDispatch>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SemanticBoundary {
    pub(crate) operation: String,
    pub(crate) call_shapes: usize,
    pub(crate) functions: Vec<String>,
    pub(crate) targets: Vec<String>,
    pub(crate) replacement_hints: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LinkedDiagnosticFragment {
    pub(crate) first_ordinal: usize,
    pub(crate) occurrences: usize,
    pub(crate) message: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LinkedDiagnostic {
    pub(crate) rendered: String,
    pub(crate) original_fragments: usize,
    pub(crate) fragments: Vec<LinkedDiagnosticFragment>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LinkedIrFunction {
    pub(crate) source: String,
    pub(crate) identity: String,
    pub(crate) selection: &'static str,
    pub(crate) member: Option<String>,
    pub(crate) symbol: String,
    pub(crate) binding: &'static str,
    pub(crate) address: Option<u32>,
    pub(crate) object_offset: u32,
    pub(crate) size: usize,
    pub(crate) flow_kind: &'static str,
    pub(crate) complete: bool,
    pub(crate) exact: bool,
    pub(crate) return_value: String,
    pub(crate) return_provenance: LinkedReturnProvenance,
    pub(crate) dependencies: Vec<String>,
    pub(crate) calls: Vec<LinkedCall>,
    pub(crate) direct_mmio_predicates: Vec<LinkedDirectMmioPredicate>,
    pub(crate) mmio_accesses: Vec<LinkedMmioAccess>,
    pub(crate) delays: Vec<LinkedDelay>,
    pub(crate) context_accesses: Vec<ContextAccess>,
    pub(crate) context_fields: Vec<ContextField>,
    pub(crate) effect_summary: LinkedEffectSummary,
    pub(crate) call_graph_diagnostics: Vec<LinkedDiagnostic>,
    pub(crate) direct_diagnostics: Vec<LinkedDiagnostic>,
    pub(crate) reference_diagnostics: Vec<LinkedDiagnostic>,
    pub(crate) call_graph_blockers: Vec<String>,
    pub(crate) direct_blockers: Vec<String>,
    pub(crate) reference_blockers: Vec<String>,
    pub(crate) pseudo: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LinkedIrReport {
    pub(crate) functions: Vec<LinkedIrFunction>,
    pub(crate) mmio_registers: Vec<LinkedMmioRegister>,
    pub(crate) mmio_functions: usize,
    pub(crate) mmio_access_shapes: usize,
    pub(crate) delay_functions: usize,
    pub(crate) delay_shapes: usize,
    pub(crate) semantic_boundaries: Vec<SemanticBoundary>,
    pub(crate) semantic_calls: usize,
    pub(crate) trampoline_slots: Vec<LinkedTrampolineSlot>,
    pub(crate) trampoline_calls: usize,
    pub(crate) exported_functions: usize,
    pub(crate) local_functions: usize,
    pub(crate) context_functions: usize,
    pub(crate) context_accesses: usize,
    pub(crate) context_fields: usize,
    pub(crate) complete_functions: usize,
    pub(crate) structured_functions: usize,
    pub(crate) internal_calls: usize,
    pub(crate) external_calls: usize,
    pub(crate) call_argument_shapes: usize,
    pub(crate) project_linked_calls: usize,
    pub(crate) ambiguous_project_calls: usize,
    pub(crate) unresolved_calls: usize,
    pub(crate) closed_effect_summaries: usize,
    pub(crate) recursive_effect_summaries: usize,
    pub(crate) complete_context_projections: usize,
    pub(crate) projected_context_fields: usize,
}

fn identity(member: Option<&str>, symbol: &str) -> String {
    member.map_or_else(|| symbol.to_owned(), |member| format!("{member}:{symbol}"))
}

fn compact_diagnostic(message: &str) -> LinkedDiagnostic {
    let mut fragment_indices = BTreeMap::<&str, usize>::new();
    let mut fragments = Vec::<LinkedDiagnosticFragment>::new();
    let mut original_fragments = 0;

    for (ordinal, fragment) in message.split("; ").enumerate() {
        original_fragments += 1;
        if let Some(index) = fragment_indices.get(fragment).copied() {
            fragments[index].occurrences += 1;
        } else {
            fragment_indices.insert(fragment, fragments.len());
            fragments.push(LinkedDiagnosticFragment {
                first_ordinal: ordinal,
                occurrences: 1,
                message: fragment.to_owned(),
            });
        }
    }

    let rendered = fragments
        .iter()
        .map(|fragment| {
            if fragment.occurrences == 1 {
                fragment.message.clone()
            } else {
                format!(
                    "{} [repeated {} times]",
                    fragment.message, fragment.occurrences
                )
            }
        })
        .collect::<Vec<_>>()
        .join("; ");

    LinkedDiagnostic {
        rendered,
        original_fragments,
        fragments,
    }
}

fn compact_diagnostics(messages: &[String]) -> Vec<LinkedDiagnostic> {
    messages
        .iter()
        .map(|message| compact_diagnostic(message))
        .collect()
}

fn rendered_diagnostics(diagnostics: &[LinkedDiagnostic]) -> Vec<String> {
    diagnostics
        .iter()
        .map(|diagnostic| diagnostic.rendered.clone())
        .collect()
}

type SymbolKey = (Option<String>, String, u64);

fn symbol_key(symbol: &artifact::ArtifactSymbolDefinition) -> SymbolKey {
    (symbol.member.clone(), symbol.name.clone(), symbol.address)
}

struct IrIdentityCatalog {
    symbols: BTreeMap<SymbolKey, String>,
    targets: BTreeMap<u32, String>,
    selectable_symbols: BTreeMap<String, artifact::ArtifactSymbolDefinition>,
}

impl IrIdentityCatalog {
    fn new(resolver: &ReferenceResolver, namespace: Option<&str>) -> Self {
        let mut definitions = resolver.symbols.clone();
        definitions.extend(resolver.symbols_by_address.values().cloned());
        definitions.sort_by_key(symbol_key);
        definitions.dedup_by_key(|symbol| symbol_key(symbol));

        let mut base_counts = BTreeMap::<(Option<String>, String), usize>::new();
        for symbol in &definitions {
            *base_counts
                .entry((symbol.member.clone(), symbol.name.clone()))
                .or_default() += 1;
        }
        let symbols = definitions
            .iter()
            .map(|symbol| {
                let base = identity(symbol.member.as_deref(), &symbol.name);
                let duplicate = base_counts
                    .get(&(symbol.member.clone(), symbol.name.clone()))
                    .copied()
                    .unwrap_or_default()
                    > 1;
                let value = if duplicate {
                    format!("{base}@{:#010x}", symbol.address as u32)
                } else {
                    base
                };
                let value = namespace.map_or(value.clone(), |source| format!("{source}::{value}"));
                (symbol_key(symbol), value)
            })
            .collect::<BTreeMap<_, _>>();
        let targets = resolver
            .symbols_by_address
            .iter()
            .map(|(target, symbol)| {
                (
                    *target,
                    symbols
                        .get(&symbol_key(symbol))
                        .expect("target symbol is present in IR identity catalog")
                        .clone(),
                )
            })
            .collect();
        let selectable_symbols = resolver
            .symbols
            .iter()
            .map(|symbol| {
                (
                    symbols
                        .get(&symbol_key(symbol))
                        .expect("primary symbol is present in IR identity catalog")
                        .clone(),
                    symbol.clone(),
                )
            })
            .collect();
        Self {
            symbols,
            targets,
            selectable_symbols,
        }
    }

    fn symbol(&self, symbol: &artifact::ArtifactSymbolDefinition) -> String {
        self.symbols
            .get(&symbol_key(symbol))
            .expect("IR symbol is present in identity catalog")
            .clone()
    }

    fn target(&self, target: u32) -> String {
        self.targets
            .get(&target)
            .cloned()
            .unwrap_or_else(|| format!("sub_{target:08x}"))
    }

    fn selectable_symbol(&self, identity: &str) -> Option<&artifact::ArtifactSymbolDefinition> {
        self.selectable_symbols.get(identity)
    }
}

fn pseudo_identifier(value: &str) -> String {
    let mut output = String::new();
    for character in value.chars() {
        if character.is_ascii_alphanumeric() || character == '_' {
            output.push(character);
        } else {
            output.push('_');
        }
    }
    if output.is_empty() {
        "unnamed".to_owned()
    } else if output.as_bytes()[0].is_ascii_digit() {
        format!("fn_{output}")
    } else {
        output
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum PseudoBitBase {
    Input(u8),
    Register { token: u32, address: u32 },
    IndexedRegister(u32),
    Memory(u32),
    PrivateStack(u32),
    CallResult(u32),
    ExternalResult(u32),
}

impl PseudoBitBase {
    fn render(&self) -> String {
        match self {
            Self::Input(index) => format!("arg{index}"),
            Self::Register { token, .. } => format!("read{token}"),
            Self::IndexedRegister(token) => format!("indexed_read{token}"),
            Self::Memory(token) => format!("ramread{token}"),
            Self::PrivateStack(token) => format!("private_stack_read{token}"),
            Self::CallResult(token) => format!("call{token}"),
            Self::ExternalResult(token) => format!("external{token}"),
        }
    }
}

fn pseudo_bit_source(source: &BitSource) -> Option<(PseudoBitBase, u8, bool)> {
    match source {
        BitSource::Input {
            index,
            bit,
            inverted,
        } => Some((PseudoBitBase::Input(*index), *bit, *inverted)),
        BitSource::Register {
            read_token,
            address,
            bit,
            inverted,
        } => Some((
            PseudoBitBase::Register {
                token: *read_token,
                address: *address,
            },
            *bit,
            *inverted,
        )),
        BitSource::IndexedRegister {
            read_token,
            bit,
            inverted,
        } => Some((PseudoBitBase::IndexedRegister(*read_token), *bit, *inverted)),
        BitSource::Memory {
            read_token,
            bit,
            inverted,
        } => Some((PseudoBitBase::Memory(*read_token), *bit, *inverted)),
        BitSource::PrivateStack {
            read_token,
            bit,
            inverted,
        } => Some((PseudoBitBase::PrivateStack(*read_token), *bit, *inverted)),
        BitSource::CallResult {
            call_token,
            bit,
            inverted,
        } => Some((PseudoBitBase::CallResult(*call_token), *bit, *inverted)),
        BitSource::ExternalResult {
            call_token,
            bit,
            inverted,
        } => Some((PseudoBitBase::ExternalResult(*call_token), *bit, *inverted)),
        BitSource::Unknown | BitSource::Constant(_) => None,
    }
}

fn pseudo_masked_bits(bits: &[BitSource; 32]) -> Option<String> {
    let mut base = None;
    let mut inverted = None;
    let mut mask = 0_u32;
    for (output_bit, source) in bits.iter().enumerate() {
        if matches!(source, BitSource::Constant(false)) {
            continue;
        }
        let (source_base, source_bit, source_inverted) = pseudo_bit_source(source)?;
        if usize::from(source_bit) != output_bit {
            return None;
        }
        if base.as_ref().is_some_and(|base| base != &source_base)
            || inverted.is_some_and(|inverted| inverted != source_inverted)
        {
            return None;
        }
        base = Some(source_base);
        inverted = Some(source_inverted);
        mask |= 1 << output_bit;
    }
    let base = base?.render();
    match (inverted == Some(true), mask) {
        (false, u32::MAX) => Some(base),
        (true, u32::MAX) => Some(format!("(!{base})")),
        (false, mask) => Some(format!("({base} & {mask:#010x})")),
        (true, mask) => Some(format!("((!{base}) & {mask:#010x})")),
    }
}

fn pseudo_value(value: &SymbolicValue) -> String {
    if let Some(index) = value.direct_input_index() {
        return format!("arg{index}");
    }
    match value {
        SymbolicValue::Unknown => "unknown".to_owned(),
        SymbolicValue::Constant(value) | SymbolicValue::InputConstant { value, .. } => {
            format!("{value:#010x}")
        }
        SymbolicValue::StackAddress(offset) => format!("stack.ptr({offset:+#x})"),
        SymbolicValue::SymbolAddress {
            member,
            symbol,
            hi_addend,
            lo_addend,
            post_offset,
        } => format!(
            "symbol({}::{symbol}, hi={hi_addend:+#x}, lo={}, post={post_offset:+#x})",
            member.as_deref().unwrap_or("linked"),
            lo_addend.map_or_else(|| "?".to_owned(), |value| format!("{value:+#x}"))
        ),
        SymbolicValue::CallResult(token) => format!("call{token}"),
        SymbolicValue::ExternalResult(token) => format!("external{token}"),
        SymbolicValue::Expression {
            operation,
            left,
            right,
        } => {
            let left = pseudo_value(left);
            let right = pseudo_value(right);
            match operation {
                ExpressionOperation::Add => format!("{left}.wrapping_add({right})"),
                ExpressionOperation::Subtract => format!("{left}.wrapping_sub({right})"),
                ExpressionOperation::Multiply => format!("{left}.wrapping_mul({right})"),
                ExpressionOperation::DivideSigned => format!("signed_div({left}, {right})"),
                ExpressionOperation::DivideUnsigned => format!("{left} / {right}"),
                ExpressionOperation::RemainderSigned => format!("signed_rem({left}, {right})"),
                ExpressionOperation::RemainderUnsigned => format!("{left} % {right}"),
                ExpressionOperation::BitAnd => format!("({left} & {right})"),
                ExpressionOperation::BitOr => format!("({left} | {right})"),
                ExpressionOperation::BitXor => format!("({left} ^ {right})"),
                ExpressionOperation::ShiftLeft => format!("({left} << ({right} & 31))"),
                ExpressionOperation::ShiftRight => format!("({left} >> ({right} & 31))"),
                ExpressionOperation::ShiftRightArithmetic => {
                    format!("(({left} as i32) >> ({right} & 31)) as u32")
                }
                ExpressionOperation::Equal => format!("u32::from({left} == {right})"),
                ExpressionOperation::LessThanSigned => {
                    format!("u32::from(({left} as i32) < ({right} as i32))")
                }
                ExpressionOperation::LessThanUnsigned => {
                    format!("u32::from({left} < {right})")
                }
            }
        }
        SymbolicValue::WideSignedDivide {
            dividend_low,
            dividend_high,
            divisor_low,
            divisor_high,
            high_word,
        } => format!(
            "sdiv64_{}({}, {}, {}, {})",
            if *high_word { "high" } else { "low" },
            pseudo_value(dividend_low),
            pseudo_value(dividend_high),
            pseudo_value(divisor_low),
            pseudo_value(divisor_high)
        ),
        SymbolicValue::RegisterImage {
            read_token,
            and_mask,
            or_mask,
            ..
        }
        | SymbolicValue::IndexedRegisterImage {
            read_token,
            and_mask,
            or_mask,
        } => format!("((read{read_token} & {and_mask:#010x}) | {or_mask:#010x})"),
        SymbolicValue::MemoryImage {
            read_token,
            and_mask,
            or_mask,
        } => format!("((ramread{read_token} & {and_mask:#010x}) | {or_mask:#010x})"),
        SymbolicValue::Bits(bits) => {
            pseudo_masked_bits(bits).unwrap_or_else(|| format!("symbolic({:?})", value.canonical()))
        }
        SymbolicValue::ExternalTable(_)
        | SymbolicValue::ExternalFunction { .. }
        | SymbolicValue::FunctionTable(_)
        | SymbolicValue::FunctionPointer { .. } => {
            format!("symbolic({:?})", value.canonical())
        }
    }
}

fn pseudo_arguments(arguments: &[SymbolicValue]) -> String {
    arguments
        .iter()
        .map(pseudo_value)
        .collect::<Vec<_>>()
        .join(", ")
}

fn pseudo_external_arguments(
    function: crate::ExternalFunctionRef,
    arguments: &[SymbolicValue],
) -> String {
    let semantic = function.spec().semantic;
    arguments
        .iter()
        .enumerate()
        .map(|(position, value)| {
            semantic.arguments.get(position).map_or_else(
                || pseudo_value(value),
                |argument| {
                    format!(
                        "{} /* {} {:?} */ = {}",
                        argument.name,
                        argument.c_type,
                        argument.direction,
                        pseudo_value(value)
                    )
                },
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn canonical_arguments(arguments: &[SymbolicValue]) -> Vec<String> {
    arguments.iter().map(SymbolicValue::canonical).collect()
}

fn affine_argument_bindings(arguments: &[SymbolicValue]) -> Vec<LinkedArgumentBinding> {
    arguments
        .iter()
        .enumerate()
        .filter_map(|(position, value)| {
            let (caller_argument, offset) = value.caller_memory_location()?;
            Some(LinkedArgumentBinding {
                position,
                caller_argument,
                offset,
                expression: match offset.cmp(&0) {
                    std::cmp::Ordering::Less => {
                        format!("arg{caller_argument} - {:#x}", offset.unsigned_abs())
                    }
                    std::cmp::Ordering::Equal => format!("arg{caller_argument}"),
                    std::cmp::Ordering::Greater => {
                        format!("arg{caller_argument} + {offset:#x}")
                    }
                },
            })
        })
        .collect()
}

fn external_typed_arguments(
    function: crate::ExternalFunctionRef,
    arguments: &[SymbolicValue],
) -> Vec<LinkedCallArgument> {
    let function = function.spec();
    arguments
        .iter()
        .enumerate()
        .map(|(position, value)| {
            let semantic = function.semantic.arguments.get(position);
            LinkedCallArgument {
                position,
                name: semantic.map_or_else(
                    || format!("arg{position}"),
                    |argument| argument.name.to_owned(),
                ),
                c_type: semantic
                    .map_or_else(|| "u32".to_owned(), |argument| argument.c_type.to_owned()),
                direction: semantic
                    .map_or("unknown", |argument| external_direction(argument.direction)),
                value: value.canonical(),
            }
        })
        .collect()
}

fn direct_semantic_typed_arguments(
    function: &crate::DirectSemanticFunctionSpec,
    arguments: &[String],
) -> Vec<LinkedCallArgument> {
    arguments
        .iter()
        .take(usize::from(function.argument_count))
        .enumerate()
        .map(|(position, value)| {
            let semantic = function.semantic.arguments.get(position);
            LinkedCallArgument {
                position,
                name: semantic.map_or_else(
                    || format!("arg{position}"),
                    |argument| argument.name.to_owned(),
                ),
                c_type: semantic
                    .map_or_else(|| "u32".to_owned(), |argument| argument.c_type.to_owned()),
                direction: semantic
                    .map_or("unknown", |argument| external_direction(argument.direction)),
                value: value.clone(),
            }
        })
        .collect()
}

fn branch_operation(operation: BranchOperation) -> &'static str {
    match operation {
        BranchOperation::Equal => "equal",
        BranchOperation::NotEqual => "not-equal",
        BranchOperation::LessSigned => "less-signed",
        BranchOperation::GreaterEqualSigned => "greater-equal-signed",
        BranchOperation::LessUnsigned => "less-unsigned",
        BranchOperation::GreaterEqualUnsigned => "greater-equal-unsigned",
    }
}

pub(crate) fn effective_branch_operation(operation: &'static str, taken: bool) -> &'static str {
    if taken {
        return operation;
    }
    match operation {
        "equal" => "not-equal",
        "not-equal" => "equal",
        "less-signed" => "greater-equal-signed",
        "greater-equal-signed" => "less-signed",
        "less-unsigned" => "greater-equal-unsigned",
        "greater-equal-unsigned" => "less-unsigned",
        _ => unreachable!("branch operations have a closed vocabulary"),
    }
}

fn format_guard_literal(guard: &LinkedCallGuard) -> String {
    if guard.taken {
        format!("({})", guard.condition)
    } else {
        format!("!({})", guard.condition)
    }
}

pub(crate) fn format_guard_path(path: &LinkedCallGuardPath) -> String {
    let expression = path
        .guards
        .iter()
        .map(format_guard_literal)
        .collect::<Vec<_>>()
        .join(" && ");
    if expression.is_empty() {
        "true".to_owned()
    } else {
        expression
    }
}

pub(crate) fn format_guard_paths(paths: &[LinkedCallGuardPath]) -> String {
    if paths.is_empty() {
        return "false".to_owned();
    }
    paths
        .iter()
        .map(|path| format!("({})", format_guard_path(path)))
        .collect::<Vec<_>>()
        .join(" || ")
}

fn format_guard_path_without(path: &LinkedCallGuardPath, excluded: usize) -> String {
    let expression = path
        .guards
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != excluded)
        .map(|(_, guard)| format_guard_literal(guard))
        .collect::<Vec<_>>()
        .join(" && ");
    if expression.is_empty() {
        "true".to_owned()
    } else {
        expression
    }
}

fn branch_expression(condition: &BranchCondition) -> String {
    let left = pseudo_value(&condition.left);
    let right = pseudo_value(&condition.right);
    match condition.operation {
        BranchOperation::Equal => format!("{left} == {right}"),
        BranchOperation::NotEqual => format!("{left} != {right}"),
        BranchOperation::LessSigned => format!("({left} as i32) < ({right} as i32)"),
        BranchOperation::GreaterEqualSigned => format!("({left} as i32) >= ({right} as i32)"),
        BranchOperation::LessUnsigned => format!("{left} < {right}"),
        BranchOperation::GreaterEqualUnsigned => format!("{left} >= {right}"),
    }
}

fn external_semantics(event: &DraftReferenceEvent) -> Option<String> {
    let DraftReferenceEvent::ExternalCall {
        table, function, ..
    } = event
    else {
        return None;
    };
    let table = table.spec();
    let function = function.spec();
    Some(format!(
        "table={} version={} slot={:#x} args={} return={:?} operation={}",
        table.id,
        table.version,
        function.offset,
        function.argument_count,
        function.return_model,
        function.semantic.operation,
    ))
}

fn external_return_model(model: ExternalReturnModel) -> String {
    match model {
        ExternalReturnModel::Constant(value) => format!("constant:{value:#010x}"),
        ExternalReturnModel::SymbolicU32 => "symbolic-u32".to_owned(),
        ExternalReturnModel::PrivateStackOutputU8 { pointer_argument } => {
            format!("private-stack-output-u8:arg{pointer_argument}")
        }
        ExternalReturnModel::Unmodeled => "unmodeled".to_owned(),
    }
}

fn linked_trampoline(
    table: crate::ExternalTableRef,
    function: crate::ExternalFunctionRef,
) -> LinkedTrampoline {
    let table = table.spec();
    let function = function.spec();
    LinkedTrampoline {
        table: table.id.to_owned(),
        pointer_symbol: table.pointer_symbol.to_owned(),
        backing_symbol: table.backing_symbol.to_owned(),
        version: table.version,
        magic: table.magic,
        table_size: table.size,
        magic_offset: table.magic_offset,
        function_id: function.id.to_owned(),
        slot: function.offset,
        c_name: function.c_name.to_owned(),
        argument_count: function.argument_count,
        return_model: external_return_model(function.return_model),
        operation: function.semantic.operation.to_owned(),
        return_type: function.semantic.return_type.to_owned(),
        replacement_hint: function.semantic.replacement.map(str::to_owned),
    }
}

fn linked_event_dispatch_contract(
    semantic: crate::ExternalSemanticSpec,
) -> Option<LinkedEventDispatchContract> {
    let dispatch = semantic.event_dispatch?;
    Some(LinkedEventDispatchContract {
        mechanism: dispatch.mechanism,
        execution_context: dispatch.execution_context,
        receiver: dispatch.receiver,
        argument_roles: dispatch
            .argument_roles
            .iter()
            .map(|binding| LinkedEventDispatchArgumentRole {
                role: binding.role,
                argument: binding.argument,
            })
            .collect(),
    })
}

fn external_direction(direction: crate::ExternalArgumentDirection) -> &'static str {
    match direction {
        crate::ExternalArgumentDirection::Input => "input",
        crate::ExternalArgumentDirection::Output => "output",
        crate::ExternalArgumentDirection::InputOutput => "input-output",
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct LinkedCallIdentity {
    kind: &'static str,
    target: String,
    site: Option<u32>,
    tail: bool,
    result_modeled: bool,
    semantics: Option<String>,
    semantic_operation: Option<String>,
    semantic_contract: Option<LinkedSemanticContract>,
    replacement_hint: Option<String>,
    project_symbol: Option<String>,
    project_candidates: Vec<String>,
    trampoline: Option<LinkedTrampoline>,
    typed_signature: Vec<(usize, String, String, &'static str)>,
}

impl From<&LinkedCall> for LinkedCallIdentity {
    fn from(call: &LinkedCall) -> Self {
        Self {
            kind: call.kind,
            target: call.target.clone(),
            site: call.site,
            tail: call.tail,
            result_modeled: call.result_modeled,
            semantics: call.semantics.clone(),
            semantic_operation: call.semantic_operation.clone(),
            semantic_contract: call.semantic_contract.clone(),
            replacement_hint: call.replacement_hint.clone(),
            project_symbol: call.project_symbol.clone(),
            project_candidates: call.project_candidates.clone(),
            trampoline: call.trampoline.clone(),
            typed_signature: call
                .typed_arguments
                .iter()
                .map(|argument| {
                    (
                        argument.position,
                        argument.name.clone(),
                        argument.c_type.clone(),
                        argument.direction,
                    )
                })
                .collect(),
        }
    }
}

fn merged_argument_value(calls: &[LinkedCall], position: usize, argument_shapes: usize) -> String {
    if let Some(first) = calls[0].arguments.get(position)
        && calls
            .iter()
            .all(|call| call.arguments.get(position) == Some(first))
    {
        return first.clone();
    }
    format!("varies-across-{argument_shapes}-shapes")
}

fn merged_typed_argument_value(
    calls: &[LinkedCall],
    position: usize,
    argument_shapes: usize,
) -> String {
    fn value(call: &LinkedCall, position: usize) -> Option<&str> {
        call.typed_arguments
            .iter()
            .find(|argument| argument.position == position)
            .map(|argument| argument.value.as_str())
    }

    if let Some(first) = value(&calls[0], position)
        && calls
            .iter()
            .all(|call| value(call, position) == Some(first))
    {
        return first.to_owned();
    }
    format!("varies-across-{argument_shapes}-shapes")
}

fn normalize_guard_paths(
    paths: impl IntoIterator<Item = LinkedCallGuardPath>,
) -> Vec<LinkedCallGuardPath> {
    let mut paths = paths
        .into_iter()
        .map(|mut path| {
            path.guards.sort();
            path.guards.dedup();
            path
        })
        .collect::<BTreeSet<_>>();

    loop {
        let snapshot = paths.iter().cloned().collect::<Vec<_>>();
        let mut consensus = None;
        'pairs: for (index, left) in snapshot.iter().enumerate() {
            for right in &snapshot[index + 1..] {
                if left.guards.len() != right.guards.len() {
                    continue;
                }
                let mut differing = None;
                let mut compatible = true;
                for (guard_index, (left, right)) in
                    left.guards.iter().zip(&right.guards).enumerate()
                {
                    if left.site != right.site
                        || left.condition != right.condition
                        || left.result_sources != right.result_sources
                    {
                        compatible = false;
                        break;
                    }
                    if left.taken != right.taken && differing.replace(guard_index).is_some() {
                        compatible = false;
                        break;
                    }
                }
                if compatible && let Some(differing) = differing {
                    let mut guards = left.guards.clone();
                    guards.remove(differing);
                    consensus = Some(LinkedCallGuardPath { guards });
                    break 'pairs;
                }
            }
        }
        let Some(consensus) = consensus else {
            break;
        };
        let previous_len = paths.len();
        paths.insert(consensus);
        let snapshot = paths.iter().cloned().collect::<Vec<_>>();
        paths.retain(|path| {
            !snapshot.iter().any(|candidate| {
                candidate != path
                    && candidate.guards.len() <= path.guards.len()
                    && candidate
                        .guards
                        .iter()
                        .all(|guard| path.guards.contains(guard))
            })
        });
        if paths.len() == previous_len {
            break;
        }
    }

    let snapshot = paths.iter().cloned().collect::<Vec<_>>();
    paths.retain(|path| {
        !snapshot.iter().any(|candidate| {
            candidate != path
                && candidate.guards.len() <= path.guards.len()
                && candidate
                    .guards
                    .iter()
                    .all(|guard| path.guards.contains(guard))
        })
    });
    paths.into_iter().collect()
}

fn merged_guard_paths(calls: &[LinkedCall]) -> Option<Vec<LinkedCallGuardPath>> {
    let mut paths = Vec::new();
    for call in calls {
        paths.extend(call.guard_paths.as_ref()?.iter().cloned());
    }
    Some(normalize_guard_paths(paths))
}

fn distinct_argument_shape_count(calls: &[LinkedCall]) -> usize {
    let mut shapes = BTreeMap::<
        (
            Vec<String>,
            Vec<LinkedArgumentBinding>,
            Vec<(usize, String)>,
        ),
        usize,
    >::new();
    for call in calls {
        let shape = (
            call.arguments.clone(),
            call.argument_bindings.clone(),
            call.typed_arguments
                .iter()
                .map(|argument| (argument.position, argument.value.clone()))
                .collect(),
        );
        shapes
            .entry(shape)
            .and_modify(|count| *count = (*count).max(call.argument_shapes))
            .or_insert(call.argument_shapes);
    }
    shapes.into_values().sum()
}

fn compact_calls(calls: impl IntoIterator<Item = LinkedCall>) -> Vec<LinkedCall> {
    let mut groups = BTreeMap::<LinkedCallIdentity, Vec<LinkedCall>>::new();
    for call in calls {
        groups
            .entry(LinkedCallIdentity::from(&call))
            .or_default()
            .push(call);
    }

    groups
        .into_values()
        .map(|calls| {
            let argument_shapes = distinct_argument_shape_count(&calls);
            let argument_count = calls
                .iter()
                .map(|call| call.arguments.len())
                .max()
                .unwrap_or_default();
            let arguments = (0..argument_count)
                .map(|position| merged_argument_value(&calls, position, argument_shapes))
                .collect();
            let argument_bindings = calls[0]
                .argument_bindings
                .iter()
                .filter(|binding| {
                    calls[1..]
                        .iter()
                        .all(|call| call.argument_bindings.contains(binding))
                })
                .cloned()
                .collect();
            let mut call = calls[0].clone();
            for argument in &mut call.typed_arguments {
                argument.value =
                    merged_typed_argument_value(&calls, argument.position, argument_shapes);
            }
            call.argument_shapes = argument_shapes;
            call.arguments = arguments;
            call.argument_bindings = argument_bindings;
            call.guard_paths = merged_guard_paths(&calls);
            call
        })
        .collect()
}

fn collect_call_event(
    event: &DraftReferenceEvent,
    resolver: &ReferenceResolver,
    identities: &IrIdentityCatalog,
    calls: &mut BTreeSet<LinkedCall>,
) {
    let call = match event {
        DraftReferenceEvent::ExternalCall {
            table,
            function,
            arguments,
            ..
        } => Some(LinkedCall {
            kind: "external",
            target: format!("{}::{}", table.spec().id, function.spec().c_name),
            site: None,
            tail: false,
            result_modeled: matches!(
                function.spec().return_model,
                ExternalReturnModel::Constant(_) | ExternalReturnModel::SymbolicU32
            ),
            semantics: external_semantics(event),
            semantic_operation: Some(function.spec().semantic.operation.to_owned()),
            semantic_contract: Some(LinkedSemanticContract {
                source: "registered-external-table-slot",
                id: format!("{}::{}", table.spec().id, function.spec().id),
                evidence: "exact-pointer-cell-and-slot".to_owned(),
                event_dispatch: linked_event_dispatch_contract(function.spec().semantic),
            }),
            replacement_hint: function.spec().semantic.replacement.map(str::to_owned),
            project_symbol: None,
            project_candidates: Vec::new(),
            trampoline: Some(linked_trampoline(*table, *function)),
            argument_shapes: 1,
            arguments: canonical_arguments(arguments),
            argument_bindings: affine_argument_bindings(arguments),
            typed_arguments: external_typed_arguments(*function, arguments),
            guard_paths: None,
        }),
        DraftReferenceEvent::DiagnosticCall {
            function,
            arguments,
            ..
        } => Some(LinkedCall {
            kind: "diagnostic",
            target: function.clone(),
            site: None,
            tail: false,
            result_modeled: false,
            semantics: Some("diagnostic/logging boundary".to_owned()),
            semantic_operation: Some("diagnostic.emit".to_owned()),
            semantic_contract: Some(LinkedSemanticContract {
                source: "registered-diagnostic-symbol",
                id: function.clone(),
                evidence: "relocated-symbol-and-reviewed-arity".to_owned(),
                event_dispatch: None,
            }),
            replacement_hint: Some("Rust logging/assertion boundary".to_owned()),
            project_symbol: None,
            project_candidates: Vec::new(),
            trampoline: None,
            argument_shapes: 1,
            arguments: canonical_arguments(arguments),
            argument_bindings: affine_argument_bindings(arguments),
            typed_arguments: Vec::new(),
            guard_paths: None,
        }),
        DraftReferenceEvent::Call {
            site,
            target,
            arguments,
            ..
        } => Some(LinkedCall {
            kind: if resolver.symbols_by_address.contains_key(target) {
                "internal"
            } else {
                "unresolved"
            },
            target: identities.target(*target),
            site: Some(*site),
            tail: false,
            result_modeled: false,
            semantics: None,
            semantic_operation: None,
            semantic_contract: None,
            replacement_hint: None,
            project_symbol: None,
            project_candidates: Vec::new(),
            trampoline: None,
            argument_shapes: 1,
            arguments: canonical_arguments(arguments),
            argument_bindings: affine_argument_bindings(arguments),
            typed_arguments: Vec::new(),
            guard_paths: None,
        }),
        DraftReferenceEvent::TailCall {
            site,
            target,
            arguments,
            ..
        } => Some(LinkedCall {
            kind: if resolver.symbols_by_address.contains_key(target) {
                "internal"
            } else {
                "unresolved"
            },
            target: identities.target(*target),
            site: Some(*site),
            tail: true,
            result_modeled: false,
            semantics: None,
            semantic_operation: None,
            semantic_contract: None,
            replacement_hint: None,
            project_symbol: None,
            project_candidates: Vec::new(),
            trampoline: None,
            argument_shapes: 1,
            arguments: canonical_arguments(arguments),
            argument_bindings: affine_argument_bindings(arguments),
            typed_arguments: Vec::new(),
            guard_paths: None,
        }),
        DraftReferenceEvent::ComposedCall {
            symbol,
            arguments,
            result_modeled,
            ..
        } => Some(LinkedCall {
            kind: "internal",
            target: symbol.clone(),
            site: None,
            tail: false,
            result_modeled: *result_modeled,
            semantics: Some("callee body was composed by the reference resolver".to_owned()),
            semantic_operation: None,
            semantic_contract: None,
            replacement_hint: None,
            project_symbol: None,
            project_candidates: Vec::new(),
            trampoline: None,
            argument_shapes: 1,
            arguments: canonical_arguments(arguments),
            argument_bindings: affine_argument_bindings(arguments),
            typed_arguments: Vec::new(),
            guard_paths: None,
        }),
        DraftReferenceEvent::ScratchCall {
            site,
            target,
            arguments,
            scratch_argument,
            scratch_size,
            ..
        } => Some(LinkedCall {
            kind: if resolver.symbols_by_address.contains_key(target) {
                "internal"
            } else {
                "unresolved"
            },
            target: identities.target(*target),
            site: Some(*site),
            tail: false,
            result_modeled: false,
            semantics: Some(format!(
                "scratch argument={scratch_argument} size={scratch_size}"
            )),
            semantic_operation: None,
            semantic_contract: None,
            replacement_hint: None,
            project_symbol: None,
            project_candidates: Vec::new(),
            trampoline: None,
            argument_shapes: 1,
            arguments: canonical_arguments(arguments),
            argument_bindings: affine_argument_bindings(arguments),
            typed_arguments: Vec::new(),
            guard_paths: None,
        }),
        DraftReferenceEvent::ComposedCallWithScratch {
            symbol,
            arguments,
            result_modeled,
            scratch_argument,
            scratch_size,
            ..
        } => Some(LinkedCall {
            kind: "internal",
            target: symbol.clone(),
            site: None,
            tail: false,
            result_modeled: *result_modeled,
            semantics: Some(format!(
                "composed callee with scratch argument={scratch_argument} size={scratch_size}"
            )),
            semantic_operation: None,
            semantic_contract: None,
            replacement_hint: None,
            project_symbol: None,
            project_candidates: Vec::new(),
            trampoline: None,
            argument_shapes: 1,
            arguments: canonical_arguments(arguments),
            argument_bindings: affine_argument_bindings(arguments),
            typed_arguments: Vec::new(),
            guard_paths: None,
        }),
        _ => None,
    };
    if let Some(call) = call {
        calls.insert(call);
    }
}

#[derive(Default)]
struct DirectCallGraph {
    calls: BTreeSet<LinkedCall>,
    direct_mmio_predicates: BTreeSet<LinkedDirectMmioPredicate>,
    blockers: BTreeSet<String>,
}

#[derive(Clone)]
struct DirectGuardEvidence {
    taken: bool,
    operation: &'static str,
    result_sources: Vec<LinkedCallGuardResultSource>,
    direct_mmio_sources: Vec<LinkedDirectMmioPredicateSource>,
}

type DirectGuardState = BTreeMap<(u32, String), DirectGuardEvidence>;

#[derive(Default)]
struct DirectTraceEvidence {
    guards: DirectGuardState,
    call_results: BTreeMap<u32, String>,
    calls: BTreeSet<LinkedCall>,
    direct_mmio_predicates: BTreeSet<LinkedDirectMmioPredicate>,
}

fn current_guard_path(guards: &DirectGuardState) -> LinkedCallGuardPath {
    LinkedCallGuardPath {
        guards: guards
            .iter()
            .map(|((site, condition), evidence)| LinkedCallGuard {
                site: *site,
                condition: condition.clone(),
                operation: evidence.operation,
                taken: evidence.taken,
                result_sources: evidence.result_sources.clone(),
                direct_mmio_sources: evidence.direct_mmio_sources.clone(),
            })
            .collect(),
    }
}

fn collect_guard_result_source_bits(
    value: &SymbolicValue,
    sources: &mut BTreeMap<(&'static str, u32), u32>,
) {
    let mut recovered_bits = false;
    for source in value.bits() {
        let (kind, token, bit) = match source {
            BitSource::CallResult {
                call_token, bit, ..
            } => ("call-result", call_token, bit),
            BitSource::ExternalResult {
                call_token, bit, ..
            } => ("external-result", call_token, bit),
            _ => continue,
        };
        *sources.entry((kind, token)).or_default() |= 1_u32 << bit;
        recovered_bits = true;
    }
    if recovered_bits {
        return;
    }
    match value {
        SymbolicValue::Expression { left, right, .. } => {
            collect_guard_result_source_bits(left, sources);
            collect_guard_result_source_bits(right, sources);
        }
        SymbolicValue::WideSignedDivide {
            dividend_low,
            dividend_high,
            divisor_low,
            divisor_high,
            ..
        } => {
            for value in [dividend_low, dividend_high, divisor_low, divisor_high] {
                collect_guard_result_source_bits(value, sources);
            }
        }
        _ => {}
    }
}

#[derive(Default)]
struct GuardResultSourceAccumulator {
    value_bits: u32,
    source_bits: u32,
    comparison_known_bits: u32,
    comparison_one_bits: u32,
    comparison_conflict: bool,
}

fn exact_guard_result_operand_sources(
    value: &SymbolicValue,
    operand: &'static str,
    comparison_value: Option<u32>,
    operation: BranchOperation,
    call_results: &BTreeMap<u32, String>,
) -> Vec<LinkedCallGuardResultSource> {
    let mut sources = BTreeMap::<(&'static str, u32, bool), GuardResultSourceAccumulator>::new();
    for (value_bit, source) in value.bits().into_iter().enumerate() {
        let (kind, token, source_bit, inverted) = match source {
            BitSource::CallResult {
                call_token,
                bit,
                inverted,
            } => ("call-result", call_token, bit, inverted),
            BitSource::ExternalResult {
                call_token,
                bit,
                inverted,
            } => ("external-result", call_token, bit, inverted),
            _ => continue,
        };
        let entry = sources.entry((kind, token, inverted)).or_default();
        entry.value_bits |= 1_u32 << value_bit;
        entry.source_bits |= 1_u32 << source_bit;
        let Some(comparison_value) = comparison_value.filter(|_| {
            matches!(
                operation,
                BranchOperation::Equal | BranchOperation::NotEqual
            )
        }) else {
            continue;
        };
        let source_mask = 1_u32 << source_bit;
        let expected = (comparison_value & (1_u32 << value_bit) != 0) ^ inverted;
        if entry.comparison_known_bits & source_mask != 0 {
            let previous = entry.comparison_one_bits & source_mask != 0;
            entry.comparison_conflict |= previous != expected;
        } else {
            entry.comparison_known_bits |= source_mask;
            if expected {
                entry.comparison_one_bits |= source_mask;
            }
        }
    }
    sources
        .into_iter()
        .map(
            |((kind, token, inverted), evidence)| LinkedCallGuardResultSource {
                kind,
                token,
                target: call_results.get(&token).cloned(),
                operand,
                value_bits: Some(evidence.value_bits),
                source_bits: evidence.source_bits,
                inverted,
                comparison_value,
                source_comparison_value: (!evidence.comparison_conflict
                    && evidence.comparison_known_bits == evidence.source_bits)
                    .then_some(evidence.comparison_one_bits),
                producer_return_exact: None,
                mmio_sources: Vec::new(),
            },
        )
        .collect()
}

fn guard_result_operand_sources(
    value: &SymbolicValue,
    operand: &'static str,
    comparison_value: Option<u32>,
    operation: BranchOperation,
    call_results: &BTreeMap<u32, String>,
) -> Vec<LinkedCallGuardResultSource> {
    let exact = exact_guard_result_operand_sources(
        value,
        operand,
        comparison_value,
        operation,
        call_results,
    );
    if !exact.is_empty() {
        return exact;
    }
    let mut fallback = BTreeMap::new();
    collect_guard_result_source_bits(value, &mut fallback);
    fallback
        .into_iter()
        .map(|((kind, token), source_bits)| LinkedCallGuardResultSource {
            kind,
            token,
            target: call_results.get(&token).cloned(),
            operand,
            value_bits: None,
            source_bits,
            inverted: false,
            comparison_value,
            source_comparison_value: None,
            producer_return_exact: None,
            mmio_sources: Vec::new(),
        })
        .collect()
}

fn guard_result_sources(
    condition: &BranchCondition,
    call_results: &BTreeMap<u32, String>,
) -> Vec<LinkedCallGuardResultSource> {
    let mut sources = guard_result_operand_sources(
        &condition.left,
        "left",
        condition.right.as_constant(),
        condition.operation,
        call_results,
    );
    sources.extend(guard_result_operand_sources(
        &condition.right,
        "right",
        condition.left.as_constant(),
        condition.operation,
        call_results,
    ));
    sources.sort();
    sources.dedup();
    sources
}

#[derive(Default)]
struct DirectMmioSourceAccumulator {
    value_bits: u32,
    register_bits: u32,
    comparison_known_bits: u32,
    comparison_one_bits: u32,
    comparison_conflict: bool,
}

fn direct_mmio_operand_sources(
    value: &SymbolicValue,
    operand: &'static str,
    comparison_value: Option<u32>,
    operation: BranchOperation,
    svd: &MmioRegisterMap,
) -> Vec<LinkedDirectMmioPredicateSource> {
    let mut sources = BTreeMap::<(u32, u32, bool), DirectMmioSourceAccumulator>::new();
    for (value_bit, source) in value.bits().into_iter().enumerate() {
        let BitSource::Register {
            read_token,
            address,
            bit: register_bit,
            inverted,
        } = source
        else {
            continue;
        };
        let entry = sources.entry((read_token, address, inverted)).or_default();
        entry.value_bits |= 1_u32 << value_bit;
        entry.register_bits |= 1_u32 << register_bit;
        let Some(comparison_value) = comparison_value.filter(|_| {
            matches!(
                operation,
                BranchOperation::Equal | BranchOperation::NotEqual
            )
        }) else {
            continue;
        };
        let register_mask = 1_u32 << register_bit;
        let expected = (comparison_value & (1_u32 << value_bit) != 0) ^ inverted;
        if entry.comparison_known_bits & register_mask != 0 {
            let previous = entry.comparison_one_bits & register_mask != 0;
            entry.comparison_conflict |= previous != expected;
        } else {
            entry.comparison_known_bits |= register_mask;
            if expected {
                entry.comparison_one_bits |= register_mask;
            }
        }
    }
    sources
        .into_iter()
        .map(
            |((read_token, address, inverted), evidence)| LinkedDirectMmioPredicateSource {
                operand,
                read_token,
                address,
                register: svd.register_name(address),
                value_bits: evidence.value_bits,
                register_bits: evidence.register_bits,
                inverted,
                comparison_value,
                register_comparison_value: (!evidence.comparison_conflict
                    && evidence.comparison_known_bits == evidence.register_bits)
                    .then_some(evidence.comparison_one_bits),
            },
        )
        .collect()
}

fn direct_mmio_predicate_sources(
    condition: &BranchCondition,
    svd: &MmioRegisterMap,
) -> Vec<LinkedDirectMmioPredicateSource> {
    let mut sources = direct_mmio_operand_sources(
        &condition.left,
        "left",
        condition.right.as_constant(),
        condition.operation,
        svd,
    );
    sources.extend(direct_mmio_operand_sources(
        &condition.right,
        "right",
        condition.left.as_constant(),
        condition.operation,
        svd,
    ));
    sources.sort();
    sources.dedup();
    sources
}

fn call_result_identity(
    event: &DraftReferenceEvent,
    identities: &IrIdentityCatalog,
) -> Option<(u32, String)> {
    match event {
        DraftReferenceEvent::Call { token, target, .. }
        | DraftReferenceEvent::ScratchCall { token, target, .. }
        | DraftReferenceEvent::TailCall { token, target, .. } => {
            Some((*token, identities.target(*target)))
        }
        DraftReferenceEvent::ComposedCall { token, symbol, .. }
        | DraftReferenceEvent::ComposedCallWithScratch { token, symbol, .. } => {
            Some((*token, symbol.clone()))
        }
        DraftReferenceEvent::ExternalCall {
            token,
            table,
            function,
            ..
        } => Some((
            *token,
            format!("{}::{}", table.spec().id, function.spec().c_name),
        )),
        _ => None,
    }
}

fn name_call_results(expression: &str, call_results: &BTreeMap<u32, String>) -> String {
    let bytes = expression.as_bytes();
    let mut output = String::with_capacity(expression.len());
    let mut index = 0;
    while index < bytes.len() {
        let prefix_len = if bytes[index..].starts_with(b"external") {
            Some(8)
        } else if bytes[index..].starts_with(b"call") {
            Some(4)
        } else {
            None
        };
        if let Some(prefix_len) = prefix_len {
            let digits_start = index + prefix_len;
            let mut digits_end = digits_start;
            while digits_end < bytes.len() && bytes[digits_end].is_ascii_digit() {
                digits_end += 1;
            }
            if digits_end != digits_start
                && let Ok(token) = expression[digits_start..digits_end].parse::<u32>()
                && let Some(target) = call_results.get(&token)
            {
                output.push_str("result_of_");
                output.push_str(&pseudo_identifier(target));
                output.push('_');
                output.push_str(&token.to_string());
                index = digits_end;
                continue;
            }
        }
        let character = expression[index..]
            .chars()
            .next()
            .expect("index is within the expression");
        output.push(character);
        index += character.len_utf8();
    }
    output
}

fn collect_guarded_direct_event(
    event: &DraftReferenceEvent,
    resolver: &ReferenceResolver,
    identities: &IrIdentityCatalog,
    svd: &MmioRegisterMap,
    evidence: &mut DirectTraceEvidence,
) {
    if let DraftReferenceEvent::BranchDecision { condition, taken } = event {
        let rendered_condition =
            name_call_results(&branch_expression(condition), &evidence.call_results);
        let direct_mmio_sources = direct_mmio_predicate_sources(condition, svd);
        if !direct_mmio_sources.is_empty() {
            evidence
                .direct_mmio_predicates
                .insert(LinkedDirectMmioPredicate {
                    site: condition.site,
                    condition: rendered_condition.clone(),
                    operation: branch_operation(condition.operation),
                    sources: direct_mmio_sources.clone(),
                });
        }
        evidence.guards.insert(
            (condition.site, rendered_condition),
            DirectGuardEvidence {
                taken: *taken,
                operation: branch_operation(condition.operation),
                result_sources: guard_result_sources(condition, &evidence.call_results),
                direct_mmio_sources,
            },
        );
        return;
    }

    let mut event_calls = BTreeSet::new();
    collect_call_event(event, resolver, identities, &mut event_calls);
    for mut call in event_calls {
        call.guard_paths = Some(vec![current_guard_path(&evidence.guards)]);
        evidence.calls.insert(call);
    }
    if let Some((token, target)) = call_result_identity(event, identities) {
        evidence.call_results.insert(token, target);
    }
}

fn explore_direct_calls(
    symbol: &artifact::ArtifactSymbolDefinition,
    resolver: &ReferenceResolver,
    identities: &IrIdentityCatalog,
    svd: &MmioRegisterMap,
) -> DirectCallGraph {
    let mut result = DirectCallGraph::default();
    let mut queue = VecDeque::from([BTreeMap::<u32, bool>::new()]);
    let mut queued = BTreeSet::from([BTreeMap::<u32, bool>::new()]);
    let mut explored_states = 0usize;

    while let Some(forced_branches) = queue.pop_front() {
        if explored_states >= MAX_CALL_GRAPH_STATES {
            result.blockers.insert(format!(
                "call graph exceeds the exploration limit of {MAX_CALL_GRAPH_STATES} states"
            ));
            break;
        }
        explored_states += 1;
        let trace = match direct::trace_binary_symbol_with_branches(
            symbol,
            svd,
            &resolver.relocated_calls,
            &resolver.pointer_context,
            None,
            &forced_branches,
        ) {
            Ok(trace) => trace,
            Err(error) => {
                result.blockers.insert(error.to_string());
                continue;
            }
        };
        let mut evidence = DirectTraceEvidence::default();
        for event in &trace.reference_events {
            collect_guarded_direct_event(event, resolver, identities, svd, &mut evidence);
        }
        result.calls.append(&mut evidence.calls);
        result
            .direct_mmio_predicates
            .append(&mut evidence.direct_mmio_predicates);
        for relocation in symbol.relocations.iter().filter(|relocation| {
            matches!(
                relocation.kind,
                artifact::RelocationKind::Call | artifact::RelocationKind::CallPlt
            )
        }) {
            let unresolved = format!(
                "unresolved-call-relocation at {:#x}: {}",
                relocation.address, relocation.symbol
            );
            if trace
                .reference_blockers
                .iter()
                .any(|blocker| blocker == &unresolved)
            {
                result.calls.insert(LinkedCall {
                    kind: "unresolved",
                    target: relocation.symbol.clone(),
                    site: Some(relocation.address),
                    tail: artifact::relocated_call_is_tail(symbol, relocation.address)
                        .unwrap_or(false),
                    result_modeled: false,
                    semantics: Some(
                        "unresolved call relocation; arguments and callee effects are unavailable"
                            .to_owned(),
                    ),
                    semantic_operation: None,
                    semantic_contract: None,
                    replacement_hint: None,
                    project_symbol: Some(relocation.symbol.clone()),
                    project_candidates: Vec::new(),
                    trampoline: None,
                    argument_shapes: 1,
                    arguments: Vec::new(),
                    argument_bindings: Vec::new(),
                    typed_arguments: Vec::new(),
                    guard_paths: Some(vec![current_guard_path(&evidence.guards)]),
                });
            }
        }
        result
            .blockers
            .extend(trace.reference_blockers.iter().cloned());

        let Some(branch) = trace.unresolved_branch else {
            continue;
        };
        if forced_branches.len() >= MAX_CALL_GRAPH_BRANCH_DECISIONS {
            result.blockers.insert(format!(
                "call graph exceeds the limit of {MAX_CALL_GRAPH_BRANCH_DECISIONS} branch decisions per path at {:#010x}",
                branch.site
            ));
            continue;
        }
        for taken in [false, true] {
            let mut next = forced_branches.clone();
            if next.insert(branch.site, taken).is_some() {
                result.blockers.insert(format!(
                    "call graph revisits branch {:#010x}; that path is incomplete",
                    branch.site
                ));
            } else if queued.insert(next.clone()) {
                queue.push_back(next);
            }
        }
    }

    result
}

fn collect_calls_from_event(
    event: &DraftReferenceEvent,
    resolver: &ReferenceResolver,
    identities: &IrIdentityCatalog,
    calls: &mut BTreeSet<LinkedCall>,
) {
    collect_call_event(event, resolver, identities, calls);
    match event {
        DraftReferenceEvent::BoundedPoll {
            body, on_exhausted, ..
        } => {
            collect_calls_from_flow(body, resolver, identities, calls);
            if let Some(event) = on_exhausted.as_deref() {
                collect_calls_from_event(event, resolver, identities, calls);
            }
        }
        DraftReferenceEvent::PollFlow { body, .. } => {
            collect_calls_from_flow(body, resolver, identities, calls);
        }
        DraftReferenceEvent::SymmetricCalibrationSearch {
            initial_read,
            setup,
            write_candidate,
            sample,
            ..
        } => {
            for flow in [initial_read, setup, write_candidate, sample] {
                collect_calls_from_flow(flow, resolver, identities, calls);
            }
        }
        // A composed call's nested flow belongs to the callee. The caller edge
        // above is direct; recursively collecting it would create transitive
        // edges and obscure the actual call graph.
        _ => {}
    }
}

fn collect_calls_from_flow(
    flow: &DraftReferenceFlow,
    resolver: &ReferenceResolver,
    identities: &IrIdentityCatalog,
    calls: &mut BTreeSet<LinkedCall>,
) {
    for event in &flow.events {
        collect_calls_from_event(event, resolver, identities, calls);
    }
    if let DraftReferenceTerminator::Branch {
        taken, not_taken, ..
    } = &flow.terminator
    {
        collect_calls_from_flow(taken, resolver, identities, calls);
        collect_calls_from_flow(not_taken, resolver, identities, calls);
    }
}

fn nested_path(path: &str, scope: &str) -> String {
    format!("{path} / {scope}")
}

fn width_mask(width: u8) -> u32 {
    match width {
        8 => 0xff,
        16 => 0xffff,
        32 => u32::MAX,
        _ => 0,
    }
}

fn bit_range_mask(lsb: u8, width: u8) -> u32 {
    if width == 32 {
        u32::MAX
    } else {
        ((1_u32 << width) - 1) << lsb
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ReturnBitDescriptor {
    kind: &'static str,
    source_bit: u8,
    inverted: bool,
    argument: Option<u8>,
    token: Option<u32>,
    target: Option<String>,
    address: Option<u32>,
    register: Option<String>,
}

impl ReturnBitDescriptor {
    fn continues_with(&self, next: &Self) -> bool {
        self.kind == next.kind
            && self.source_bit.checked_add(1) == Some(next.source_bit)
            && self.inverted == next.inverted
            && self.argument == next.argument
            && self.token == next.token
            && self.target == next.target
            && self.address == next.address
            && self.register == next.register
    }
}

fn return_bit_descriptor(
    source: BitSource,
    call_results: &BTreeMap<u32, String>,
    svd: &MmioRegisterMap,
) -> Option<ReturnBitDescriptor> {
    let descriptor = match source {
        BitSource::Input {
            index,
            bit,
            inverted,
        } => ReturnBitDescriptor {
            kind: "argument",
            source_bit: bit,
            inverted,
            argument: Some(index),
            token: None,
            target: None,
            address: None,
            register: None,
        },
        BitSource::Register {
            read_token,
            address,
            bit,
            inverted,
        } => ReturnBitDescriptor {
            kind: "mmio-read",
            source_bit: bit,
            inverted,
            argument: None,
            token: Some(read_token),
            target: None,
            address: Some(address),
            register: Some(svd.register_name(address)),
        },
        BitSource::IndexedRegister {
            read_token,
            bit,
            inverted,
        } => ReturnBitDescriptor {
            kind: "indexed-mmio-read",
            source_bit: bit,
            inverted,
            argument: None,
            token: Some(read_token),
            target: None,
            address: None,
            register: None,
        },
        BitSource::Memory {
            read_token,
            bit,
            inverted,
        } => ReturnBitDescriptor {
            kind: "memory-read",
            source_bit: bit,
            inverted,
            argument: None,
            token: Some(read_token),
            target: None,
            address: None,
            register: None,
        },
        BitSource::PrivateStack {
            read_token,
            bit,
            inverted,
        } => ReturnBitDescriptor {
            kind: "private-stack-read",
            source_bit: bit,
            inverted,
            argument: None,
            token: Some(read_token),
            target: None,
            address: None,
            register: None,
        },
        BitSource::CallResult {
            call_token,
            bit,
            inverted,
        } => ReturnBitDescriptor {
            kind: "call-result",
            source_bit: bit,
            inverted,
            argument: None,
            token: Some(call_token),
            target: call_results.get(&call_token).cloned(),
            address: None,
            register: None,
        },
        BitSource::ExternalResult {
            call_token,
            bit,
            inverted,
        } => ReturnBitDescriptor {
            kind: "external-result",
            source_bit: bit,
            inverted,
            argument: None,
            token: Some(call_token),
            target: call_results.get(&call_token).cloned(),
            address: None,
            register: None,
        },
        BitSource::Unknown | BitSource::Constant(_) => return None,
    };
    Some(descriptor)
}

fn return_provenance(
    value: &SymbolicValue,
    call_results: &BTreeMap<u32, String>,
    svd: &MmioRegisterMap,
) -> LinkedReturnProvenance {
    let bits = value.bits();
    let mut known_zero_bits = 0_u32;
    let mut known_one_bits = 0_u32;
    let mut unknown_bits = 0_u32;
    let mut sources = Vec::new();
    let mut output_bit = 0_usize;
    while output_bit < bits.len() {
        match bits[output_bit] {
            BitSource::Constant(false) => {
                known_zero_bits |= 1_u32 << output_bit;
                output_bit += 1;
            }
            BitSource::Constant(true) => {
                known_one_bits |= 1_u32 << output_bit;
                output_bit += 1;
            }
            BitSource::Unknown => {
                unknown_bits |= 1_u32 << output_bit;
                output_bit += 1;
            }
            source => {
                let descriptor = return_bit_descriptor(source, call_results, svd)
                    .expect("non-constant return bit has a source descriptor");
                let first_output_bit = output_bit;
                let first_source_bit = descriptor.source_bit;
                let mut previous = descriptor.clone();
                output_bit += 1;
                while output_bit < bits.len() {
                    let Some(next) = return_bit_descriptor(bits[output_bit], call_results, svd)
                    else {
                        break;
                    };
                    if !previous.continues_with(&next) {
                        break;
                    }
                    previous = next;
                    output_bit += 1;
                }
                let width = (output_bit - first_output_bit) as u8;
                sources.push(LinkedReturnBitSource {
                    kind: descriptor.kind,
                    output_lsb: first_output_bit as u8,
                    source_lsb: first_source_bit,
                    width,
                    output_bits: bit_range_mask(first_output_bit as u8, width),
                    source_bits: bit_range_mask(first_source_bit, width),
                    inverted: descriptor.inverted,
                    argument: descriptor.argument,
                    token: descriptor.token,
                    target: descriptor.target,
                    address: descriptor.address,
                    register: descriptor.register,
                });
            }
        }
    }
    LinkedReturnProvenance {
        exact: unknown_bits == 0,
        known_zero_bits,
        known_one_bits,
        unknown_bits,
        sources,
    }
}

fn trace_call_results(
    trace: &FunctionAnalysis,
    identities: &IrIdentityCatalog,
) -> BTreeMap<u32, String> {
    let mut candidates = BTreeMap::<u32, BTreeSet<String>>::new();
    for event in &trace.reference_events {
        if let Some((token, target)) = call_result_identity(event, identities) {
            candidates.entry(token).or_default().insert(target);
        }
    }
    candidates
        .into_iter()
        .filter_map(|(token, targets)| {
            let mut targets = targets.into_iter();
            let target = targets.next()?;
            targets.next().is_none().then_some((token, target))
        })
        .collect()
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ProjectedMmioReturnBit {
    address: u32,
    register: String,
    register_bit: u8,
    inverted: bool,
    producer_path: Vec<String>,
}

fn project_return_bit_to_mmio(
    producer: &str,
    output_bit: u8,
    producers: &BTreeMap<String, LinkedReturnProvenance>,
) -> Option<ProjectedMmioReturnBit> {
    let mut producer = producer.to_owned();
    let mut output_bit = output_bit;
    let mut active = BTreeSet::new();
    let mut producer_path = Vec::new();
    let mut inverted = false;
    loop {
        if !active.insert((producer.clone(), output_bit)) {
            return None;
        }
        producer_path.push(producer.clone());
        let provenance = producers.get(&producer)?;
        let source = provenance
            .sources
            .iter()
            .find(|source| source.output_bits & (1_u32 << output_bit) != 0)?;
        let source_bit = source.source_lsb + (output_bit - source.output_lsb);
        inverted ^= source.inverted;
        match source.kind {
            "mmio-read" => {
                return Some(ProjectedMmioReturnBit {
                    address: source
                        .address
                        .expect("MMIO return source has a concrete address"),
                    register: source
                        .register
                        .clone()
                        .expect("MMIO return source has a register label"),
                    register_bit: source_bit,
                    inverted,
                    producer_path,
                });
            }
            "call-result" => {
                producer = source.target.clone()?;
                output_bit = source_bit;
            }
            _ => return None,
        }
    }
}

#[derive(Default)]
struct GuardMmioSourceAccumulator {
    result_bits: u32,
    register_bits: u32,
    comparison_known_bits: u32,
    comparison_one_bits: u32,
    comparison_conflict: bool,
}

fn guard_mmio_sources(
    result_source: &LinkedCallGuardResultSource,
    producer: &str,
    producers: &BTreeMap<String, LinkedReturnProvenance>,
) -> Vec<LinkedCallGuardMmioSource> {
    let mut sources =
        BTreeMap::<(u32, String, bool, Vec<String>), GuardMmioSourceAccumulator>::new();
    for result_bit in 0..32_u8 {
        if result_source.source_bits & (1_u32 << result_bit) == 0 {
            continue;
        }
        let Some(projected) = project_return_bit_to_mmio(producer, result_bit, producers) else {
            continue;
        };
        let register_mask = 1_u32 << projected.register_bit;
        let entry = sources
            .entry((
                projected.address,
                projected.register,
                result_source.inverted ^ projected.inverted,
                projected.producer_path,
            ))
            .or_default();
        entry.result_bits |= 1_u32 << result_bit;
        entry.register_bits |= register_mask;
        let Some(comparison_value) = result_source.source_comparison_value else {
            continue;
        };
        let expected = (comparison_value & (1_u32 << result_bit) != 0) ^ projected.inverted;
        if entry.comparison_known_bits & register_mask != 0 {
            let previous = entry.comparison_one_bits & register_mask != 0;
            entry.comparison_conflict |= previous != expected;
        } else {
            entry.comparison_known_bits |= register_mask;
            if expected {
                entry.comparison_one_bits |= register_mask;
            }
        }
    }
    sources
        .into_iter()
        .map(
            |((address, register, inverted, producer_path), evidence)| LinkedCallGuardMmioSource {
                address,
                register,
                producer_path,
                result_bits: evidence.result_bits,
                register_bits: evidence.register_bits,
                inverted,
                result_comparison_value: result_source
                    .source_comparison_value
                    .map(|value| value & evidence.result_bits),
                register_comparison_value: result_source
                    .source_comparison_value
                    .filter(|_| {
                        !evidence.comparison_conflict
                            && evidence.comparison_known_bits == evidence.register_bits
                    })
                    .map(|_| evidence.comparison_one_bits),
            },
        )
        .collect()
}

fn link_guard_result_mmio_sources(functions: &mut [LinkedIrFunction]) {
    let producers = functions
        .iter()
        .map(|function| {
            (
                function.identity.clone(),
                function.return_provenance.clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    for source in functions
        .iter_mut()
        .flat_map(|function| &mut function.calls)
        .filter_map(|call| call.guard_paths.as_mut())
        .flatten()
        .flat_map(|path| &mut path.guards)
        .flat_map(|guard| &mut guard.result_sources)
    {
        let Some(target) = source.target.as_deref() else {
            continue;
        };
        let Some(provenance) = producers.get(target) else {
            continue;
        };
        source.producer_return_exact = Some(provenance.exact);
        source.mmio_sources = guard_mmio_sources(source, target, &producers);
    }
}

fn context_write_masks(
    access: MemoryAccess,
    width: u8,
    value: Option<&SymbolicValue>,
) -> (Option<u32>, Option<u32>, Option<u32>, Option<u32>) {
    if access != MemoryAccess::Write {
        return (None, None, None, None);
    }
    let width_mask = width_mask(width);
    let Some(SymbolicValue::MemoryImage {
        and_mask, or_mask, ..
    }) = value
    else {
        return (Some(width_mask), None, None, None);
    };
    let forced_one = or_mask & width_mask;
    let preserved = and_mask & !forced_one & width_mask;
    let forced_zero = width_mask & !(preserved | forced_one);
    (
        Some(forced_zero | forced_one),
        Some(preserved),
        Some(forced_zero),
        Some(forced_one),
    )
}

fn collect_context_access_from_event(
    event: &DraftReferenceEvent,
    path: &str,
    output: &mut Vec<ContextAccess>,
) {
    match event {
        DraftReferenceEvent::Memory {
            access,
            width,
            address,
            value,
            ..
        } => {
            if let Some((argument, offset)) = address.caller_memory_location() {
                let (write_mask, preserved_mask, forced_zero_mask, forced_one_mask) =
                    context_write_masks(*access, *width, value.as_ref());
                output.push(ContextAccess {
                    argument,
                    offset,
                    access: match access {
                        MemoryAccess::Read => "read",
                        MemoryAccess::Write => "write",
                    },
                    width: *width,
                    path: path.to_owned(),
                    value: value.as_ref().map(SymbolicValue::canonical),
                    value_pseudo: value.as_ref().map(pseudo_value),
                    write_mask,
                    preserved_mask,
                    forced_zero_mask,
                    forced_one_mask,
                });
            }
        }
        DraftReferenceEvent::BoundedPoll {
            body, on_exhausted, ..
        } => {
            collect_context_access_from_flow(body, &nested_path(path, "bounded-poll"), output);
            if let Some(event) = on_exhausted.as_deref() {
                collect_context_access_from_event(
                    event,
                    &nested_path(path, "poll-exhausted"),
                    output,
                );
            }
        }
        DraftReferenceEvent::PollFlow { body, .. } => {
            collect_context_access_from_flow(body, &nested_path(path, "poll"), output);
        }
        DraftReferenceEvent::SymmetricCalibrationSearch {
            initial_read,
            setup,
            write_candidate,
            sample,
            ..
        } => {
            for (scope, flow) in [
                ("calibration-initial-read", initial_read),
                ("calibration-setup", setup),
                ("calibration-write-candidate", write_candidate),
                ("calibration-sample", sample),
            ] {
                collect_context_access_from_flow(flow, &nested_path(path, scope), output);
            }
        }
        DraftReferenceEvent::ComposedCall { symbol, flow, .. }
        | DraftReferenceEvent::ComposedCallWithScratch { symbol, flow, .. } => {
            collect_context_access_from_flow(
                flow,
                &nested_path(path, &format!("call {symbol}")),
                output,
            );
        }
        _ => {}
    }
}

fn collect_context_access_from_flow(
    flow: &DraftReferenceFlow,
    path: &str,
    output: &mut Vec<ContextAccess>,
) {
    for event in &flow.events {
        collect_context_access_from_event(event, path, output);
    }
    if let DraftReferenceTerminator::Branch {
        condition,
        taken,
        not_taken,
    } = &flow.terminator
    {
        let condition = branch_expression(condition);
        collect_context_access_from_flow(
            taken,
            &nested_path(path, &format!("if {condition}")),
            output,
        );
        collect_context_access_from_flow(
            not_taken,
            &nested_path(path, &format!("if !({condition})")),
            output,
        );
    }
}

fn context_accesses_for_trace(trace: &FunctionAnalysis) -> Vec<ContextAccess> {
    let mut output = Vec::new();
    if let Some(flow) = trace.reference_flow.as_ref() {
        collect_context_access_from_flow(flow, "entry", &mut output);
    } else {
        for event in &trace.reference_events {
            collect_context_access_from_event(event, "entry", &mut output);
        }
    }
    output.sort();
    output.dedup();
    output
}

fn context_fields_for_accesses(accesses: &[ContextAccess]) -> Vec<ContextField> {
    let mut fields = BTreeMap::<(u8, i32, u8), ContextField>::new();
    for access in accesses {
        let field = fields
            .entry((access.argument, access.offset, access.width))
            .or_insert_with(|| ContextField {
                argument: access.argument,
                offset: access.offset,
                width: access.width,
                reads: 0,
                writes: 0,
                write_mask: 0,
                paths: Vec::new(),
                write_values: Vec::new(),
            });
        match access.access {
            "read" => field.reads += 1,
            "write" => {
                field.writes += 1;
                field.write_mask |= access.write_mask.unwrap_or_default();
                if let Some(value) = access.value_pseudo.as_ref()
                    && !field.write_values.contains(value)
                {
                    field.write_values.push(value.clone());
                }
            }
            _ => unreachable!("context access has a closed access vocabulary"),
        }
        if !field.paths.contains(&access.path) {
            field.paths.push(access.path.clone());
        }
    }
    fields.into_values().collect()
}

fn mmio_write_masks(
    access: MemoryAccess,
    address: u32,
    width: u8,
    value: Option<&SymbolicValue>,
) -> [Option<u32>; 7] {
    if access != MemoryAccess::Write {
        return [None; 7];
    }
    let pattern = super::mmio_discovery::classify_write_bits(value, address, width);
    [
        Some(pattern.modified_mask(width)),
        Some(pattern.preserved_mask),
        Some(pattern.inverted_mask),
        Some(pattern.forced_zero_mask),
        Some(pattern.forced_one_mask),
        Some(pattern.read_derived_mask),
        Some(pattern.dynamic_mask),
    ]
}

struct MmioAccessDraft<'a> {
    address: u32,
    width: u8,
    register: &'a str,
    access: MemoryAccess,
    mode: &'static str,
    path: &'a str,
    address_expression: Option<String>,
    guard: Option<String>,
    value: Option<&'a SymbolicValue>,
}

fn push_mmio_access(output: &mut Vec<LinkedMmioAccess>, draft: MmioAccessDraft<'_>) {
    let MmioAccessDraft {
        address,
        width,
        register,
        access,
        mode,
        path,
        address_expression,
        guard,
        value,
    } = draft;
    let [
        modified_mask,
        preserved_mask,
        inverted_mask,
        forced_zero_mask,
        forced_one_mask,
        read_derived_mask,
        dynamic_mask,
    ] = mmio_write_masks(access, address, width, value);
    output.push(LinkedMmioAccess {
        ordinal: output.len(),
        address,
        width,
        register: register.to_owned(),
        access: match access {
            MemoryAccess::Read => "read",
            MemoryAccess::Write => "write",
        },
        mode,
        path: path.to_owned(),
        address_expression,
        guard,
        predicate_mask: None,
        predicate_expected: None,
        value: value.map(pseudo_value),
        modified_mask,
        preserved_mask,
        inverted_mask,
        forced_zero_mask,
        forced_one_mask,
        read_derived_mask,
        dynamic_mask,
    });
}

fn collect_mmio_access_from_event(
    event: &DraftReferenceEvent,
    path: &str,
    output: &mut Vec<LinkedMmioAccess>,
) {
    match event {
        DraftReferenceEvent::Observable(ObservableEvent::Memory {
            access,
            width,
            address,
            register,
            value,
        }) => push_mmio_access(
            output,
            MmioAccessDraft {
                address: *address,
                width: *width,
                register,
                access: *access,
                mode: "static",
                path,
                address_expression: None,
                guard: None,
                value: value.as_ref(),
            },
        ),
        DraftReferenceEvent::IndexedMmio {
            access,
            width,
            address,
            registers,
            guard,
            value,
        } => {
            let address_expression = Some(pseudo_value(address));
            let guard = guard
                .as_ref()
                .map(|guard| format!("{} < {}", pseudo_value(&guard.selector), guard.maximum));
            for register in registers {
                push_mmio_access(
                    output,
                    MmioAccessDraft {
                        address: register.address,
                        width: *width,
                        register: &register.name,
                        access: *access,
                        mode: "indexed-candidate",
                        path,
                        address_expression: address_expression.clone(),
                        guard: guard.clone(),
                        value: value.as_ref(),
                    },
                );
            }
        }
        DraftReferenceEvent::PollMmio {
            width,
            address,
            registers,
            guard,
            mask,
            expected,
        } => {
            let address_expression = Some(pseudo_value(address));
            let guard = guard.as_ref().map_or_else(
                || format!("value & {mask:#010x} == {expected:#010x}"),
                |guard| {
                    format!(
                        "{} < {}; value & {mask:#010x} == {expected:#010x}",
                        pseudo_value(&guard.selector),
                        guard.maximum
                    )
                },
            );
            for register in registers {
                let mut access = LinkedMmioAccess {
                    ordinal: output.len(),
                    address: register.address,
                    width: *width,
                    register: register.name.clone(),
                    access: "poll",
                    mode: "indexed-candidate",
                    path: path.to_owned(),
                    address_expression: address_expression.clone(),
                    guard: Some(guard.clone()),
                    predicate_mask: Some(*mask),
                    predicate_expected: Some(*expected),
                    value: None,
                    modified_mask: None,
                    preserved_mask: None,
                    inverted_mask: None,
                    forced_zero_mask: None,
                    forced_one_mask: None,
                    read_derived_mask: None,
                    dynamic_mask: None,
                };
                if registers.len() == 1 && address.as_constant() == Some(register.address) {
                    access.mode = "static";
                    access.address_expression = None;
                }
                output.push(access);
            }
        }
        DraftReferenceEvent::BoundedPoll {
            body, on_exhausted, ..
        } => {
            collect_mmio_access_from_flow(body, &nested_path(path, "bounded-poll"), output);
            if let Some(event) = on_exhausted.as_deref() {
                collect_mmio_access_from_event(event, &nested_path(path, "poll-exhausted"), output);
            }
        }
        DraftReferenceEvent::PollFlow { body, .. } => {
            collect_mmio_access_from_flow(body, &nested_path(path, "poll"), output);
        }
        DraftReferenceEvent::SymmetricCalibrationSearch {
            initial_read,
            setup,
            write_candidate,
            sample,
            ..
        } => {
            for (scope, flow) in [
                ("calibration-initial-read", initial_read),
                ("calibration-setup", setup),
                ("calibration-write-candidate", write_candidate),
                ("calibration-sample", sample),
            ] {
                collect_mmio_access_from_flow(flow, &nested_path(path, scope), output);
            }
        }
        DraftReferenceEvent::ComposedCall { symbol, flow, .. }
        | DraftReferenceEvent::ComposedCallWithScratch { symbol, flow, .. } => {
            collect_mmio_access_from_flow(
                flow,
                &nested_path(path, &format!("call {symbol}")),
                output,
            );
        }
        _ => {}
    }
}

fn collect_mmio_access_from_flow(
    flow: &DraftReferenceFlow,
    path: &str,
    output: &mut Vec<LinkedMmioAccess>,
) {
    for event in &flow.events {
        collect_mmio_access_from_event(event, path, output);
    }
    if let DraftReferenceTerminator::Branch {
        condition,
        taken,
        not_taken,
    } = &flow.terminator
    {
        let condition = branch_expression(condition);
        collect_mmio_access_from_flow(
            taken,
            &nested_path(path, &format!("if {condition}")),
            output,
        );
        collect_mmio_access_from_flow(
            not_taken,
            &nested_path(path, &format!("if !({condition})")),
            output,
        );
    }
}

fn mmio_accesses_for_trace(trace: &FunctionAnalysis) -> Vec<LinkedMmioAccess> {
    let mut output = Vec::new();
    if let Some(flow) = trace.reference_flow.as_ref() {
        collect_mmio_access_from_flow(flow, "entry", &mut output);
    } else {
        for event in &trace.reference_events {
            collect_mmio_access_from_event(event, "entry", &mut output);
        }
    }
    output
}

fn collect_delay_from_event(
    event: &DraftReferenceEvent,
    path: &str,
    output: &mut Vec<LinkedDelay>,
) {
    match event {
        DraftReferenceEvent::DelayMicros { micros } => output.push(LinkedDelay {
            ordinal: output.len(),
            path: path.to_owned(),
            micros: micros.canonical(),
            constant_micros: micros.as_constant(),
        }),
        DraftReferenceEvent::BoundedPoll {
            body, on_exhausted, ..
        } => {
            collect_delays_from_flow(body, &nested_path(path, "bounded-poll"), output);
            if let Some(event) = on_exhausted.as_deref() {
                collect_delay_from_event(event, &nested_path(path, "poll-exhausted"), output);
            }
        }
        DraftReferenceEvent::PollFlow { body, .. } => {
            collect_delays_from_flow(body, &nested_path(path, "poll"), output);
        }
        DraftReferenceEvent::SymmetricCalibrationSearch {
            settle_micros,
            initial_read,
            setup,
            write_candidate,
            sample,
            ..
        } => {
            output.push(LinkedDelay {
                ordinal: output.len(),
                path: nested_path(path, "calibration-settle"),
                micros: SymbolicValue::Constant(*settle_micros).canonical(),
                constant_micros: Some(*settle_micros),
            });
            for (scope, flow) in [
                ("calibration-initial-read", initial_read),
                ("calibration-setup", setup),
                ("calibration-write-candidate", write_candidate),
                ("calibration-sample", sample),
            ] {
                collect_delays_from_flow(flow, &nested_path(path, scope), output);
            }
        }
        DraftReferenceEvent::ComposedCall { symbol, flow, .. }
        | DraftReferenceEvent::ComposedCallWithScratch { symbol, flow, .. } => {
            collect_delays_from_flow(flow, &nested_path(path, &format!("call {symbol}")), output);
        }
        _ => {}
    }
}

fn collect_delays_from_flow(flow: &DraftReferenceFlow, path: &str, output: &mut Vec<LinkedDelay>) {
    for event in &flow.events {
        collect_delay_from_event(event, path, output);
    }
    if let DraftReferenceTerminator::Branch {
        condition,
        taken,
        not_taken,
    } = &flow.terminator
    {
        let condition = branch_expression(condition);
        collect_delays_from_flow(
            taken,
            &nested_path(path, &format!("if {condition}")),
            output,
        );
        collect_delays_from_flow(
            not_taken,
            &nested_path(path, &format!("if !({condition})")),
            output,
        );
    }
}

fn delays_for_trace(trace: &FunctionAnalysis) -> Vec<LinkedDelay> {
    let mut output = Vec::new();
    if let Some(flow) = trace.reference_flow.as_ref() {
        collect_delays_from_flow(flow, "entry", &mut output);
    } else {
        for event in &trace.reference_events {
            collect_delay_from_event(event, "entry", &mut output);
        }
    }
    output
}

#[derive(Clone, Default)]
struct RenderState {
    mmio_reads: u32,
    memory_reads: u32,
}

fn indent(level: usize) -> String {
    "    ".repeat(level)
}

fn render_observable(
    event: &ObservableEvent,
    output: &mut String,
    level: usize,
    state: &mut RenderState,
) {
    let prefix = indent(level);
    match event {
        ObservableEvent::Memory {
            access,
            width,
            address,
            register,
            value,
        } => match access {
            MemoryAccess::Read => {
                writeln!(
                    output,
                    "{prefix}let read{} = mmio.read{width}({address:#010x}); // {register}",
                    state.mmio_reads
                )
                .unwrap();
                state.mmio_reads += 1;
            }
            MemoryAccess::Write => {
                let value = value
                    .as_ref()
                    .map_or_else(|| "unknown".to_owned(), pseudo_value);
                writeln!(
                    output,
                    "{prefix}mmio.write{width}({address:#010x}, {value}); // {register}"
                )
                .unwrap();
            }
        },
        ObservableEvent::Fence {
            fm,
            predecessor,
            successor,
        } => writeln!(
            output,
            "{prefix}fence(fm={fm:#x}, pred={predecessor:#x}, succ={successor:#x});"
        )
        .unwrap(),
    }
}

fn render_embedded_flow(
    label: &str,
    flow: &DraftReferenceFlow,
    output: &mut String,
    level: usize,
    state: &mut RenderState,
) {
    let prefix = indent(level);
    writeln!(output, "{prefix}// {label}").unwrap();
    for event in &flow.events {
        render_event(event, output, level, state);
    }
}

fn render_event(
    event: &DraftReferenceEvent,
    output: &mut String,
    level: usize,
    state: &mut RenderState,
) {
    let prefix = indent(level);
    match event {
        DraftReferenceEvent::Observable(event) => {
            render_observable(event, output, level, state);
        }
        DraftReferenceEvent::IndexedMmio {
            access,
            width,
            address,
            registers,
            guard,
            value,
        } => {
            let candidates = registers
                .iter()
                .map(|register| format!("{}@{:#010x}", register.name, register.address))
                .collect::<Vec<_>>()
                .join(", ");
            if let Some(guard) = guard {
                writeln!(
                    output,
                    "{prefix}assert!({} < {});",
                    pseudo_value(&guard.selector),
                    guard.maximum
                )
                .unwrap();
            }
            match access {
                MemoryAccess::Read => {
                    writeln!(
                        output,
                        "{prefix}let read{} = mmio.read{width}({}); // indexed: {candidates}",
                        state.mmio_reads,
                        pseudo_value(address)
                    )
                    .unwrap();
                    state.mmio_reads += 1;
                }
                MemoryAccess::Write => {
                    let value = value
                        .as_ref()
                        .map_or_else(|| "unknown".to_owned(), pseudo_value);
                    writeln!(
                        output,
                        "{prefix}mmio.write{width}({}, {value}); // indexed: {candidates}",
                        pseudo_value(address)
                    )
                    .unwrap();
                }
            }
        }
        DraftReferenceEvent::PollMmio {
            width,
            address,
            mask,
            expected,
            ..
        } => writeln!(
            output,
            "{prefix}while (mmio.read{width}({}) & {mask:#010x}) != {expected:#010x} {{ spin(); }}",
            pseudo_value(address)
        )
        .unwrap(),
        DraftReferenceEvent::BoundedPoll {
            maximum_attempts,
            body,
            repeat_while_mask,
            repeat_while_expected,
            on_exhausted,
        } => {
            writeln!(
                output,
                "{prefix}for attempt in 0..{maximum_attempts} {{ // repeat while result & {repeat_while_mask:#010x} == {repeat_while_expected:#010x}"
            )
            .unwrap();
            render_embedded_flow("poll body", body, output, level + 1, state);
            writeln!(output, "{prefix}}}").unwrap();
            if let Some(event) = on_exhausted.as_deref() {
                writeln!(output, "{prefix}if exhausted {{").unwrap();
                render_event(event, output, level + 1, state);
                writeln!(output, "{prefix}}}").unwrap();
            }
        }
        DraftReferenceEvent::PollFlow {
            body,
            exit_when_mask,
            exit_when_expected,
        } => {
            writeln!(
                output,
                "{prefix}loop {{ // exit when result & {exit_when_mask:#010x} == {exit_when_expected:#010x}"
            )
            .unwrap();
            render_embedded_flow("poll flow", body, output, level + 1, state);
            writeln!(output, "{prefix}}}").unwrap();
        }
        DraftReferenceEvent::SymmetricCalibrationSearch {
            attempts_per_direction,
            settle_micros,
            sample_shift,
            sample_mask,
            accepted_sample,
            initial_read,
            setup,
            write_candidate,
            sample,
            ..
        } => {
            writeln!(
                output,
                "{prefix}calibration_search(attempts={attempts_per_direction}, settle_us={settle_micros}, sample=(_ >> {sample_shift}) & {sample_mask:#x}, accepted={accepted_sample:#x}) {{"
            )
            .unwrap();
            for (label, flow) in [
                ("initial read", initial_read),
                ("setup", setup),
                ("write candidate", write_candidate),
                ("sample", sample),
            ] {
                render_embedded_flow(label, flow, output, level + 1, state);
            }
            writeln!(output, "{prefix}}}").unwrap();
        }
        DraftReferenceEvent::DelayMicros { micros } => {
            writeln!(output, "{prefix}delay_us({});", pseudo_value(micros)).unwrap();
        }
        DraftReferenceEvent::Memory {
            access,
            width,
            address,
            region,
            value,
        } => match access {
            MemoryAccess::Read => {
                if let Some((argument, offset)) = address.caller_memory_location() {
                    writeln!(
                        output,
                        "{prefix}let ramread{} = ctx{argument}.read{width}({offset:+#x}); // {region}",
                        state.memory_reads,
                    )
                    .unwrap();
                } else {
                    writeln!(
                        output,
                        "{prefix}let ramread{} = memory.read{width}({}); // {region}",
                        state.memory_reads,
                        pseudo_value(address)
                    )
                    .unwrap();
                }
                state.memory_reads += 1;
            }
            MemoryAccess::Write => {
                let value = value.as_ref().map_or_else(|| "unknown".to_owned(), pseudo_value);
                if let Some((argument, offset)) = address.caller_memory_location() {
                    writeln!(
                        output,
                        "{prefix}ctx{argument}.write{width}({offset:+#x}, {value}); // {region}"
                    )
                    .unwrap();
                } else {
                    writeln!(
                        output,
                        "{prefix}memory.write{width}({}, {value}); // {region}",
                        pseudo_value(address)
                    )
                    .unwrap();
                }
            }
        },
        DraftReferenceEvent::PrivateStackLoad {
            token,
            offset,
            width,
            signed,
        } => writeln!(
            output,
            "{prefix}let private_stack_read{token} = stack.load{width}({offset:+#x}, signed={signed});"
        )
        .unwrap(),
        DraftReferenceEvent::PrivateStackStore {
            offset,
            width,
            value,
        } => writeln!(
            output,
            "{prefix}stack.store{width}({offset:+#x}, {});",
            pseudo_value(value)
        )
        .unwrap(),
        DraftReferenceEvent::ExternalCall {
            token,
            table,
            function,
            arguments,
        } => {
            let function_spec = function.spec();
            writeln!(
                output,
                "{prefix}let external{token} = semantic.{}({}); // ABI {}+{:#x} {}, returns {}; replacement: {}",
                pseudo_identifier(function_spec.semantic.operation),
                pseudo_external_arguments(*function, arguments),
                table.spec().id,
                function_spec.offset,
                function_spec.c_name,
                function_spec.semantic.return_type,
                function_spec.semantic.replacement.unwrap_or("none"),
            )
            .unwrap();
        }
        DraftReferenceEvent::DiagnosticCall {
            function,
            arguments,
            ..
        } => writeln!(
            output,
            "{prefix}diagnostic.{function}({});",
            pseudo_arguments(arguments)
        )
        .unwrap(),
        DraftReferenceEvent::Call {
            token,
            target,
            arguments,
            ..
        } => writeln!(
            output,
            "{prefix}let call{token} = sub_{target:08x}({});",
            pseudo_arguments(arguments)
        )
        .unwrap(),
        DraftReferenceEvent::TailCall {
            target, arguments, ..
        } => writeln!(
            output,
            "{prefix}return sub_{target:08x}({}); // tail call",
            pseudo_arguments(arguments)
        )
        .unwrap(),
        DraftReferenceEvent::ComposedCall {
            token,
            symbol,
            arguments,
            result_modeled,
            ..
        } => {
            let callee = pseudo_identifier(symbol);
            if *result_modeled {
                writeln!(
                    output,
                    "{prefix}let call{token} = {callee}({});",
                    pseudo_arguments(arguments)
                )
                .unwrap();
            } else {
                writeln!(
                    output,
                    "{prefix}{callee}({}); // return value not modeled",
                    pseudo_arguments(arguments)
                )
                .unwrap();
            }
        }
        DraftReferenceEvent::ScratchCall {
            token,
            target,
            arguments,
            scratch_argument,
            scratch_size,
            ..
        } => writeln!(
            output,
            "{prefix}let call{token} = sub_{target:08x}_with_scratch(arg={scratch_argument}, size={scratch_size}, [{}]);",
            pseudo_arguments(arguments)
        )
        .unwrap(),
        DraftReferenceEvent::ComposedCallWithScratch {
            token,
            symbol,
            arguments,
            result_modeled,
            scratch_argument,
            scratch_size,
            ..
        } => writeln!(
            output,
            "{prefix}{}{}({}); // scratch arg={scratch_argument} size={scratch_size}, result-modeled={result_modeled}",
            if *result_modeled {
                format!("let call{token} = ")
            } else {
                String::new()
            },
            pseudo_identifier(symbol),
            pseudo_arguments(arguments)
        )
        .unwrap(),
        DraftReferenceEvent::WideSignedDivide {
            token,
            dividend_low,
            dividend_high,
            divisor_low,
            divisor_high,
        } => writeln!(
            output,
            "{prefix}let wide_div{token} = sdiv64(low={}, high={}, divisor_low={}, divisor_high={});",
            pseudo_value(dividend_low),
            pseudo_value(dividend_high),
            pseudo_value(divisor_low),
            pseudo_value(divisor_high)
        )
        .unwrap(),
        DraftReferenceEvent::BranchDecision { condition, taken } => writeln!(
            output,
            "{prefix}// forced branch at {:#010x}: {} => {taken}",
            condition.site,
            branch_expression(condition)
        )
        .unwrap(),
    }
}

fn render_flow(
    flow: &DraftReferenceFlow,
    output: &mut String,
    level: usize,
    mut state: RenderState,
) {
    for event in &flow.events {
        render_event(event, output, level, &mut state);
    }
    let prefix = indent(level);
    match &flow.terminator {
        DraftReferenceTerminator::Return(value) => {
            writeln!(output, "{prefix}return {};", pseudo_value(value)).unwrap();
        }
        DraftReferenceTerminator::Branch {
            condition,
            taken,
            not_taken,
        } => {
            writeln!(
                output,
                "{prefix}if {} {{ // site {:#010x}",
                branch_expression(condition),
                condition.site
            )
            .unwrap();
            render_flow(taken, output, level + 1, state.clone());
            writeln!(output, "{prefix}}} else {{").unwrap();
            render_flow(not_taken, output, level + 1, state);
            writeln!(output, "{prefix}}}").unwrap();
        }
    }
}

fn render_pseudo(
    identity: &str,
    trace: &FunctionAnalysis,
    calls: &[LinkedCall],
    direct_blockers: &[String],
    reference_blockers: &[String],
    call_graph_blockers: &[String],
) -> String {
    let mut output = String::new();
    writeln!(output, "// vendor symbol: {identity}").unwrap();
    for blocker in direct_blockers {
        writeln!(output, "// DIRECT-BLOCKER: {blocker}").unwrap();
    }
    for blocker in reference_blockers {
        writeln!(output, "// REFERENCE-BLOCKER: {blocker}").unwrap();
    }
    for blocker in call_graph_blockers {
        writeln!(output, "// CALL-GRAPH-BLOCKER: {blocker}").unwrap();
    }
    for call in calls {
        let site = call
            .site
            .map_or_else(|| "unknown-site".to_owned(), |site| format!("{site:#010x}"));
        let semantic = call.semantic_operation.as_deref().unwrap_or("-");
        let contract = call
            .semantic_contract
            .as_ref()
            .map_or("-", |contract| contract.id.as_str());
        let guard_paths = call
            .guard_paths
            .as_ref()
            .map_or_else(|| "unknown".to_owned(), |paths| paths.len().to_string());
        writeln!(
            output,
            "// DIRECT-CALL {site}: {} {}{} [argument-shapes={}] [cfg-guard-paths={guard_paths}] [semantic={semantic}] [contract={contract}]",
            call.kind,
            call.target,
            if call.tail { " [tail]" } else { "" },
            call.argument_shapes,
        )
        .unwrap();
    }
    writeln!(
        output,
        "fn {}(args: [u32; 16]) -> u32 {{",
        pseudo_identifier(identity)
    )
    .unwrap();
    writeln!(
        output,
        "    // argN denotes args[N]; ctxN denotes memory rooted at pointer argument N."
    )
    .unwrap();
    if let Some(flow) = trace.reference_flow.as_ref() {
        render_flow(flow, &mut output, 1, RenderState::default());
    } else {
        let mut state = RenderState::default();
        for event in &trace.reference_events {
            render_event(event, &mut output, 1, &mut state);
        }
        if trace.unresolved_branch.is_some() {
            writeln!(
                output,
                "    // control flow continues beyond the recovered prefix"
            )
            .unwrap();
        }
        writeln!(output, "    return {};", pseudo_value(&trace.return_value)).unwrap();
    }
    output.push_str("}\n");
    output
}

fn calls_for_trace(
    trace: &FunctionAnalysis,
    resolver: &ReferenceResolver,
    identities: &IrIdentityCatalog,
) -> Vec<LinkedCall> {
    let mut calls = BTreeSet::new();
    if let Some(flow) = trace.reference_flow.as_ref() {
        collect_calls_from_flow(flow, resolver, identities, &mut calls);
    } else {
        for event in &trace.reference_events {
            collect_calls_from_event(event, resolver, identities, &mut calls);
        }
    }
    compact_calls(calls)
}

fn annotate_direct_semantic_calls(
    calls: &mut [LinkedCall],
    resolver: &ReferenceResolver,
    identities: &IrIdentityCatalog,
) {
    let Some(hooks) = resolver.pointer_context.summary_hooks else {
        return;
    };
    for call in calls
        .iter_mut()
        .filter(|call| call.kind == "internal" && call.semantic_operation.is_none())
    {
        let Some(symbol) = identities.selectable_symbol(&call.target) else {
            continue;
        };
        let Some(function) = (hooks.direct_semantic)(symbol) else {
            continue;
        };
        debug_assert_eq!(
            function.semantic.arguments.len(),
            usize::from(function.argument_count),
            "direct semantic ABI arity must match its typed arguments"
        );
        call.semantics = Some(format!(
            "reviewed direct semantic function={} args={} operation={}",
            function.c_name, function.argument_count, function.semantic.operation,
        ));
        call.semantic_operation = Some(function.semantic.operation.to_owned());
        call.semantic_contract = Some(LinkedSemanticContract {
            source: "reviewed-internal-function",
            id: function.id.to_owned(),
            evidence: function.evidence.to_owned(),
            event_dispatch: linked_event_dispatch_contract(function.semantic),
        });
        call.replacement_hint = function.semantic.replacement.map(str::to_owned);
        call.typed_arguments = direct_semantic_typed_arguments(function, &call.arguments);
    }
}

pub(crate) fn build_linked_ir_for_source(
    resolver: &ReferenceResolver,
    symbol_prefix: &str,
    svd: &MmioRegisterMap,
    source: &str,
    namespace_identities: bool,
    include_reachable: bool,
) -> LinkedIrReport {
    let mut functions = Vec::new();
    let identities = IrIdentityCatalog::new(resolver, namespace_identities.then_some(source));
    let mut scheduled = BTreeSet::<SymbolKey>::new();
    let mut pending = VecDeque::new();
    for symbol in resolver
        .symbols
        .iter()
        .filter(|symbol| symbol.name.starts_with(symbol_prefix))
    {
        if scheduled.insert(symbol_key(symbol)) {
            pending.push_back(symbol.clone());
        }
    }

    while let Some(symbol) = pending.pop_front() {
        let selection = if symbol.name.starts_with(symbol_prefix) {
            "symbol-prefix-root"
        } else {
            "reachable-internal"
        };
        let function_identity = identities.symbol(&symbol);
        let binding = if resolver.symbol_is_exported(&symbol) {
            "global-or-weak"
        } else {
            "local"
        };
        let DirectCallGraph {
            calls: direct_calls,
            direct_mmio_predicates,
            blockers,
        } = explore_direct_calls(&symbol, resolver, &identities, svd);
        let direct_mmio_predicates = direct_mmio_predicates.into_iter().collect::<Vec<_>>();
        if include_reachable {
            for call in direct_calls.iter().filter(|call| call.kind == "internal") {
                let Some(callee) = identities.selectable_symbol(&call.target) else {
                    continue;
                };
                if scheduled.insert(symbol_key(callee)) {
                    pending.push_back(callee.clone());
                }
            }
        }
        let call_graph_messages = blockers.into_iter().collect::<Vec<_>>();
        let call_graph_diagnostics = compact_diagnostics(&call_graph_messages);
        let call_graph_blockers = rendered_diagnostics(&call_graph_diagnostics);
        match resolver.trace_symbol(&symbol, svd) {
            Ok(trace) => {
                let context_accesses = context_accesses_for_trace(&trace);
                let context_fields = context_fields_for_accesses(&context_accesses);
                let mmio_accesses = mmio_accesses_for_trace(&trace);
                let delays = delays_for_trace(&trace);
                let return_call_results = trace_call_results(&trace, &identities);
                let return_provenance =
                    return_provenance(&trace.return_value, &return_call_results, svd);
                let mut calls = if direct_calls.is_empty() {
                    calls_for_trace(&trace, resolver, &identities)
                } else {
                    compact_calls(direct_calls)
                };
                annotate_direct_semantic_calls(&mut calls, resolver, &identities);
                let flow_kind = if trace.reference_flow.is_some() {
                    "structured"
                } else if trace.is_reference_eligible() {
                    "linear"
                } else {
                    "partial"
                };
                let direct_diagnostics = compact_diagnostics(&trace.blockers);
                let reference_diagnostics = compact_diagnostics(&trace.reference_blockers);
                let direct_blockers = rendered_diagnostics(&direct_diagnostics);
                let reference_blockers = rendered_diagnostics(&reference_diagnostics);
                let pseudo = render_pseudo(
                    &function_identity,
                    &trace,
                    &calls,
                    &direct_blockers,
                    &reference_blockers,
                    &call_graph_blockers,
                );
                functions.push(LinkedIrFunction {
                    source: source.to_owned(),
                    identity: function_identity.clone(),
                    selection,
                    member: symbol.member.clone(),
                    symbol: symbol.name.clone(),
                    binding,
                    address: symbol.addresses_resolved.then_some(symbol.address as u32),
                    object_offset: symbol.address as u32,
                    size: symbol.bytes.len(),
                    flow_kind,
                    complete: trace.is_reference_eligible(),
                    exact: trace.is_exact(),
                    return_value: trace.return_value.canonical(),
                    return_provenance,
                    dependencies: trace
                        .reference_dependencies
                        .iter()
                        .map(|dependency| {
                            if namespace_identities {
                                format!("{source}::{dependency}")
                            } else {
                                dependency.clone()
                            }
                        })
                        .collect(),
                    calls,
                    direct_mmio_predicates,
                    mmio_accesses,
                    delays,
                    context_accesses,
                    context_fields,
                    effect_summary: LinkedEffectSummary::default(),
                    call_graph_diagnostics,
                    direct_diagnostics,
                    reference_diagnostics,
                    call_graph_blockers,
                    direct_blockers,
                    reference_blockers,
                    pseudo,
                });
            }
            Err(error) => {
                let direct_diagnostics = vec![compact_diagnostic(&error.to_string())];
                let direct_blockers = rendered_diagnostics(&direct_diagnostics);
                let mut calls = compact_calls(direct_calls);
                annotate_direct_semantic_calls(&mut calls, resolver, &identities);
                functions.push(LinkedIrFunction {
                    source: source.to_owned(),
                    identity: function_identity.clone(),
                    selection,
                    member: symbol.member.clone(),
                    symbol: symbol.name.clone(),
                    binding,
                    address: symbol.addresses_resolved.then_some(symbol.address as u32),
                    object_offset: symbol.address as u32,
                    size: symbol.bytes.len(),
                    flow_kind: "unavailable",
                    complete: false,
                    exact: false,
                    return_value: "unknown".to_owned(),
                    return_provenance: return_provenance(
                        &SymbolicValue::Unknown,
                        &BTreeMap::new(),
                        svd,
                    ),
                    dependencies: Vec::new(),
                    calls,
                    direct_mmio_predicates,
                    mmio_accesses: Vec::new(),
                    delays: Vec::new(),
                    context_accesses: Vec::new(),
                    context_fields: Vec::new(),
                    effect_summary: LinkedEffectSummary::default(),
                    call_graph_diagnostics,
                    direct_diagnostics,
                    reference_diagnostics: Vec::new(),
                    call_graph_blockers,
                    direct_blockers,
                    reference_blockers: Vec::new(),
                    pseudo: format!(
                        "// vendor symbol: {function_identity}\n// DECODE-BLOCKER: {error}\nfn {}(args: [u32; 16]) -> u32 {{ unknown }}\n",
                        pseudo_identifier(&function_identity)
                    ),
                });
            }
        }
    }

    summarize_linked_ir(functions)
}

pub(crate) fn merge_linked_ir(reports: Vec<LinkedIrReport>) -> LinkedIrReport {
    let functions = reports
        .into_iter()
        .flat_map(|report| report.functions)
        .collect();
    summarize_linked_ir(functions)
}

pub(crate) fn link_project_calls(reports: &mut [LinkedIrReport]) {
    let mut exported_definitions = BTreeMap::<String, BTreeSet<String>>::new();
    for function in reports.iter().flat_map(|report| &report.functions) {
        if function.binding == "global-or-weak" {
            exported_definitions
                .entry(function.symbol.clone())
                .or_default()
                .insert(function.identity.clone());
        }
    }

    for function in reports.iter_mut().flat_map(|report| &mut report.functions) {
        let mut project_notes = Vec::new();
        let mut linked_dependencies = Vec::new();
        for call in &mut function.calls {
            let Some(symbol) = call.project_symbol.as_ref() else {
                continue;
            };
            let candidates = exported_definitions
                .get(symbol)
                .map(|definitions| definitions.iter().cloned().collect::<Vec<_>>())
                .unwrap_or_default();
            call.project_candidates = candidates.clone();
            match candidates.as_slice() {
                [target] => {
                    call.kind = "project-linked";
                    call.target = target.clone();
                    call.semantics = Some(
                        "unique exported project definition; edge linked without substituting callee arguments, returns or addresses"
                            .to_owned(),
                    );
                    linked_dependencies.push(target.clone());
                    project_notes.push(format!(
                        "// PROJECT-LINKED-CALL: {symbol} -> {target}; reachable effects are inventoried without argument substitution"
                    ));
                }
                [] => {
                    call.semantics = Some(
                        "unresolved project call; no exported definition was found".to_owned(),
                    );
                }
                _ => {
                    call.semantics = Some(
                        "ambiguous project call; multiple exported definitions were found"
                            .to_owned(),
                    );
                    project_notes.push(format!(
                        "// PROJECT-AMBIGUOUS-CALL: {symbol} -> {}",
                        candidates.join(" | ")
                    ));
                }
            }
        }
        function.dependencies.extend(linked_dependencies);
        function.dependencies.sort();
        function.dependencies.dedup();
        function.calls.sort();
        if !project_notes.is_empty() {
            project_notes.sort();
            project_notes.dedup();
            function.pseudo = format!("{}\n{}", project_notes.join("\n"), function.pseudo);
        }
    }
}

fn recursive_call_graph_nodes(adjacency: &[Vec<usize>]) -> BTreeSet<usize> {
    let mut visited = vec![false; adjacency.len()];
    let mut finished = Vec::with_capacity(adjacency.len());
    for start in 0..adjacency.len() {
        if visited[start] {
            continue;
        }
        visited[start] = true;
        let mut stack = vec![(start, 0_usize)];
        while let Some((node, next_target)) = stack.last_mut() {
            if *next_target < adjacency[*node].len() {
                let target = adjacency[*node][*next_target];
                *next_target += 1;
                if !visited[target] {
                    visited[target] = true;
                    stack.push((target, 0));
                }
            } else {
                let (node, _) = stack.pop().expect("DFS stack is non-empty");
                finished.push(node);
            }
        }
    }

    let mut reverse = vec![Vec::new(); adjacency.len()];
    for (source, targets) in adjacency.iter().enumerate() {
        for &target in targets {
            reverse[target].push(source);
        }
    }
    let mut assigned = vec![false; adjacency.len()];
    let mut recursive = BTreeSet::new();
    for &start in finished.iter().rev() {
        if assigned[start] {
            continue;
        }
        assigned[start] = true;
        let mut component = Vec::new();
        let mut stack = vec![start];
        while let Some(node) = stack.pop() {
            component.push(node);
            for &target in &reverse[node] {
                if !assigned[target] {
                    assigned[target] = true;
                    stack.push(target);
                }
            }
        }
        let self_recursive = component.len() == 1
            && adjacency[component[0]]
                .iter()
                .any(|target| *target == component[0]);
        if component.len() > 1 || self_recursive {
            recursive.extend(component);
        }
    }
    recursive
}

#[derive(Default)]
struct SummaryMmioAccumulator {
    access_shapes: usize,
    accesses: BTreeSet<&'static str>,
    modes: BTreeSet<&'static str>,
    origins: BTreeSet<String>,
}

#[derive(Default)]
struct SummaryDelayAccumulator {
    delay_shapes: usize,
    origins: BTreeSet<String>,
}

#[derive(Default)]
struct SummarySemanticAccumulator {
    call_shapes: usize,
    targets: BTreeSet<String>,
    replacement_hints: BTreeSet<String>,
    origins: BTreeSet<String>,
}

#[derive(Clone)]
struct SummaryCallEdge {
    target: usize,
    site: Option<u32>,
    bindings: Vec<LinkedArgumentBinding>,
    guard_paths: Option<Vec<LinkedCallGuardPath>>,
}

#[derive(Default)]
struct SummaryContextAccumulator {
    read_shapes: BTreeSet<String>,
    write_shapes: BTreeSet<ContextWriteShape>,
    write_mask: u32,
    origins: BTreeSet<String>,
    paths: BTreeSet<String>,
    write_values: BTreeSet<String>,
}

#[derive(Eq, Ord, PartialEq, PartialOrd)]
struct ContextWriteShape {
    path: String,
    value: Option<String>,
    write_mask: Option<u32>,
    preserved_mask: Option<u32>,
    forced_zero_mask: Option<u32>,
    forced_one_mask: Option<u32>,
}

struct ContextProjectionState {
    function: usize,
    argument_map: Vec<Option<(u8, i32)>>,
    visited_functions: Vec<usize>,
    site_path: Vec<Option<u32>>,
    guard_scopes: Option<Vec<LinkedCallGuardScope>>,
    path: String,
}

fn local_context_access(access: &ContextAccess) -> bool {
    !access
        .path
        .split(" / ")
        .any(|component| component.starts_with("call "))
}

fn project_call_arguments(
    call: &LinkedCall,
    argument_map: &[Option<(u8, i32)>],
    blockers: &mut BTreeSet<String>,
    boundary: &str,
    path: &str,
) -> Vec<LinkedProjectedCallArgument> {
    call.typed_arguments
        .iter()
        .map(|argument| {
            let affine = call
                .argument_bindings
                .iter()
                .find(|binding| binding.position == argument.position);
            let pointer = argument.c_type.contains('*');
            let (binding, root_argument, root_offset) = if !pointer {
                ("non-pointer", None, None)
            } else if let Some(affine) = affine {
                match argument_map
                    .get(usize::from(affine.caller_argument))
                    .copied()
                    .flatten()
                    .and_then(|(argument, offset)| {
                        offset
                            .checked_add(affine.offset)
                            .map(|offset| (argument, offset))
                    }) {
                    Some((argument, offset)) => {
                        ("affine-root-context", Some(argument), Some(offset))
                    }
                    None => {
                        blockers.insert(format!(
                            "semantic pointer binding cannot reach root: {boundary} arg{} along {path}",
                            argument.position
                        ));
                        ("affine-origin-context-unavailable", None, None)
                    }
                }
            } else {
                ("not-affine-caller-context", None, None)
            };
            LinkedProjectedCallArgument {
                position: argument.position,
                name: argument.name.clone(),
                c_type: argument.c_type.clone(),
                direction: argument.direction,
                value: argument.value.clone(),
                binding,
                root_argument,
                root_offset,
            }
        })
        .collect()
}

fn extend_guard_scopes(
    scopes: Option<&[LinkedCallGuardScope]>,
    function: &str,
    guard_paths: Option<&[LinkedCallGuardPath]>,
) -> Option<Vec<LinkedCallGuardScope>> {
    let mut scopes = scopes?.to_vec();
    let paths = guard_paths?;
    if paths.iter().any(|path| path.guards.is_empty()) {
        return Some(scopes);
    }
    scopes.push(LinkedCallGuardScope {
        function: function.to_owned(),
        paths: paths.to_vec(),
    });
    Some(scopes)
}

fn project_event_dispatches(actions: &[LinkedProjectedSemanticAction]) -> Vec<LinkedEventDispatch> {
    actions
        .iter()
        .enumerate()
        .filter_map(|(semantic_action_index, action)| {
            let spec = action.contract.as_ref()?.event_dispatch.as_ref()?;
            let mut blockers = BTreeSet::new();
            if spec.mechanism.is_empty() {
                blockers.insert("event dispatch mechanism is empty".to_owned());
            }
            if spec.execution_context.is_empty() {
                blockers.insert("event dispatch execution context is empty".to_owned());
            }
            let expected_names = spec
                .argument_roles
                .iter()
                .map(|binding| binding.argument)
                .collect::<BTreeSet<_>>();
            for argument in &action.arguments {
                if !expected_names.contains(argument.name.as_str()) {
                    blockers.insert(format!(
                        "unexpected semantic argument {} at position {}",
                        argument.name, argument.position
                    ));
                }
            }
            let mut bindings = Vec::new();
            let mut declared_roles = BTreeSet::new();
            let mut declared_arguments = BTreeSet::new();
            for binding in &spec.argument_roles {
                let role = binding.role;
                let name = binding.argument;
                if !declared_roles.insert(role) {
                    blockers.insert(format!("duplicate event role {role}"));
                }
                if !declared_arguments.insert(name) {
                    blockers.insert(format!("duplicate event argument {name}"));
                }
                if role.is_empty() {
                    blockers.insert(format!("semantic argument {name} has an empty event role"));
                }
                if name.is_empty() {
                    blockers.insert(format!("event role {role} has an empty semantic argument"));
                }
                let matching = action
                    .arguments
                    .iter()
                    .filter(|argument| argument.name == name)
                    .collect::<Vec<_>>();
                match matching.as_slice() {
                    [argument] => bindings.push(LinkedEventDispatchBinding {
                        role,
                        argument: (*argument).clone(),
                    }),
                    [] => {
                        blockers
                            .insert(format!("missing semantic argument {name} for role {role}"));
                    }
                    _ => {
                        blockers.insert(format!(
                            "ambiguous semantic argument {name} for role {role}"
                        ));
                    }
                }
            }
            Some(LinkedEventDispatch {
                semantic_action_index,
                mechanism: spec.mechanism,
                execution_context: spec.execution_context,
                receiver: spec.receiver.map(str::to_owned),
                interface_complete: blockers.is_empty(),
                blockers: blockers.into_iter().collect(),
                bindings,
            })
        })
        .collect()
}

fn project_context_fields(
    root: usize,
    functions: &[LinkedIrFunction],
    call_edges: &[Vec<SummaryCallEdge>],
    projection_reachable: &[bool],
    call_graph_closed: bool,
) -> (
    bool,
    Vec<String>,
    Vec<LinkedSummaryContextField>,
    Vec<LinkedProjectedTrampolineCall>,
    Vec<LinkedProjectedSemanticAction>,
    Vec<LinkedEventDispatch>,
) {
    let mut root_arguments = vec![None; usize::from(LINKED_CONTEXT_ARGUMENTS)];
    for argument in 0..LINKED_CONTEXT_ARGUMENTS {
        root_arguments[usize::from(argument)] = Some((argument, 0));
    }
    let mut queue = VecDeque::from([ContextProjectionState {
        function: root,
        argument_map: root_arguments,
        visited_functions: vec![root],
        site_path: Vec::new(),
        guard_scopes: Some(Vec::new()),
        path: functions[root].identity.clone(),
    }]);
    let mut blockers = BTreeSet::new();
    let mut fields = BTreeMap::<(u8, i32, u8), SummaryContextAccumulator>::new();
    let mut trampoline_calls = BTreeSet::new();
    let mut semantic_actions = BTreeSet::new();
    let mut explored = 0_usize;

    while let Some(state) = queue.pop_front() {
        if explored >= MAX_CONTEXT_PROJECTION_STATES {
            blockers.insert(format!(
                "context projection exceeds {MAX_CONTEXT_PROJECTION_STATES} simple-path states"
            ));
            break;
        }
        explored += 1;
        let function = &functions[state.function];
        for call in function
            .calls
            .iter()
            .filter(|call| call.semantic_operation.is_some())
        {
            let operation = call
                .semantic_operation
                .as_ref()
                .expect("filtered semantic call")
                .clone();
            let site = call
                .site
                .map_or_else(|| "composed".to_owned(), |site| format!("{site:#010x}"));
            let boundary = format!("{operation} via {}", call.target);
            let mut site_path = state.site_path.clone();
            site_path.push(call.site);
            semantic_actions.insert(LinkedProjectedSemanticAction {
                site_path,
                path: format!("{} --semantic@{}--> {}", state.path, site, call.target),
                operation,
                target: call.target.clone(),
                contract: call.semantic_contract.clone(),
                replacement_hint: call.replacement_hint.clone(),
                origin: function.identity.clone(),
                site: call.site,
                argument_shapes: call.argument_shapes,
                arguments: project_call_arguments(
                    call,
                    &state.argument_map,
                    &mut blockers,
                    &boundary,
                    &state.path,
                ),
                guard_scopes: extend_guard_scopes(
                    state.guard_scopes.as_deref(),
                    &function.identity,
                    call.guard_paths.as_deref(),
                ),
            });
        }
        for call in function
            .calls
            .iter()
            .filter(|call| call.trampoline.is_some())
        {
            let trampoline = call
                .trampoline
                .as_ref()
                .expect("filtered trampoline call")
                .clone();
            let boundary = format!("{} {}", trampoline.table, trampoline.c_name);
            let arguments = project_call_arguments(
                call,
                &state.argument_map,
                &mut blockers,
                &boundary,
                &state.path,
            );
            trampoline_calls.insert(LinkedProjectedTrampolineCall {
                path: format!(
                    "{} --trampoline@{}+{:#x}--> {}",
                    state.path, trampoline.table, trampoline.slot, trampoline.c_name
                ),
                trampoline,
                origin: function.identity.clone(),
                argument_shapes: call.argument_shapes,
                arguments,
            });
        }
        for access in function
            .context_accesses
            .iter()
            .filter(|access| local_context_access(access))
        {
            let Some((root_argument, base_offset)) = state
                .argument_map
                .get(usize::from(access.argument))
                .copied()
                .flatten()
            else {
                blockers.insert(format!(
                    "no affine binding for {} arg{} along {}",
                    function.identity, access.argument, state.path
                ));
                continue;
            };
            let Some(offset) = base_offset.checked_add(access.offset) else {
                blockers.insert(format!(
                    "context offset overflow for {} arg{} along {}",
                    function.identity, access.argument, state.path
                ));
                continue;
            };
            let field = fields
                .entry((root_argument, offset, access.width))
                .or_default();
            match access.access {
                "read" => {
                    field.read_shapes.insert(access.path.clone());
                }
                "write" => {
                    field.write_shapes.insert(ContextWriteShape {
                        path: access.path.clone(),
                        value: access.value.clone(),
                        write_mask: access.write_mask,
                        preserved_mask: access.preserved_mask,
                        forced_zero_mask: access.forced_zero_mask,
                        forced_one_mask: access.forced_one_mask,
                    });
                    field.write_mask |= access.write_mask.unwrap_or_default();
                    if let Some(value) = access.value_pseudo.as_ref() {
                        field.write_values.insert(value.clone());
                    }
                }
                _ => unreachable!("context access has a closed access vocabulary"),
            }
            field.origins.insert(function.identity.clone());
            field
                .paths
                .insert(format!("{} / {}", state.path, access.path));
        }

        for edge in &call_edges[state.function] {
            if !projection_reachable[edge.target] {
                continue;
            }
            if state.visited_functions.contains(&edge.target) {
                blockers.insert(format!(
                    "recursive context projection stopped: {} -> {}",
                    function.identity, functions[edge.target].identity
                ));
                continue;
            }
            let mut argument_map = vec![None; usize::from(LINKED_CONTEXT_ARGUMENTS)];
            for binding in &edge.bindings {
                if binding.position >= argument_map.len() {
                    continue;
                }
                let Some((root_argument, caller_offset)) = state
                    .argument_map
                    .get(usize::from(binding.caller_argument))
                    .copied()
                    .flatten()
                else {
                    continue;
                };
                let Some(offset) = caller_offset.checked_add(binding.offset) else {
                    blockers.insert(format!(
                        "call argument offset overflow: {} -> {} arg{}",
                        function.identity, functions[edge.target].identity, binding.position
                    ));
                    continue;
                };
                argument_map[binding.position] = Some((root_argument, offset));
            }
            let mut visited_functions = state.visited_functions.clone();
            visited_functions.push(edge.target);
            let mut site_path = state.site_path.clone();
            site_path.push(edge.site);
            let guard_scopes = extend_guard_scopes(
                state.guard_scopes.as_deref(),
                &function.identity,
                edge.guard_paths.as_deref(),
            );
            let site = edge
                .site
                .map_or_else(|| "composed".to_owned(), |site| format!("{site:#010x}"));
            queue.push_back(ContextProjectionState {
                function: edge.target,
                argument_map,
                visited_functions,
                site_path,
                guard_scopes,
                path: format!(
                    "{} --call@{}--> {}",
                    state.path, site, functions[edge.target].identity
                ),
            });
        }
    }

    let fields = fields
        .into_iter()
        .map(
            |((argument, offset, width), field)| LinkedSummaryContextField {
                argument,
                offset,
                width,
                reads: field.read_shapes.len(),
                writes: field.write_shapes.len(),
                write_mask: field.write_mask,
                origins: field.origins.into_iter().collect(),
                paths: field.paths.into_iter().collect(),
                write_values: field.write_values.into_iter().collect(),
            },
        )
        .collect();
    let semantic_actions = semantic_actions.into_iter().collect::<Vec<_>>();
    let event_dispatches = project_event_dispatches(&semantic_actions);
    (
        call_graph_closed && blockers.is_empty(),
        blockers.into_iter().collect(),
        fields,
        trampoline_calls.into_iter().collect(),
        semantic_actions,
        event_dispatches,
    )
}

fn projection_reachability(functions: &[LinkedIrFunction], adjacency: &[Vec<usize>]) -> Vec<bool> {
    let mut reverse = vec![Vec::new(); functions.len()];
    for (source, targets) in adjacency.iter().enumerate() {
        for &target in targets {
            reverse[target].push(source);
        }
    }
    let mut reachable = functions
        .iter()
        .map(|function| {
            function.context_accesses.iter().any(local_context_access)
                || function.calls.iter().any(|call| call.trampoline.is_some())
                || function
                    .calls
                    .iter()
                    .any(|call| call.semantic_operation.is_some())
        })
        .collect::<Vec<_>>();
    let mut queue = reachable
        .iter()
        .enumerate()
        .filter_map(|(index, reachable)| reachable.then_some(index))
        .collect::<VecDeque<_>>();
    while let Some(target) = queue.pop_front() {
        for &source in &reverse[target] {
            if !reachable[source] {
                reachable[source] = true;
                queue.push_back(source);
            }
        }
    }
    reachable
}

fn populate_effect_summaries(functions: &mut [LinkedIrFunction]) {
    let identities = functions
        .iter()
        .enumerate()
        .map(|(index, function)| (function.identity.clone(), index))
        .collect::<BTreeMap<_, _>>();
    let mut source_symbols = BTreeMap::<(String, String), Vec<usize>>::new();
    for (index, function) in functions.iter().enumerate() {
        source_symbols
            .entry((function.source.clone(), function.symbol.clone()))
            .or_default()
            .push(index);
    }

    let mut adjacency = vec![Vec::new(); functions.len()];
    let mut call_edges = vec![Vec::new(); functions.len()];
    let mut local_blockers = vec![BTreeSet::<String>::new(); functions.len()];
    for (index, function) in functions.iter().enumerate() {
        if !function.complete {
            local_blockers[index]
                .insert(format!("incomplete function body: {}", function.identity));
        }
        for blocker in &function.call_graph_blockers {
            local_blockers[index].insert(format!("{}: {blocker}", function.identity));
        }
        for call in &function.calls {
            match call.kind {
                "internal" | "project-linked" => {
                    let target = identities.get(&call.target).copied().or_else(|| {
                        let candidates =
                            source_symbols.get(&(function.source.clone(), call.target.clone()))?;
                        (candidates.len() == 1).then_some(candidates[0])
                    });
                    if let Some(target) = target {
                        adjacency[index].push(target);
                        call_edges[index].push(SummaryCallEdge {
                            target,
                            site: call.site,
                            bindings: call.argument_bindings.clone(),
                            guard_paths: call.guard_paths.clone(),
                        });
                    } else {
                        local_blockers[index].insert(format!(
                            "callee is outside the exported IR: {} -> {}",
                            function.identity, call.target
                        ));
                    }
                }
                "unresolved" => {
                    local_blockers[index].insert(format!(
                        "unresolved call edge: {} -> {}",
                        function.identity, call.target
                    ));
                }
                "external" | "diagnostic" if call.semantic_operation.is_none() => {
                    local_blockers[index].insert(format!(
                        "opaque semantic boundary: {} -> {}",
                        function.identity, call.target
                    ));
                }
                "external" | "diagnostic" => {}
                kind => {
                    local_blockers[index].insert(format!(
                        "unsupported call edge {kind}: {} -> {}",
                        function.identity, call.target
                    ));
                }
            }
        }
        adjacency[index].sort_unstable();
        adjacency[index].dedup();
    }
    let recursive_nodes = recursive_call_graph_nodes(&adjacency);
    let projection_reachable = projection_reachability(functions, &adjacency);

    let summaries = (0..functions.len())
        .map(|root| {
            let mut depths = vec![None; functions.len()];
            depths[root] = Some(0);
            let mut queue = VecDeque::from([root]);
            while let Some(source) = queue.pop_front() {
                let next_depth = depths[source].expect("queued function has a depth") + 1;
                for &target in &adjacency[source] {
                    if depths[target].is_none() {
                        depths[target] = Some(next_depth);
                        queue.push_back(target);
                    }
                }
            }
            let reachable = depths
                .iter()
                .enumerate()
                .filter_map(|(index, depth)| depth.map(|depth| (index, depth)))
                .collect::<Vec<_>>();
            let mut blockers = BTreeSet::new();
            let mut mmio = BTreeMap::<(u32, u8), SummaryMmioAccumulator>::new();
            let mut delays = BTreeMap::<(String, Option<u32>), SummaryDelayAccumulator>::new();
            let mut semantics = BTreeMap::<String, SummarySemanticAccumulator>::new();
            for &(index, _) in &reachable {
                let function = &functions[index];
                blockers.extend(local_blockers[index].iter().cloned());
                for access in &function.mmio_accesses {
                    let entry = mmio.entry((access.address, access.width)).or_default();
                    entry.access_shapes += 1;
                    entry.accesses.insert(access.access);
                    entry.modes.insert(access.mode);
                    entry.origins.insert(function.identity.clone());
                }
                for delay in &function.delays {
                    let entry = delays
                        .entry((delay.micros.clone(), delay.constant_micros))
                        .or_default();
                    entry.delay_shapes += 1;
                    entry.origins.insert(function.identity.clone());
                }
                for call in &function.calls {
                    let Some(operation) = call.semantic_operation.as_ref() else {
                        continue;
                    };
                    let entry = semantics.entry(operation.clone()).or_default();
                    entry.call_shapes += call.argument_shapes;
                    entry.targets.insert(call.target.clone());
                    if let Some(replacement) = call.replacement_hint.as_ref() {
                        entry.replacement_hints.insert(replacement.clone());
                    }
                    entry.origins.insert(function.identity.clone());
                }
            }
            let reachable_functions = reachable
                .iter()
                .filter(|(index, _)| *index != root)
                .map(|(index, _)| functions[*index].identity.clone())
                .collect();
            let recursive_functions = reachable
                .iter()
                .filter(|(index, _)| recursive_nodes.contains(index))
                .map(|(index, _)| functions[*index].identity.clone())
                .collect();
            let call_graph_closed = blockers.is_empty();
            let (
                context_projection_complete,
                context_projection_blockers,
                context_fields,
                trampoline_calls,
                semantic_actions,
                event_dispatches,
            ) = project_context_fields(
                root,
                functions,
                &call_edges,
                &projection_reachable,
                call_graph_closed,
            );
            LinkedEffectSummary {
                call_graph_closed,
                max_depth: reachable
                    .iter()
                    .map(|(_, depth)| *depth)
                    .max()
                    .unwrap_or_default(),
                reachable_functions,
                recursive_functions,
                blockers: blockers.into_iter().collect(),
                mmio_registers: mmio
                    .into_iter()
                    .map(|((address, width), entry)| LinkedSummaryMmio {
                        address,
                        width,
                        access_shapes: entry.access_shapes,
                        accesses: entry.accesses.into_iter().collect(),
                        modes: entry.modes.into_iter().collect(),
                        origins: entry.origins.into_iter().collect(),
                    })
                    .collect(),
                delays: delays
                    .into_iter()
                    .map(|((micros, constant_micros), entry)| LinkedSummaryDelay {
                        micros,
                        constant_micros,
                        delay_shapes: entry.delay_shapes,
                        origins: entry.origins.into_iter().collect(),
                    })
                    .collect(),
                semantic_operations: semantics
                    .into_iter()
                    .map(|(operation, entry)| LinkedSummarySemantic {
                        operation,
                        call_shapes: entry.call_shapes,
                        targets: entry.targets.into_iter().collect(),
                        replacement_hints: entry.replacement_hints.into_iter().collect(),
                        origins: entry.origins.into_iter().collect(),
                    })
                    .collect(),
                context_projection_complete,
                context_projection_blockers,
                context_fields,
                trampoline_calls,
                semantic_actions,
                event_dispatches,
            }
        })
        .collect::<Vec<_>>();
    for (function, summary) in functions.iter_mut().zip(summaries) {
        function.effect_summary = summary;
    }
}

#[derive(Default)]
struct MmioFieldCandidateAccumulator {
    write_shapes: usize,
    predicate_shapes: usize,
    poll_shapes: usize,
    functions: BTreeSet<String>,
    access_functions: BTreeSet<String>,
    predicate_functions: BTreeSet<String>,
    predicate_evidence: BTreeSet<LinkedMmioFieldPredicateEvidence>,
    semantic_operations: BTreeSet<String>,
    semantic_roots: BTreeSet<String>,
    semantic_evidence: BTreeSet<LinkedMmioFieldSemanticEvidence>,
}

#[derive(Default)]
struct MmioRegisterAccumulator {
    names: BTreeSet<String>,
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
    write_masks: BTreeSet<u32>,
    predicate_masks: BTreeSet<u32>,
    poll_masks: BTreeSet<u32>,
    candidate_bit_ranges: BTreeMap<(u8, u8, u32), (usize, BTreeSet<String>)>,
    field_candidates: BTreeMap<(u8, u8, u32), MmioFieldCandidateAccumulator>,
    functions: BTreeSet<String>,
}

fn candidate_bit_ranges(mask: u32, width: u8) -> Vec<(u8, u8, u32)> {
    let mask = mask & width_mask(width);
    let mut output = Vec::new();
    let mut bit = 0_u8;
    while bit < width {
        if mask & (1_u32 << bit) == 0 {
            bit += 1;
            continue;
        }
        let first = bit;
        while bit + 1 < width && mask & (1_u32 << (bit + 1)) != 0 {
            bit += 1;
        }
        let last = bit;
        let range_width = last - first + 1;
        let range_mask = if range_width == 32 {
            u32::MAX
        } else {
            ((1_u32 << range_width) - 1) << first
        };
        output.push((first, last, range_mask));
        bit += 1;
    }
    output
}

fn record_access_field_mask(
    entry: &mut MmioRegisterAccumulator,
    mask: u32,
    width: u8,
    function: &str,
    access: &'static str,
    evidence: Option<LinkedMmioFieldPredicateEvidence>,
) {
    let full_mask = width_mask(width);
    let mask = mask & full_mask;
    if mask == 0 || mask == full_mask {
        return;
    }
    for range in candidate_bit_ranges(mask, width) {
        let candidate = entry.field_candidates.entry(range).or_default();
        match access {
            "write" => candidate.write_shapes += 1,
            "poll" => candidate.poll_shapes += 1,
            _ => unreachable!("field access evidence has a closed vocabulary"),
        }
        candidate.functions.insert(function.to_owned());
        candidate.access_functions.insert(function.to_owned());
        if let Some(mut evidence) = evidence.clone() {
            evidence.register_comparison_value = evidence
                .register_comparison_value
                .map(|value| value & range.2);
            candidate.predicate_functions.insert(function.to_owned());
            candidate.predicate_evidence.insert(evidence);
        }
    }
}

fn record_predicate_field_mask(
    entry: &mut MmioRegisterAccumulator,
    mask: u32,
    width: u8,
    predicate_function: &str,
    evidence: &[LinkedMmioFieldPredicateEvidence],
) {
    let full_mask = width_mask(width);
    let mask = mask & full_mask;
    if mask == 0 || mask == full_mask {
        return;
    }
    for range in candidate_bit_ranges(mask, width) {
        let candidate = entry.field_candidates.entry(range).or_default();
        candidate.predicate_shapes += 1;
        candidate.functions.insert(predicate_function.to_owned());
        candidate
            .predicate_functions
            .insert(predicate_function.to_owned());
        for evidence in evidence {
            let mut evidence = evidence.clone();
            evidence.register_comparison_value = evidence
                .register_comparison_value
                .map(|value| value & range.2);
            candidate
                .functions
                .extend(evidence.producer_path.iter().cloned());
            if let Some(producer) = evidence.producer_path.last() {
                candidate.access_functions.insert(producer.clone());
            }
            candidate.predicate_evidence.insert(evidence);
        }
    }
}

struct SemanticFieldEvidence<'a> {
    kind: &'static str,
    mask: u32,
    width: u8,
    operation: &'a str,
    root: &'a str,
    action_target: &'a str,
    action_origin: &'a str,
    action_site: Option<u32>,
    action_site_path: &'a [Option<u32>],
    action_path: &'a str,
    predicate_function: &'a str,
    producer: Option<&'a str>,
    producer_path: &'a [String],
    scope_index: usize,
    scope_alternatives: usize,
    path_index: usize,
    path_expression: &'a str,
    path_guards: usize,
    guard_index: usize,
    residual_path_expression: &'a str,
    site: u32,
    condition: &'a str,
    taken: bool,
    guard_operation: &'static str,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct SemanticFieldLink {
    kind: &'static str,
    address: u32,
    register_bits: u32,
    root: String,
    operation: String,
    action_target: String,
    action_origin: String,
    action_site: Option<u32>,
    action_site_path: Vec<Option<u32>>,
    action_path: String,
    predicate_function: String,
    producer: Option<String>,
    producer_path: Vec<String>,
    scope_index: usize,
    scope_alternatives: usize,
    path_index: usize,
    path_expression: String,
    path_guards: usize,
    guard_index: usize,
    residual_path_expression: String,
    site: u32,
    condition: String,
    guard_operation: &'static str,
    taken: bool,
}

fn record_semantic_field_link(
    entry: &mut MmioRegisterAccumulator,
    evidence: SemanticFieldEvidence<'_>,
) {
    let full_mask = width_mask(evidence.width);
    let mask = evidence.mask & full_mask;
    if mask == 0 || mask == full_mask {
        return;
    }
    for range in candidate_bit_ranges(mask, evidence.width) {
        let candidate = entry.field_candidates.entry(range).or_default();
        candidate
            .semantic_operations
            .insert(evidence.operation.to_owned());
        candidate.semantic_roots.insert(evidence.root.to_owned());
        candidate
            .semantic_evidence
            .insert(LinkedMmioFieldSemanticEvidence {
                kind: evidence.kind,
                root: evidence.root.to_owned(),
                operation: evidence.operation.to_owned(),
                action_target: evidence.action_target.to_owned(),
                action_origin: evidence.action_origin.to_owned(),
                action_site: evidence.action_site,
                action_site_path: evidence.action_site_path.to_vec(),
                action_path: evidence.action_path.to_owned(),
                predicate_function: evidence.predicate_function.to_owned(),
                producer: evidence.producer.map(str::to_owned),
                producer_path: evidence.producer_path.to_vec(),
                scope_index: evidence.scope_index,
                scope_alternatives: evidence.scope_alternatives,
                path_index: evidence.path_index,
                path_expression: evidence.path_expression.to_owned(),
                path_guards: evidence.path_guards,
                guard_index: evidence.guard_index,
                residual_path_expression: evidence.residual_path_expression.to_owned(),
                site: evidence.site,
                condition: evidence.condition.to_owned(),
                taken: evidence.taken,
                effective_operation: effective_branch_operation(
                    evidence.guard_operation,
                    evidence.taken,
                ),
            });
        candidate
            .predicate_functions
            .insert(evidence.predicate_function.to_owned());
        candidate
            .functions
            .insert(evidence.predicate_function.to_owned());
        candidate
            .functions
            .extend(evidence.producer_path.iter().cloned());
        if let Some(producer) = evidence.producer_path.last() {
            candidate.access_functions.insert(producer.clone());
        }
    }
}

fn unique_mmio_widths(
    index: &BTreeMap<(u32, u8), MmioRegisterAccumulator>,
) -> BTreeMap<u32, Option<u8>> {
    let mut widths = BTreeMap::new();
    for &(address, width) in index.keys() {
        widths
            .entry(address)
            .and_modify(|known| {
                if *known != Some(width) {
                    *known = None;
                }
            })
            .or_insert(Some(width));
    }
    widths
}

fn summarize_linked_ir(mut functions: Vec<LinkedIrFunction>) -> LinkedIrReport {
    functions.sort_by(|left, right| left.identity.cmp(&right.identity));
    link_guard_result_mmio_sources(&mut functions);
    populate_effect_summaries(&mut functions);
    let mmio_functions = functions
        .iter()
        .filter(|function| !function.mmio_accesses.is_empty())
        .count();
    let mmio_access_shapes = functions
        .iter()
        .map(|function| function.mmio_accesses.len())
        .sum();
    let delay_functions = functions
        .iter()
        .filter(|function| !function.delays.is_empty())
        .count();
    let delay_shapes = functions.iter().map(|function| function.delays.len()).sum();
    let mut mmio_index = BTreeMap::<(u32, u8), MmioRegisterAccumulator>::new();
    for function in &functions {
        for access in &function.mmio_accesses {
            let entry = mmio_index
                .entry((access.address, access.width))
                .or_default();
            entry.names.insert(access.register.clone());
            match access.access {
                "read" => entry.read_shapes += 1,
                "write" => {
                    entry.write_shapes += 1;
                    let modified_mask = access.modified_mask.unwrap_or(width_mask(access.width));
                    entry.write_masks.insert(modified_mask);
                    entry.whole_register_write_shapes +=
                        usize::from(modified_mask == width_mask(access.width));
                    let register_derived_mask = access.preserved_mask.unwrap_or_default()
                        | access.inverted_mask.unwrap_or_default()
                        | access.read_derived_mask.unwrap_or_default();
                    entry.read_modify_write_shapes += usize::from(register_derived_mask != 0);
                    for range in candidate_bit_ranges(modified_mask, access.width) {
                        let candidate = entry.candidate_bit_ranges.entry(range).or_default();
                        candidate.0 += 1;
                        candidate.1.insert(function.identity.clone());
                    }
                    record_access_field_mask(
                        entry,
                        modified_mask,
                        access.width,
                        &function.identity,
                        "write",
                        None,
                    );
                }
                "poll" => {
                    entry.poll_shapes += 1;
                    let predicate_mask = access
                        .predicate_mask
                        .expect("poll MMIO access has a structured predicate mask")
                        & width_mask(access.width);
                    entry.poll_masks.insert(predicate_mask);
                    entry.whole_register_poll_shapes +=
                        usize::from(predicate_mask == width_mask(access.width));
                    record_access_field_mask(
                        entry,
                        predicate_mask,
                        access.width,
                        &function.identity,
                        "poll",
                        Some(LinkedMmioFieldPredicateEvidence {
                            kind: "poll",
                            function: function.identity.clone(),
                            producer: None,
                            producer_path: Vec::new(),
                            site: None,
                            path: Some(access.path.clone()),
                            condition: access
                                .guard
                                .clone()
                                .expect("poll MMIO access has a predicate expression"),
                            operation: "equal",
                            taken: None,
                            effective_operation: None,
                            operand: Some("read"),
                            comparison_value: access.predicate_expected,
                            register_comparison_value: access.predicate_expected,
                            inverted: false,
                        }),
                    );
                }
                _ => unreachable!("linked MMIO access has a closed access vocabulary"),
            }
            match access.mode {
                "static" => entry.static_shapes += 1,
                "indexed-candidate" => entry.indexed_candidate_shapes += 1,
                _ => unreachable!("linked MMIO access has a closed address-mode vocabulary"),
            }
            entry.functions.insert(function.identity.clone());
        }
    }
    let unique_widths = unique_mmio_widths(&mmio_index);
    for function in &functions {
        for predicate in &function.direct_mmio_predicates {
            for source in &predicate.sources {
                let Some(width) = unique_widths.get(&source.address).copied().flatten() else {
                    continue;
                };
                let entry = mmio_index
                    .get_mut(&(source.address, width))
                    .expect("unique MMIO width comes from the register index");
                let predicate_mask = source.register_bits & width_mask(width);
                entry.predicate_shapes += 1;
                entry.predicate_masks.insert(predicate_mask);
                entry.whole_register_predicate_shapes +=
                    usize::from(predicate_mask == width_mask(width));
                entry.functions.insert(function.identity.clone());
                record_predicate_field_mask(
                    entry,
                    predicate_mask,
                    width,
                    &function.identity,
                    &[LinkedMmioFieldPredicateEvidence {
                        kind: "direct-mmio",
                        function: function.identity.clone(),
                        producer: None,
                        producer_path: vec![function.identity.clone()],
                        site: Some(predicate.site),
                        path: None,
                        condition: predicate.condition.clone(),
                        operation: predicate.operation,
                        taken: None,
                        effective_operation: None,
                        operand: Some(source.operand),
                        comparison_value: source.comparison_value,
                        register_comparison_value: source.register_comparison_value,
                        inverted: source.inverted,
                    }],
                );
            }
        }
    }
    let mut predicate_evidence = BTreeMap::<
        (String, u32, String, Vec<String>, u32, String, u32, u32),
        BTreeSet<LinkedMmioFieldPredicateEvidence>,
    >::new();
    for function in &functions {
        for call in &function.calls {
            let Some(paths) = call.guard_paths.as_deref() else {
                continue;
            };
            for path in paths {
                for guard in &path.guards {
                    for source in &guard.result_sources {
                        let producer = source
                            .target
                            .clone()
                            .unwrap_or_else(|| "unknown".to_owned());
                        for mmio in &source.mmio_sources {
                            predicate_evidence
                                .entry((
                                    function.identity.clone(),
                                    guard.site,
                                    producer.clone(),
                                    mmio.producer_path.clone(),
                                    mmio.address,
                                    mmio.register.clone(),
                                    mmio.result_bits,
                                    mmio.register_bits,
                                ))
                                .or_default()
                                .insert(LinkedMmioFieldPredicateEvidence {
                                    kind: "producer-return",
                                    function: function.identity.clone(),
                                    producer: (producer != "unknown").then_some(producer.clone()),
                                    producer_path: mmio.producer_path.clone(),
                                    site: Some(guard.site),
                                    path: None,
                                    condition: guard.condition.clone(),
                                    operation: guard.operation,
                                    taken: Some(guard.taken),
                                    effective_operation: Some(effective_branch_operation(
                                        guard.operation,
                                        guard.taken,
                                    )),
                                    operand: Some(source.operand),
                                    comparison_value: source.comparison_value,
                                    register_comparison_value: mmio.register_comparison_value,
                                    inverted: mmio.inverted,
                                });
                        }
                    }
                }
            }
        }
    }
    for (
        (
            predicate_function,
            _site,
            _producer,
            producer_path,
            address,
            _register,
            _result_bits,
            register_bits,
        ),
        evidence,
    ) in predicate_evidence
    {
        let Some(width) = unique_widths.get(&address).copied().flatten() else {
            continue;
        };
        let entry = mmio_index
            .get_mut(&(address, width))
            .expect("unique MMIO width comes from the register index");
        let predicate_mask = register_bits & width_mask(width);
        entry.predicate_shapes += 1;
        entry.predicate_masks.insert(predicate_mask);
        entry.whole_register_predicate_shapes += usize::from(predicate_mask == width_mask(width));
        entry.functions.insert(predicate_function.clone());
        entry.functions.extend(producer_path);
        record_predicate_field_mask(
            entry,
            predicate_mask,
            width,
            &predicate_function,
            &evidence.into_iter().collect::<Vec<_>>(),
        );
    }
    let mut semantic_evidence = BTreeSet::<SemanticFieldLink>::new();
    for function in &functions {
        for action in &function.effect_summary.semantic_actions {
            let Some(scopes) = action.guard_scopes.as_deref() else {
                continue;
            };
            for (scope_index, scope) in scopes.iter().enumerate() {
                for (path_index, path) in scope.paths.iter().enumerate() {
                    let path_expression = format_guard_path(path);
                    for (guard_index, guard) in path.guards.iter().enumerate() {
                        let residual_path_expression = format_guard_path_without(path, guard_index);
                        let link = |kind,
                                    address,
                                    register_bits,
                                    producer: Option<String>,
                                    producer_path: Vec<String>| {
                            SemanticFieldLink {
                                kind,
                                address,
                                register_bits,
                                root: function.identity.clone(),
                                operation: action.operation.clone(),
                                action_target: action.target.clone(),
                                action_origin: action.origin.clone(),
                                action_site: action.site,
                                action_site_path: action.site_path.clone(),
                                action_path: action.path.clone(),
                                predicate_function: scope.function.clone(),
                                producer,
                                producer_path,
                                scope_index,
                                scope_alternatives: scope.paths.len(),
                                path_index,
                                path_expression: path_expression.clone(),
                                path_guards: path.guards.len(),
                                guard_index,
                                residual_path_expression: residual_path_expression.clone(),
                                site: guard.site,
                                condition: guard.condition.clone(),
                                guard_operation: guard.operation,
                                taken: guard.taken,
                            }
                        };
                        for mmio in &guard.direct_mmio_sources {
                            semantic_evidence.insert(link(
                                "direct-mmio",
                                mmio.address,
                                mmio.register_bits,
                                Some(scope.function.clone()),
                                vec![scope.function.clone()],
                            ));
                        }
                        for source in &guard.result_sources {
                            for mmio in &source.mmio_sources {
                                semantic_evidence.insert(link(
                                    "producer-return",
                                    mmio.address,
                                    mmio.register_bits,
                                    source.target.clone(),
                                    mmio.producer_path.clone(),
                                ));
                            }
                        }
                    }
                }
            }
        }
    }
    for link in semantic_evidence {
        let Some(width) = unique_widths.get(&link.address).copied().flatten() else {
            continue;
        };
        let entry = mmio_index
            .get_mut(&(link.address, width))
            .expect("unique MMIO width comes from the register index");
        record_semantic_field_link(
            entry,
            SemanticFieldEvidence {
                kind: link.kind,
                mask: link.register_bits,
                width,
                operation: &link.operation,
                root: &link.root,
                action_target: &link.action_target,
                action_origin: &link.action_origin,
                action_site: link.action_site,
                action_site_path: &link.action_site_path,
                action_path: &link.action_path,
                predicate_function: &link.predicate_function,
                producer: link.producer.as_deref(),
                producer_path: &link.producer_path,
                scope_index: link.scope_index,
                scope_alternatives: link.scope_alternatives,
                path_index: link.path_index,
                path_expression: &link.path_expression,
                path_guards: link.path_guards,
                guard_index: link.guard_index,
                residual_path_expression: &link.residual_path_expression,
                site: link.site,
                condition: &link.condition,
                taken: link.taken,
                guard_operation: link.guard_operation,
            },
        );
    }
    let mmio_registers = mmio_index
        .into_iter()
        .map(|((address, width), entry)| LinkedMmioRegister {
            address,
            width,
            names: entry.names.into_iter().collect(),
            read_shapes: entry.read_shapes,
            write_shapes: entry.write_shapes,
            poll_shapes: entry.poll_shapes,
            predicate_shapes: entry.predicate_shapes,
            static_shapes: entry.static_shapes,
            indexed_candidate_shapes: entry.indexed_candidate_shapes,
            whole_register_write_shapes: entry.whole_register_write_shapes,
            whole_register_predicate_shapes: entry.whole_register_predicate_shapes,
            whole_register_poll_shapes: entry.whole_register_poll_shapes,
            read_modify_write_shapes: entry.read_modify_write_shapes,
            write_masks: entry.write_masks.into_iter().collect(),
            predicate_masks: entry.predicate_masks.into_iter().collect(),
            poll_masks: entry.poll_masks.into_iter().collect(),
            candidate_bit_ranges: entry
                .candidate_bit_ranges
                .into_iter()
                .map(
                    |(
                        (least_significant_bit, most_significant_bit, mask),
                        (write_shapes, functions),
                    )| LinkedMmioBitRange {
                        least_significant_bit,
                        most_significant_bit,
                        mask,
                        write_shapes,
                        functions: functions.into_iter().collect(),
                    },
                )
                .collect(),
            field_candidates: entry
                .field_candidates
                .into_iter()
                .map(
                    |((least_significant_bit, most_significant_bit, mask), candidate)| {
                        LinkedMmioFieldCandidate {
                            least_significant_bit,
                            most_significant_bit,
                            mask,
                            write_shapes: candidate.write_shapes,
                            predicate_shapes: candidate.predicate_shapes,
                            poll_shapes: candidate.poll_shapes,
                            functions: candidate.functions.into_iter().collect(),
                            access_functions: candidate.access_functions.into_iter().collect(),
                            predicate_functions: candidate
                                .predicate_functions
                                .into_iter()
                                .collect(),
                            predicate_evidence: candidate.predicate_evidence.into_iter().collect(),
                            semantic_operations: candidate
                                .semantic_operations
                                .into_iter()
                                .collect(),
                            semantic_roots: candidate.semantic_roots.into_iter().collect(),
                            semantic_evidence: candidate.semantic_evidence.into_iter().collect(),
                        }
                    },
                )
                .collect(),
            functions: entry.functions.into_iter().collect(),
        })
        .collect();
    let exported_functions = functions
        .iter()
        .filter(|function| function.binding == "global-or-weak")
        .count();
    let local_functions = functions
        .iter()
        .filter(|function| function.binding == "local")
        .count();
    let context_functions = functions
        .iter()
        .filter(|function| !function.context_accesses.is_empty())
        .count();
    let context_accesses = functions
        .iter()
        .map(|function| function.context_accesses.len())
        .sum();
    let context_fields = functions
        .iter()
        .map(|function| function.context_fields.len())
        .sum();
    let complete_functions = functions
        .iter()
        .filter(|function| function.complete)
        .count();
    let structured_functions = functions
        .iter()
        .filter(|function| function.flow_kind == "structured")
        .count();
    let internal_calls = functions
        .iter()
        .flat_map(|function| &function.calls)
        .filter(|call| call.kind == "internal")
        .count();
    let external_calls = functions
        .iter()
        .flat_map(|function| &function.calls)
        .filter(|call| matches!(call.kind, "external" | "diagnostic"))
        .count();
    let call_argument_shapes = functions
        .iter()
        .flat_map(|function| &function.calls)
        .map(|call| call.argument_shapes)
        .sum();
    let project_linked_calls = functions
        .iter()
        .flat_map(|function| &function.calls)
        .filter(|call| call.kind == "project-linked")
        .count();
    let ambiguous_project_calls = functions
        .iter()
        .flat_map(|function| &function.calls)
        .filter(|call| call.kind == "unresolved" && call.project_candidates.len() > 1)
        .count();
    let unresolved_calls = functions
        .iter()
        .flat_map(|function| &function.calls)
        .filter(|call| call.kind == "unresolved")
        .count();
    let closed_effect_summaries = functions
        .iter()
        .filter(|function| function.effect_summary.call_graph_closed)
        .count();
    let recursive_effect_summaries = functions
        .iter()
        .filter(|function| !function.effect_summary.recursive_functions.is_empty())
        .count();
    let complete_context_projections = functions
        .iter()
        .filter(|function| function.effect_summary.context_projection_complete)
        .count();
    let projected_context_fields = functions
        .iter()
        .map(|function| function.effect_summary.context_fields.len())
        .sum();
    let semantic_calls = functions
        .iter()
        .flat_map(|function| &function.calls)
        .filter(|call| call.semantic_operation.is_some())
        .map(|call| call.argument_shapes)
        .sum();
    let mut semantic_index =
        BTreeMap::<String, (usize, BTreeSet<String>, BTreeSet<String>, BTreeSet<String>)>::new();
    for function in &functions {
        for call in &function.calls {
            let Some(operation) = call.semantic_operation.as_ref() else {
                continue;
            };
            let entry = semantic_index.entry(operation.clone()).or_default();
            entry.0 += call.argument_shapes;
            entry.1.insert(function.identity.clone());
            entry.2.insert(call.target.clone());
            if let Some(replacement) = call.replacement_hint.as_ref() {
                entry.3.insert(replacement.clone());
            }
        }
    }
    let semantic_boundaries = semantic_index
        .into_iter()
        .map(
            |(operation, (call_shapes, functions, targets, replacement_hints))| SemanticBoundary {
                operation,
                call_shapes,
                functions: functions.into_iter().collect(),
                targets: targets.into_iter().collect(),
                replacement_hints: replacement_hints.into_iter().collect(),
            },
        )
        .collect();
    let mut trampoline_index =
        BTreeMap::<LinkedTrampoline, (Vec<LinkedCallArgument>, usize, BTreeSet<String>)>::new();
    for function in &functions {
        for call in &function.calls {
            let Some(trampoline) = call.trampoline.as_ref() else {
                continue;
            };
            let abi_arguments = call
                .typed_arguments
                .iter()
                .cloned()
                .map(|mut argument| {
                    argument.value.clear();
                    argument
                })
                .collect::<Vec<_>>();
            let entry = trampoline_index
                .entry(trampoline.clone())
                .or_insert_with(|| (abi_arguments, 0, BTreeSet::new()));
            entry.1 += call.argument_shapes;
            entry.2.insert(function.identity.clone());
        }
    }
    let trampoline_calls = trampoline_index.values().map(|entry| entry.1).sum();
    let trampoline_slots = trampoline_index
        .into_iter()
        .map(
            |(trampoline, (arguments, call_shapes, functions))| LinkedTrampolineSlot {
                trampoline,
                arguments,
                call_shapes,
                functions: functions.into_iter().collect(),
            },
        )
        .collect();

    LinkedIrReport {
        functions,
        mmio_registers,
        mmio_functions,
        mmio_access_shapes,
        delay_functions,
        delay_shapes,
        semantic_boundaries,
        semantic_calls,
        trampoline_slots,
        trampoline_calls,
        exported_functions,
        local_functions,
        context_functions,
        context_accesses,
        context_fields,
        complete_functions,
        structured_functions,
        internal_calls,
        external_calls,
        call_argument_shapes,
        project_linked_calls,
        ambiguous_project_calls,
        unresolved_calls,
        closed_effect_summaries,
        recursive_effect_summaries,
        complete_context_projections,
        projected_context_fields,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn symbol(name: &str, address: u64, bytes: Vec<u8>) -> artifact::ArtifactSymbolDefinition {
        artifact::ArtifactSymbolDefinition {
            member: Some("member.o".to_owned()),
            name: name.to_owned(),
            address,
            bytes,
            addresses_resolved: false,
            memory_regions: Vec::new(),
            relocations: Vec::new(),
        }
    }

    fn empty_resolver() -> ReferenceResolver {
        ReferenceResolver {
            symbols: Vec::new(),
            symbols_by_address: BTreeMap::new(),
            symbol_ids: BTreeMap::new(),
            exported_symbol_keys: BTreeSet::new(),
            relocated_calls: BTreeMap::new(),
            pointer_context: direct::StructuralPointerContext::default(),
        }
    }

    fn linked_test_function(
        source: &str,
        symbol: &str,
        binding: &'static str,
        calls: Vec<LinkedCall>,
    ) -> LinkedIrFunction {
        LinkedIrFunction {
            source: source.to_owned(),
            identity: format!("{source}::{symbol}"),
            selection: "symbol-prefix-root",
            member: None,
            symbol: symbol.to_owned(),
            binding,
            address: None,
            object_offset: 0,
            size: 4,
            flow_kind: "partial",
            complete: false,
            exact: false,
            return_value: "unknown".to_owned(),
            return_provenance: LinkedReturnProvenance {
                exact: false,
                known_zero_bits: 0,
                known_one_bits: 0,
                unknown_bits: u32::MAX,
                sources: Vec::new(),
            },
            dependencies: Vec::new(),
            calls,
            direct_mmio_predicates: Vec::new(),
            mmio_accesses: Vec::new(),
            delays: Vec::new(),
            context_accesses: Vec::new(),
            context_fields: Vec::new(),
            effect_summary: LinkedEffectSummary::default(),
            call_graph_diagnostics: Vec::new(),
            direct_diagnostics: Vec::new(),
            reference_diagnostics: Vec::new(),
            call_graph_blockers: Vec::new(),
            direct_blockers: Vec::new(),
            reference_blockers: Vec::new(),
            pseudo: format!("// vendor symbol: {source}::{symbol}\n"),
        }
    }

    fn projected_argument(
        position: usize,
        name: &str,
        c_type: &str,
        value: &str,
    ) -> LinkedProjectedCallArgument {
        LinkedProjectedCallArgument {
            position,
            name: name.to_owned(),
            c_type: c_type.to_owned(),
            direction: "input",
            value: value.to_owned(),
            binding: "constant-or-symbolic",
            root_argument: None,
            root_offset: None,
        }
    }

    fn projected_semantic_action(
        operation: &str,
        arguments: Vec<LinkedProjectedCallArgument>,
        event_dispatch: Option<LinkedEventDispatchContract>,
    ) -> LinkedProjectedSemanticAction {
        LinkedProjectedSemanticAction {
            site_path: vec![Some(0x10)],
            operation: operation.to_owned(),
            target: "semantic::dispatch".to_owned(),
            contract: Some(LinkedSemanticContract {
                source: "test-reviewed-contract",
                id: format!("test::{operation}"),
                evidence: "unit-test".to_owned(),
                event_dispatch,
            }),
            replacement_hint: None,
            origin: "rom::irq_handler".to_owned(),
            path: "rom::irq_handler --semantic@0x00000010--> semantic::dispatch".to_owned(),
            site: Some(0x10),
            argument_shapes: 1,
            arguments,
            guard_scopes: Some(Vec::new()),
        }
    }

    fn event_dispatch_contract(
        mechanism: &'static str,
        execution_context: &'static str,
        receiver: Option<&'static str>,
        argument_roles: &[(&'static str, &'static str)],
    ) -> LinkedEventDispatchContract {
        LinkedEventDispatchContract {
            mechanism,
            execution_context,
            receiver,
            argument_roles: argument_roles
                .iter()
                .map(|&(role, argument)| LinkedEventDispatchArgumentRole { role, argument })
                .collect(),
        }
    }

    #[test]
    fn event_dispatch_projection_assigns_reviewed_argument_roles() {
        let actions = vec![
            projected_semantic_action("wifi.internal-signal.post", Vec::new(), None),
            projected_semantic_action(
                "vendor.radio.notify",
                vec![projected_argument(0, "signal", "u32", "const:0x1a")],
                Some(event_dispatch_contract(
                    "internal-signal",
                    "unspecified",
                    Some("test::radio-owner"),
                    &[("selector", "signal")],
                )),
            ),
            projected_semantic_action(
                "platform.queue.publish",
                vec![
                    projected_argument(0, "queue", "*mut void", "arg0"),
                    projected_argument(1, "item", "*const void", "arg1"),
                    projected_argument(2, "higher_priority_task_woken", "*mut bool", "arg2"),
                ],
                Some(event_dispatch_contract(
                    "rtos-queue",
                    "isr",
                    None,
                    &[
                        ("channel", "queue"),
                        ("payload", "item"),
                        ("wake-output", "higher_priority_task_woken"),
                    ],
                )),
            ),
            projected_semantic_action(
                "platform.event.publish",
                vec![
                    projected_argument(0, "event_base", "*const char", "arg0"),
                    projected_argument(1, "event_id", "i32", "const:0x7"),
                    projected_argument(2, "event_data", "*const void", "arg1"),
                    projected_argument(3, "event_data_size", "usize", "const:0x4"),
                    projected_argument(4, "ticks_to_wait", "u32", "const:0x0"),
                ],
                Some(event_dispatch_contract(
                    "rtos-event-loop",
                    "unspecified",
                    None,
                    &[
                        ("channel", "event_base"),
                        ("selector", "event_id"),
                        ("payload", "event_data"),
                        ("payload-size", "event_data_size"),
                        ("wait", "ticks_to_wait"),
                    ],
                )),
            ),
        ];

        let dispatches = project_event_dispatches(&actions);

        assert_eq!(dispatches.len(), 3);
        assert_eq!(dispatches[0].semantic_action_index, 1);
        assert_eq!(dispatches[0].mechanism, "internal-signal");
        assert_eq!(dispatches[0].execution_context, "unspecified");
        assert_eq!(dispatches[0].receiver.as_deref(), Some("test::radio-owner"));
        assert!(dispatches[0].interface_complete);
        assert_eq!(dispatches[0].bindings[0].role, "selector");
        assert_eq!(dispatches[0].bindings[0].argument.value, "const:0x1a");

        assert_eq!(dispatches[1].semantic_action_index, 2);
        assert_eq!(dispatches[1].mechanism, "rtos-queue");
        assert_eq!(dispatches[1].execution_context, "isr");
        assert_eq!(
            dispatches[1]
                .bindings
                .iter()
                .map(|binding| binding.role)
                .collect::<Vec<_>>(),
            ["channel", "payload", "wake-output"]
        );

        assert_eq!(dispatches[2].semantic_action_index, 3);
        assert_eq!(dispatches[2].mechanism, "rtos-event-loop");
        assert_eq!(
            dispatches[2]
                .bindings
                .iter()
                .map(|binding| binding.role)
                .collect::<Vec<_>>(),
            ["channel", "selector", "payload", "payload-size", "wait"]
        );
        assert!(
            dispatches
                .iter()
                .all(|dispatch| dispatch.interface_complete && dispatch.blockers.is_empty())
        );
        assert!(
            dispatches[1..]
                .iter()
                .all(|dispatch| dispatch.receiver.is_none())
        );
    }

    #[test]
    fn event_dispatch_projection_exposes_contract_and_schema_mismatches() {
        let actions = vec![projected_semantic_action(
            "wifi.internal-signal.post",
            vec![projected_argument(0, "unexpected", "u32", "arg0")],
            Some(event_dispatch_contract(
                "internal-signal",
                "unspecified",
                None,
                &[("selector", "signal")],
            )),
        )];

        let dispatches = project_event_dispatches(&actions);

        assert_eq!(dispatches.len(), 1);
        assert!(!dispatches[0].interface_complete);
        assert!(dispatches[0].bindings.is_empty());
        assert_eq!(
            dispatches[0].blockers,
            [
                "missing semantic argument signal for role selector",
                "unexpected semantic argument unexpected at position 0",
            ]
        );
    }

    #[test]
    fn direct_call_graph_survives_reference_summary_inlining() {
        let parent = symbol(
            "vendor_parent",
            0x1000,
            vec![
                0x97, 0x00, 0x00, 0x00, // auipc ra, 0
                0xe7, 0x80, 0x00, 0x00, // jalr ra, 0(ra)
                0x67, 0x80, 0x00, 0x00, // ret
            ],
        );
        let child = symbol(
            "vendor_child",
            0x2000,
            vec![0x67, 0x80, 0x00, 0x00], // ret
        );
        let child_id = 0x8000_0000;
        let resolver = ReferenceResolver {
            symbols: vec![parent.clone(), child.clone()],
            symbols_by_address: BTreeMap::from([(child_id, child)]),
            symbol_ids: BTreeMap::from([
                (
                    (parent.member.clone(), parent.name.clone(), parent.address),
                    0x8000_0001,
                ),
                (
                    (
                        Some("member.o".to_owned()),
                        "vendor_child".to_owned(),
                        0x2000,
                    ),
                    child_id,
                ),
            ]),
            exported_symbol_keys: BTreeSet::new(),
            relocated_calls: BTreeMap::from([(
                direct::StructuralCallSite::new(&parent, 0x1000),
                ("vendor_child".to_owned(), Some(child_id)),
            )]),
            pointer_context: direct::StructuralPointerContext::default(),
        };
        let map = MmioRegisterMap {
            registers: Vec::new(),
            windows: Vec::new(),
        };

        let identities = IrIdentityCatalog::new(&resolver, None);
        let graph = explore_direct_calls(&parent, &resolver, &identities, &map);
        let calls = graph.calls.into_iter().collect::<Vec<_>>();

        assert!(graph.blockers.is_empty(), "{:#?}", graph.blockers);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].kind, "internal");
        assert_eq!(calls[0].target, "member.o:vendor_child");
        assert_eq!(calls[0].site, Some(0x1000));
        assert_eq!(calls[0].argument_bindings.len(), 16);
        assert_eq!(
            calls[0].argument_bindings[0],
            LinkedArgumentBinding {
                position: 0,
                caller_argument: 0,
                offset: 0,
                expression: "arg0".to_owned(),
            }
        );

        let roots_only =
            build_linked_ir_for_source(&resolver, "vendor_parent", &map, "primary", false, false);
        assert_eq!(roots_only.functions.len(), 1);
        assert_eq!(roots_only.functions[0].symbol, "vendor_parent");

        let report =
            build_linked_ir_for_source(&resolver, "vendor_parent", &map, "primary", false, true);
        assert_eq!(
            report
                .functions
                .iter()
                .map(|function| (function.symbol.as_str(), function.selection))
                .collect::<Vec<_>>(),
            [
                ("vendor_child", "reachable-internal"),
                ("vendor_parent", "symbol-prefix-root"),
            ]
        );
    }

    #[test]
    fn external_call_keeps_reviewed_table_slot_semantics() {
        static ARGUMENTS: [crate::ExternalArgumentSpec; 1] = [crate::ExternalArgumentSpec {
            name: "micros",
            c_type: "u32",
            direction: crate::ExternalArgumentDirection::Input,
        }];
        static FUNCTIONS: [crate::ExternalFunctionSpec; 1] = [crate::ExternalFunctionSpec {
            id: "delay_us",
            offset: 0x20,
            c_name: "ets_delay_us",
            argument_count: 1,
            return_model: ExternalReturnModel::Constant(0),
            semantic: crate::ExternalSemanticSpec {
                operation: "time.delay-micros",
                arguments: &ARGUMENTS,
                return_type: "void",
                replacement: Some("Rust async timer"),
                event_dispatch: None,
            },
        }];
        static TABLE: crate::ExternalTableSpec = crate::ExternalTableSpec {
            id: "wifi_osi",
            pointer_symbol: "g_wifi_osi_funcs",
            backing_symbol: "wifi_osi_funcs",
            version: 3,
            magic: 0x1234_5678,
            size: 0x100,
            magic_offset: 0,
            functions: &FUNCTIONS,
        };
        let event = DraftReferenceEvent::ExternalCall {
            token: 0,
            table: crate::ExternalTableRef::new(&TABLE),
            function: crate::ExternalFunctionRef::new(&FUNCTIONS[0]),
            arguments: vec![SymbolicValue::input(0)].into_boxed_slice(),
        };
        let mut calls = BTreeSet::new();
        let resolver = empty_resolver();
        let identities = IrIdentityCatalog::new(&resolver, None);
        let mut pseudo = String::new();

        collect_call_event(&event, &resolver, &identities, &mut calls);
        render_event(&event, &mut pseudo, 1, &mut RenderState::default());
        let call = calls.into_iter().next().unwrap();

        assert_eq!(call.kind, "external");
        assert_eq!(call.target, "wifi_osi::ets_delay_us");
        assert_eq!(call.arguments, [SymbolicValue::input(0).canonical()]);
        assert_eq!(
            call.semantic_operation.as_deref(),
            Some("time.delay-micros")
        );
        assert_eq!(
            call.semantic_contract.as_ref(),
            Some(&LinkedSemanticContract {
                source: "registered-external-table-slot",
                id: "wifi_osi::delay_us".to_owned(),
                evidence: "exact-pointer-cell-and-slot".to_owned(),
                event_dispatch: None,
            })
        );
        assert_eq!(call.replacement_hint.as_deref(), Some("Rust async timer"));
        assert_eq!(call.typed_arguments.len(), 1);
        assert_eq!(call.typed_arguments[0].name, "micros");
        assert_eq!(call.typed_arguments[0].c_type, "u32");
        assert_eq!(call.typed_arguments[0].direction, "input");
        let trampoline = call.trampoline.as_ref().unwrap();
        assert_eq!(trampoline.table, "wifi_osi");
        assert_eq!(trampoline.pointer_symbol, "g_wifi_osi_funcs");
        assert_eq!(trampoline.backing_symbol, "wifi_osi_funcs");
        assert_eq!(trampoline.version, 3);
        assert_eq!(trampoline.magic, 0x1234_5678);
        assert_eq!(trampoline.table_size, 0x100);
        assert_eq!(trampoline.slot, 0x20);
        assert_eq!(trampoline.function_id, "delay_us");
        assert_eq!(trampoline.return_model, "constant:0x00000000");
        assert_eq!(trampoline.return_type, "void");
        assert!(
            pseudo.contains(
                "semantic.time_delay_micros(micros /* u32 Input */ = arg0); // ABI wifi_osi+0x20 ets_delay_us, returns void; replacement: Rust async timer"
            ),
            "{pseudo}"
        );
        assert!(
            call.semantics
                .as_deref()
                .is_some_and(|semantics| semantics.contains("version=3 slot=0x20 args=1")),
            "{:?}",
            call.semantics
        );
    }

    #[test]
    fn call_compaction_keeps_only_bindings_shared_by_every_argument_shape() {
        let variant = |second_argument: &str, second_caller: u8| LinkedCall {
            kind: "internal",
            target: "member.o:callee".to_owned(),
            site: Some(0x24),
            tail: false,
            result_modeled: false,
            semantics: None,
            semantic_operation: None,
            semantic_contract: None,
            replacement_hint: None,
            project_symbol: None,
            project_candidates: Vec::new(),
            trampoline: None,
            argument_shapes: 1,
            arguments: vec!["arg0".to_owned(), second_argument.to_owned()],
            argument_bindings: vec![
                LinkedArgumentBinding {
                    position: 0,
                    caller_argument: 0,
                    offset: 0,
                    expression: "arg0".to_owned(),
                },
                LinkedArgumentBinding {
                    position: 1,
                    caller_argument: second_caller,
                    offset: 4,
                    expression: format!("arg{second_caller} + 0x4"),
                },
            ],
            typed_arguments: vec![LinkedCallArgument {
                position: 1,
                name: "context".to_owned(),
                c_type: "*mut context".to_owned(),
                direction: "input-output",
                value: second_argument.to_owned(),
            }],
            guard_paths: None,
        };

        let calls = compact_calls([variant("arg1+0x4", 1), variant("arg3+0x4", 3)]);

        assert_eq!(calls.len(), 1);
        let call = &calls[0];
        assert_eq!(call.argument_shapes, 2);
        assert_eq!(call.arguments, ["arg0", "varies-across-2-shapes"]);
        assert_eq!(call.typed_arguments[0].value, "varies-across-2-shapes");
        assert_eq!(
            call.argument_bindings,
            [LinkedArgumentBinding {
                position: 0,
                caller_argument: 0,
                offset: 0,
                expression: "arg0".to_owned(),
            }]
        );

        let guarded = |taken| {
            let mut call = variant("arg1+0x4", 1);
            call.guard_paths = Some(vec![LinkedCallGuardPath {
                guards: vec![LinkedCallGuard {
                    site: 0x10,
                    condition: "arg0 != 0".to_owned(),
                    operation: "not-equal",
                    taken,
                    result_sources: Vec::new(),
                    direct_mmio_sources: Vec::new(),
                }],
            }]);
            call
        };
        let guarded = compact_calls([guarded(true), guarded(false)]);
        assert_eq!(guarded[0].argument_shapes, 1);
        assert_eq!(
            guarded[0].guard_paths,
            Some(vec![LinkedCallGuardPath { guards: Vec::new() }])
        );
    }

    #[test]
    fn cfg_guard_paths_minimize_complementary_branches_without_weakening_other_clauses() {
        let guard = |site, condition: &str, taken| LinkedCallGuard {
            site,
            condition: condition.to_owned(),
            operation: "not-equal",
            taken,
            result_sources: Vec::new(),
            direct_mmio_sources: Vec::new(),
        };
        let paths = normalize_guard_paths([
            LinkedCallGuardPath {
                guards: vec![
                    guard(0x10, "arg0 != 0", true),
                    guard(0x20, "arg1 == 3", true),
                ],
            },
            LinkedCallGuardPath {
                guards: vec![
                    guard(0x10, "arg0 != 0", true),
                    guard(0x20, "arg1 == 3", false),
                ],
            },
            LinkedCallGuardPath {
                guards: vec![
                    guard(0x10, "arg0 != 0", true),
                    guard(0x20, "arg1 == 3", true),
                    guard(0x30, "arg2 < 4", true),
                ],
            },
        ]);

        assert_eq!(
            paths,
            [LinkedCallGuardPath {
                guards: vec![guard(0x10, "arg0 != 0", true)],
            }]
        );
    }

    #[test]
    fn guard_path_rendering_keeps_the_residual_condition_for_one_literal() {
        let guard = |site, condition: &str, taken| LinkedCallGuard {
            site,
            condition: condition.to_owned(),
            operation: "not-equal",
            taken,
            result_sources: Vec::new(),
            direct_mmio_sources: Vec::new(),
        };
        let path = LinkedCallGuardPath {
            guards: vec![
                guard(0x10, "status & 0x30 != 0", false),
                guard(0x20, "queue != 0", true),
            ],
        };
        assert_eq!(
            format_guard_path(&path),
            "!(status & 0x30 != 0) && (queue != 0)"
        );
        assert_eq!(format_guard_path_without(&path, 0), "(queue != 0)");
    }

    #[test]
    fn cfg_guard_names_call_results_without_token_prefix_collisions() {
        let call_results = BTreeMap::from([
            (1, "vendor::one".to_owned()),
            (10, "vendor::ten".to_owned()),
        ]);

        assert_eq!(
            name_call_results("call1 | call10 | external10 | callback", &call_results),
            "result_of_vendor__one_1 | result_of_vendor__ten_10 | result_of_vendor__ten_10 | callback"
        );
    }

    #[test]
    fn diagnostic_compaction_counts_exact_fragments_and_keeps_first_ordinals() {
        let diagnostic = compact_diagnostic(
            "symbolic-cfg: unsupported effects; repeated call; unique jump; repeated call; repeated call",
        );

        assert_eq!(diagnostic.original_fragments, 5);
        assert_eq!(diagnostic.fragments.len(), 3);
        assert_eq!(diagnostic.fragments[0].first_ordinal, 0);
        assert_eq!(diagnostic.fragments[0].occurrences, 1);
        assert_eq!(diagnostic.fragments[1].first_ordinal, 1);
        assert_eq!(diagnostic.fragments[1].occurrences, 3);
        assert_eq!(diagnostic.fragments[1].message, "repeated call");
        assert_eq!(diagnostic.fragments[2].first_ordinal, 2);
        assert_eq!(
            diagnostic.rendered,
            "symbolic-cfg: unsupported effects; repeated call [repeated 3 times]; unique jump"
        );
    }

    #[test]
    fn diagnostic_compaction_leaves_a_single_fragment_unchanged() {
        let diagnostic = compact_diagnostic("decoder stopped at unsupported instruction");

        assert_eq!(diagnostic.original_fragments, 1);
        assert_eq!(diagnostic.fragments.len(), 1);
        assert_eq!(
            diagnostic.rendered,
            "decoder stopped at unsupported instruction"
        );
    }

    #[test]
    fn pseudo_value_renders_register_images_as_read_modify_write_expressions() {
        let value = SymbolicValue::RegisterImage {
            read_token: 3,
            address: 0x2010_7030,
            and_mask: 0xdfff_ffff,
            or_mask: 0x2000_0000,
        };

        assert_eq!(pseudo_value(&value), "((read3 & 0xdfffffff) | 0x20000000)");
    }

    #[test]
    fn pseudo_value_compacts_aligned_symbolic_bit_slices_into_masks() {
        let mut bits = [BitSource::Constant(false); 32];
        for (bit, source) in bits.iter_mut().enumerate().take(8).skip(4) {
            *source = BitSource::CallResult {
                call_token: 10,
                bit: bit as u8,
                inverted: false,
            };
        }

        assert_eq!(
            pseudo_value(&SymbolicValue::Bits(Box::new(bits))),
            "(call10 & 0x000000f0)"
        );
    }

    #[test]
    fn cfg_guard_result_sources_link_masks_to_producer_targets() {
        let mut bits = [BitSource::Constant(false); 32];
        for (bit, source) in bits.iter_mut().enumerate().take(8).skip(4) {
            *source = BitSource::CallResult {
                call_token: 10,
                bit: bit as u8,
                inverted: false,
            };
        }
        let condition = BranchCondition {
            site: 0x20,
            operation: BranchOperation::NotEqual,
            left: SymbolicValue::Bits(Box::new(bits)),
            right: SymbolicValue::Constant(0),
        };
        let call_results = BTreeMap::from([(10, "hal::interrupt_status".to_owned())]);

        assert_eq!(
            guard_result_sources(&condition, &call_results),
            [LinkedCallGuardResultSource {
                kind: "call-result",
                token: 10,
                target: Some("hal::interrupt_status".to_owned()),
                operand: "left",
                value_bits: Some(0x0000_00f0),
                source_bits: 0x0000_00f0,
                inverted: false,
                comparison_value: Some(0),
                source_comparison_value: Some(0),
                producer_return_exact: None,
                mmio_sources: Vec::new(),
            }]
        );
    }

    #[test]
    fn direct_mmio_predicate_maps_shifted_comparison_back_to_register_bits() {
        let address = 0x2010_4c48;
        let condition = BranchCondition {
            site: 0x24,
            operation: BranchOperation::Equal,
            left: SymbolicValue::register_read(7, address, 32, false)
                .and(0x0000_00f0)
                .shift_right(4),
            right: SymbolicValue::Constant(3),
        };
        let svd = MmioRegisterMap {
            registers: vec![crate::Register {
                address,
                name: "WIFI_MAC_INTERRUPT.STATUS".to_owned(),
            }],
            windows: Vec::new(),
        };

        assert_eq!(
            direct_mmio_predicate_sources(&condition, &svd),
            [LinkedDirectMmioPredicateSource {
                operand: "left",
                read_token: 7,
                address,
                register: "WIFI_MAC_INTERRUPT.STATUS".to_owned(),
                value_bits: 0x0000_000f,
                register_bits: 0x0000_00f0,
                inverted: false,
                comparison_value: Some(3),
                register_comparison_value: Some(0x30),
            }]
        );
    }

    #[test]
    fn direct_mmio_branch_is_inventoried_without_a_guarded_call() {
        let address = 0x2010_4cb4;
        let event = DraftReferenceEvent::BranchDecision {
            condition: BranchCondition {
                site: 0x54,
                operation: BranchOperation::Equal,
                left: SymbolicValue::register_read(2, address, 32, false).and(0xf0),
                right: SymbolicValue::Constant(0),
            },
            taken: true,
        };
        let resolver = empty_resolver();
        let identities = IrIdentityCatalog::new(&resolver, None);
        let svd = MmioRegisterMap {
            registers: vec![crate::Register {
                address,
                name: "WIFI_MAC_TX_COMMON.QUEUE_STATE".to_owned(),
            }],
            windows: Vec::new(),
        };
        let mut evidence = DirectTraceEvidence::default();

        collect_guarded_direct_event(&event, &resolver, &identities, &svd, &mut evidence);

        assert!(evidence.calls.is_empty());
        assert_eq!(evidence.direct_mmio_predicates.len(), 1);
        let predicate = evidence
            .direct_mmio_predicates
            .first()
            .expect("direct predicate was retained");
        assert_eq!(predicate.operation, "equal");
        assert_eq!(predicate.sources[0].register_bits, 0xf0);
        assert_eq!(predicate.sources[0].register_comparison_value, Some(0));
        assert_eq!(
            current_guard_path(&evidence.guards).guards[0].direct_mmio_sources,
            predicate.sources
        );
    }

    #[test]
    fn return_provenance_maps_result_ranges_back_to_mmio_bits() {
        let mut bits = [BitSource::Constant(false); 32];
        for (output_bit, source) in bits.iter_mut().enumerate().take(4) {
            *source = BitSource::Register {
                read_token: 3,
                address: 0x2010_4c48,
                bit: output_bit as u8 + 4,
                inverted: false,
            };
        }
        let svd = MmioRegisterMap {
            registers: vec![crate::Register {
                address: 0x2010_4c48,
                name: "WIFI_MAC_INTERRUPT.STATUS".to_owned(),
            }],
            windows: vec![crate::Window {
                start: 0x2010_0000,
                end: 0x2011_0000,
            }],
        };

        let provenance =
            return_provenance(&SymbolicValue::Bits(Box::new(bits)), &BTreeMap::new(), &svd);

        assert!(provenance.exact);
        assert_eq!(provenance.known_zero_bits, 0xffff_fff0);
        assert_eq!(provenance.unknown_bits, 0);
        assert_eq!(provenance.sources.len(), 1);
        assert_eq!(provenance.sources[0].output_bits, 0x0000_000f);
        assert_eq!(provenance.sources[0].source_bits, 0x0000_00f0);
        let result_source = LinkedCallGuardResultSource {
            kind: "call-result",
            token: 10,
            target: Some("hal::interrupt_status".to_owned()),
            operand: "left",
            value_bits: Some(0x0000_0005),
            source_bits: 0x0000_0005,
            inverted: false,
            comparison_value: Some(1),
            source_comparison_value: Some(1),
            producer_return_exact: None,
            mmio_sources: Vec::new(),
        };
        let producers = BTreeMap::from([("hal::interrupt_status".to_owned(), provenance)]);
        assert_eq!(
            guard_mmio_sources(&result_source, "hal::interrupt_status", &producers),
            [LinkedCallGuardMmioSource {
                address: 0x2010_4c48,
                register: "WIFI_MAC_INTERRUPT.STATUS".to_owned(),
                producer_path: vec!["hal::interrupt_status".to_owned()],
                result_bits: 0x0000_0005,
                register_bits: 0x0000_0050,
                inverted: false,
                result_comparison_value: Some(1),
                register_comparison_value: Some(0x10),
            }]
        );
    }

    #[test]
    fn guard_comparison_projects_through_shifted_inverted_producer_return() {
        let address = 0x2010_4c48;
        let mut condition_bits = [BitSource::Constant(false); 32];
        for (value_bit, source) in condition_bits.iter_mut().enumerate().take(4) {
            *source = BitSource::CallResult {
                call_token: 10,
                bit: value_bit as u8 + 4,
                inverted: false,
            };
        }
        let condition = BranchCondition {
            site: 0x20,
            operation: BranchOperation::Equal,
            left: SymbolicValue::Bits(Box::new(condition_bits)),
            right: SymbolicValue::Constant(3),
        };
        let result_source = guard_result_sources(
            &condition,
            &BTreeMap::from([(10, "hal::interrupt_status".to_owned())]),
        )
        .into_iter()
        .next()
        .expect("shifted call result has exact provenance");

        assert_eq!(result_source.value_bits, Some(0x0000_000f));
        assert_eq!(result_source.source_bits, 0x0000_00f0);
        assert_eq!(result_source.comparison_value, Some(3));
        assert_eq!(result_source.source_comparison_value, Some(0x30));

        let mut return_bits = [BitSource::Constant(false); 32];
        for (output_bit, source) in return_bits.iter_mut().enumerate().take(8).skip(4) {
            *source = BitSource::Register {
                read_token: 3,
                address,
                bit: output_bit as u8 + 4,
                inverted: true,
            };
        }
        let provenance = return_provenance(
            &SymbolicValue::Bits(Box::new(return_bits)),
            &BTreeMap::new(),
            &MmioRegisterMap {
                registers: vec![crate::Register {
                    address,
                    name: "WIFI_MAC_INTERRUPT.STATUS".to_owned(),
                }],
                windows: Vec::new(),
            },
        );
        let producers = BTreeMap::from([("hal::interrupt_status".to_owned(), provenance)]);

        assert_eq!(
            guard_mmio_sources(&result_source, "hal::interrupt_status", &producers),
            [LinkedCallGuardMmioSource {
                address,
                register: "WIFI_MAC_INTERRUPT.STATUS".to_owned(),
                producer_path: vec!["hal::interrupt_status".to_owned()],
                result_bits: 0x0000_00f0,
                register_bits: 0x0000_0f00,
                inverted: true,
                result_comparison_value: Some(0x30),
                register_comparison_value: Some(0xc00),
            }]
        );
    }

    #[test]
    fn guard_mmio_sources_follow_exact_internal_return_wrappers() {
        let address = 0x2010_4c48;
        let svd = MmioRegisterMap {
            registers: vec![crate::Register {
                address,
                name: "WIFI_MAC_INTERRUPT.STATUS".to_owned(),
            }],
            windows: Vec::new(),
        };
        let mut leaf_bits = [BitSource::Constant(false); 32];
        for (output_bit, source) in leaf_bits.iter_mut().enumerate().take(8).skip(4) {
            *source = BitSource::Register {
                read_token: 3,
                address,
                bit: output_bit as u8 + 4,
                inverted: true,
            };
        }
        let leaf = return_provenance(
            &SymbolicValue::Bits(Box::new(leaf_bits)),
            &BTreeMap::new(),
            &svd,
        );
        let mut wrapper_bits = [BitSource::Constant(false); 32];
        for (output_bit, source) in wrapper_bits.iter_mut().enumerate().take(4) {
            *source = BitSource::CallResult {
                call_token: 7,
                bit: output_bit as u8 + 4,
                inverted: true,
            };
        }
        let wrapper = return_provenance(
            &SymbolicValue::Bits(Box::new(wrapper_bits)),
            &BTreeMap::from([(7, "hal::leaf_status".to_owned())]),
            &svd,
        );
        let result_source = LinkedCallGuardResultSource {
            kind: "call-result",
            token: 10,
            target: Some("hal::status_wrapper".to_owned()),
            operand: "left",
            value_bits: Some(0x5),
            source_bits: 0x5,
            inverted: false,
            comparison_value: Some(1),
            source_comparison_value: Some(1),
            producer_return_exact: None,
            mmio_sources: Vec::new(),
        };
        let producers = BTreeMap::from([
            ("hal::leaf_status".to_owned(), leaf),
            ("hal::status_wrapper".to_owned(), wrapper),
        ]);

        assert_eq!(
            guard_mmio_sources(&result_source, "hal::status_wrapper", &producers),
            [LinkedCallGuardMmioSource {
                address,
                register: "WIFI_MAC_INTERRUPT.STATUS".to_owned(),
                producer_path: vec![
                    "hal::status_wrapper".to_owned(),
                    "hal::leaf_status".to_owned(),
                ],
                result_bits: 0x5,
                register_bits: 0x500,
                inverted: false,
                result_comparison_value: Some(1),
                register_comparison_value: Some(0x100),
            }]
        );
    }

    #[test]
    fn guard_mmio_sources_stop_at_recursive_return_cycles() {
        let call_return = |target: &str| LinkedReturnProvenance {
            exact: true,
            known_zero_bits: u32::MAX ^ 1,
            known_one_bits: 0,
            unknown_bits: 0,
            sources: vec![LinkedReturnBitSource {
                kind: "call-result",
                output_lsb: 0,
                source_lsb: 0,
                width: 1,
                output_bits: 1,
                source_bits: 1,
                inverted: false,
                argument: None,
                token: Some(0),
                target: Some(target.to_owned()),
                address: None,
                register: None,
            }],
        };
        let producers = BTreeMap::from([
            ("a".to_owned(), call_return("b")),
            ("b".to_owned(), call_return("a")),
        ]);
        let source = LinkedCallGuardResultSource {
            kind: "call-result",
            token: 0,
            target: Some("a".to_owned()),
            operand: "left",
            value_bits: Some(1),
            source_bits: 1,
            inverted: false,
            comparison_value: Some(0),
            source_comparison_value: Some(0),
            producer_return_exact: None,
            mmio_sources: Vec::new(),
        };

        assert!(guard_mmio_sources(&source, "a", &producers).is_empty());
    }

    #[test]
    fn pseudo_ir_keeps_a_named_call_and_structured_branch() {
        let callee_flow = DraftReferenceFlow {
            events: Vec::new(),
            terminator: DraftReferenceTerminator::Return(SymbolicValue::input(0)),
        };
        let flow = DraftReferenceFlow {
            events: vec![DraftReferenceEvent::ComposedCall {
                token: 0,
                symbol: "vendor_child".to_owned(),
                arguments: vec![SymbolicValue::input(0)].into_boxed_slice(),
                flow: Box::new(callee_flow),
                result_modeled: true,
            }],
            terminator: DraftReferenceTerminator::Branch {
                condition: BranchCondition {
                    site: 0x1010,
                    operation: BranchOperation::Equal,
                    left: SymbolicValue::input(0),
                    right: SymbolicValue::Constant(0),
                },
                taken: Box::new(DraftReferenceFlow {
                    events: Vec::new(),
                    terminator: DraftReferenceTerminator::Return(SymbolicValue::Constant(1)),
                }),
                not_taken: Box::new(DraftReferenceFlow {
                    events: Vec::new(),
                    terminator: DraftReferenceTerminator::Return(SymbolicValue::Constant(2)),
                }),
            },
        };
        let trace = FunctionAnalysis {
            symbol: "vendor_parent".to_owned(),
            events: Vec::new(),
            reference_events: Vec::new(),
            reference_dependencies: vec!["vendor_child".to_owned()],
            blockers: Vec::new(),
            reference_blockers: Vec::new(),
            return_value: SymbolicValue::Unknown,
            reference_flow: Some(flow),
            unresolved_branch: None,
        };

        let pseudo = render_pseudo("vendor_parent", &trace, &[], &[], &[], &[]);
        assert!(
            pseudo.contains("let call0 = vendor_child(arg0);"),
            "{pseudo}"
        );
        assert!(pseudo.contains("if arg0 == 0x00000000"), "{pseudo}");
        assert!(pseudo.contains("return 0x00000001;"), "{pseudo}");
        assert!(pseudo.contains("return 0x00000002;"), "{pseudo}");
    }

    #[test]
    fn context_map_recovers_argument_offsets_branch_paths_and_rmw_masks() {
        let write = DraftReferenceEvent::Memory {
            access: MemoryAccess::Write,
            width: 32,
            address: SymbolicValue::input(2).add_constant(4),
            region: "caller-owned ABI argument RAM".to_owned(),
            value: Some(SymbolicValue::MemoryImage {
                read_token: 0,
                and_mask: 0xffff_ffdf,
                or_mask: 0x20,
            }),
        };
        let trace = FunctionAnalysis {
            symbol: "update_context".to_owned(),
            events: Vec::new(),
            reference_events: Vec::new(),
            reference_dependencies: Vec::new(),
            blockers: Vec::new(),
            reference_blockers: Vec::new(),
            return_value: SymbolicValue::Constant(0),
            reference_flow: Some(DraftReferenceFlow {
                events: Vec::new(),
                terminator: DraftReferenceTerminator::Branch {
                    condition: BranchCondition {
                        site: 0x1010,
                        operation: BranchOperation::NotEqual,
                        left: SymbolicValue::input(1),
                        right: SymbolicValue::Constant(0),
                    },
                    taken: Box::new(DraftReferenceFlow {
                        events: vec![write],
                        terminator: DraftReferenceTerminator::Return(SymbolicValue::Constant(1)),
                    }),
                    not_taken: Box::new(DraftReferenceFlow {
                        events: Vec::new(),
                        terminator: DraftReferenceTerminator::Return(SymbolicValue::Constant(0)),
                    }),
                },
            }),
            unresolved_branch: None,
        };

        let accesses = context_accesses_for_trace(&trace);
        let fields = context_fields_for_accesses(&accesses);
        let pseudo = render_pseudo("update_context", &trace, &[], &[], &[], &[]);

        assert_eq!(accesses.len(), 1);
        assert_eq!(accesses[0].argument, 2);
        assert_eq!(accesses[0].offset, 4);
        assert_eq!(accesses[0].write_mask, Some(0x20));
        assert_eq!(accesses[0].preserved_mask, Some(0xffff_ffdf));
        assert_eq!(accesses[0].forced_zero_mask, Some(0));
        assert_eq!(accesses[0].forced_one_mask, Some(0x20));
        assert!(accesses[0].path.contains("if arg1 != 0x00000000"));
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].reads, 0);
        assert_eq!(fields[0].writes, 1);
        assert_eq!(fields[0].write_mask, 0x20);
        assert!(
            pseudo.contains("ctx2.write32(+0x4, ((ramread0 & 0xffffffdf) | 0x00000020));"),
            "{pseudo}"
        );
    }

    #[test]
    fn mmio_index_keeps_static_indexed_poll_and_write_bit_evidence() {
        assert_eq!(
            candidate_bit_ranges(0x3000_00f3, 32),
            [
                (0, 1, 0x0000_0003),
                (4, 7, 0x0000_00f0),
                (28, 29, 0x3000_0000),
            ]
        );
        let address = 0x2010_7030;
        let write_value = SymbolicValue::register_read(0, address, 32, false)
            .and(0xffff_fff0)
            .or(0x5);
        let trace = FunctionAnalysis {
            symbol: "touch_registers".to_owned(),
            events: Vec::new(),
            reference_events: Vec::new(),
            reference_dependencies: Vec::new(),
            blockers: Vec::new(),
            reference_blockers: Vec::new(),
            return_value: SymbolicValue::Constant(0),
            reference_flow: Some(DraftReferenceFlow {
                events: vec![
                    DraftReferenceEvent::Observable(ObservableEvent::Memory {
                        access: MemoryAccess::Write,
                        width: 32,
                        address,
                        register: "AGC.CONTROL".to_owned(),
                        value: Some(write_value),
                    }),
                    DraftReferenceEvent::IndexedMmio {
                        access: MemoryAccess::Read,
                        width: 32,
                        address: SymbolicValue::input(0).shift_left(2).add_constant(address),
                        registers: vec![
                            crate::IndexedMmioRegister {
                                address,
                                name: "AGC.CONTROL".to_owned(),
                            },
                            crate::IndexedMmioRegister {
                                address: address + 4,
                                name: "AGC.STATUS".to_owned(),
                            },
                        ],
                        guard: Some(crate::IndexedMmioGuard {
                            selector: SymbolicValue::input(0),
                            maximum: 2,
                        }),
                        value: None,
                    },
                    DraftReferenceEvent::PollMmio {
                        width: 32,
                        address: SymbolicValue::Constant(address + 4),
                        registers: vec![crate::IndexedMmioRegister {
                            address: address + 4,
                            name: "AGC.STATUS".to_owned(),
                        }],
                        guard: None,
                        mask: 1,
                        expected: 1,
                    },
                ],
                terminator: DraftReferenceTerminator::Return(SymbolicValue::Constant(0)),
            }),
            unresolved_branch: None,
        };

        let accesses = mmio_accesses_for_trace(&trace);

        assert_eq!(accesses.len(), 4);
        assert_eq!(
            accesses
                .iter()
                .map(|access| access.ordinal)
                .collect::<Vec<_>>(),
            [0, 1, 2, 3]
        );
        let write = accesses
            .iter()
            .find(|access| access.access == "write")
            .unwrap();
        assert_eq!(write.mode, "static");
        assert_eq!(write.modified_mask, Some(0xf));
        assert_eq!(write.preserved_mask, Some(0xffff_fff0));
        assert_eq!(write.forced_zero_mask, Some(0xa));
        assert_eq!(write.forced_one_mask, Some(0x5));
        assert_eq!(
            accesses
                .iter()
                .filter(|access| access.mode == "indexed-candidate")
                .count(),
            2
        );
        let poll = accesses
            .iter()
            .find(|access| access.access == "poll")
            .unwrap();
        assert_eq!(poll.mode, "static");
        assert_eq!(poll.predicate_mask, Some(1));
        assert_eq!(poll.predicate_expected, Some(1));
        assert_eq!(
            poll.guard.as_deref(),
            Some("value & 0x00000001 == 0x00000001")
        );
    }

    #[test]
    fn field_candidates_separate_evidence_and_exclude_whole_register_masks() {
        let mut register = MmioRegisterAccumulator::default();
        record_access_field_mask(&mut register, 0x30, 32, "writer", "write", None);
        record_access_field_mask(&mut register, 0x30, 32, "poller", "poll", None);
        let false_branch = LinkedMmioFieldPredicateEvidence {
            kind: "producer-return",
            function: "dispatcher".to_owned(),
            producer: Some("wrapper".to_owned()),
            producer_path: vec!["wrapper".to_owned(), "reader".to_owned()],
            site: Some(0x10),
            path: None,
            condition: "result & 0x30 != 0".to_owned(),
            operation: "not-equal",
            taken: Some(false),
            effective_operation: Some("equal"),
            operand: Some("left"),
            comparison_value: Some(0),
            register_comparison_value: Some(0),
            inverted: false,
        };
        let mut true_branch = false_branch.clone();
        true_branch.taken = Some(true);
        true_branch.effective_operation = Some("not-equal");
        record_predicate_field_mask(
            &mut register,
            0x30,
            32,
            "dispatcher",
            &[false_branch, true_branch],
        );
        record_semantic_field_link(
            &mut register,
            SemanticFieldEvidence {
                kind: "producer-return",
                mask: 0x30,
                width: 32,
                operation: "rtos.event.post",
                root: "irq_handler",
                action_target: "pp_post",
                action_origin: "event_dispatch",
                action_site: Some(0x30),
                action_site_path: &[Some(0x20), Some(0x30)],
                action_path: "irq_handler -> event_dispatch -> pp_post",
                predicate_function: "dispatcher",
                producer: Some("wrapper"),
                producer_path: &["wrapper".to_owned(), "reader".to_owned()],
                scope_index: 0,
                scope_alternatives: 1,
                path_index: 0,
                path_expression: "!(result & 0x30 != 0) && (queue != 0)",
                path_guards: 2,
                guard_index: 0,
                residual_path_expression: "(queue != 0)",
                site: 0x10,
                condition: "result & 0x30 != 0",
                taken: false,
                guard_operation: "not-equal",
            },
        );
        record_access_field_mask(&mut register, u32::MAX, 32, "whole_writer", "write", None);
        record_predicate_field_mask(
            &mut register,
            u32::MAX,
            32,
            "whole_dispatcher",
            &[LinkedMmioFieldPredicateEvidence {
                kind: "direct-mmio",
                function: "whole_dispatcher".to_owned(),
                producer: None,
                producer_path: vec!["whole_dispatcher".to_owned()],
                site: Some(0x20),
                path: None,
                condition: "read != 0".to_owned(),
                operation: "not-equal",
                taken: None,
                effective_operation: None,
                operand: Some("left"),
                comparison_value: Some(0),
                register_comparison_value: Some(0),
                inverted: false,
            }],
        );

        assert_eq!(register.field_candidates.len(), 1);
        let candidate = register
            .field_candidates
            .get(&(4, 5, 0x30))
            .expect("partial mask creates one contiguous candidate");
        assert_eq!(candidate.write_shapes, 1);
        assert_eq!(candidate.poll_shapes, 1);
        assert_eq!(candidate.predicate_shapes, 1);
        assert_eq!(
            candidate.functions,
            BTreeSet::from([
                "dispatcher".to_owned(),
                "poller".to_owned(),
                "reader".to_owned(),
                "wrapper".to_owned(),
                "writer".to_owned(),
            ])
        );
        assert_eq!(
            candidate.access_functions,
            BTreeSet::from([
                "poller".to_owned(),
                "reader".to_owned(),
                "writer".to_owned()
            ])
        );
        assert_eq!(
            candidate.predicate_functions,
            BTreeSet::from(["dispatcher".to_owned()])
        );
        assert_eq!(candidate.predicate_evidence.len(), 2);
        assert_eq!(
            candidate.semantic_operations,
            BTreeSet::from(["rtos.event.post".to_owned()])
        );
        assert_eq!(
            candidate.semantic_roots,
            BTreeSet::from(["irq_handler".to_owned()])
        );
        assert_eq!(candidate.semantic_evidence.len(), 1);
        let semantic = candidate.semantic_evidence.first().unwrap();
        assert_eq!(semantic.effective_operation, "equal");
        assert!(!semantic.taken);
        assert_eq!(semantic.action_site_path, [Some(0x20), Some(0x30)]);
        assert_eq!(semantic.path_index, 0);
        assert_eq!(semantic.guard_index, 0);
        assert_eq!(semantic.residual_path_expression, "(queue != 0)");
        assert_eq!(semantic.producer.as_deref(), Some("wrapper"));
        assert_eq!(semantic.producer_path, ["wrapper", "reader"]);
    }

    #[test]
    fn duplicate_private_names_get_stable_address_qualified_ir_identities() {
        let first = artifact::ArtifactSymbolDefinition {
            member: None,
            name: "private_helper".to_owned(),
            address: 0x1000,
            bytes: vec![0x67, 0x80, 0x00, 0x00],
            addresses_resolved: true,
            memory_regions: Vec::new(),
            relocations: Vec::new(),
        };
        let second = artifact::ArtifactSymbolDefinition {
            address: 0x2000,
            ..first.clone()
        };
        let resolver = ReferenceResolver {
            symbols: vec![first.clone(), second.clone()],
            symbols_by_address: BTreeMap::from([
                (first.address as u32, first.clone()),
                (second.address as u32, second.clone()),
            ]),
            symbol_ids: BTreeMap::from([
                (
                    (None, first.name.clone(), first.address),
                    first.address as u32,
                ),
                (
                    (None, second.name.clone(), second.address),
                    second.address as u32,
                ),
            ]),
            exported_symbol_keys: BTreeSet::new(),
            relocated_calls: BTreeMap::new(),
            pointer_context: direct::StructuralPointerContext::default(),
        };
        let map = MmioRegisterMap {
            registers: Vec::new(),
            windows: Vec::new(),
        };

        let report =
            build_linked_ir_for_source(&resolver, "private_", &map, "primary", false, false);

        assert_eq!(report.exported_functions, 0);
        assert_eq!(report.local_functions, 2);
        assert_eq!(
            report
                .functions
                .iter()
                .map(|function| (function.identity.as_str(), function.binding))
                .collect::<Vec<_>>(),
            [
                ("private_helper@0x00001000", "local"),
                ("private_helper@0x00002000", "local"),
            ]
        );

        let project_report = merge_linked_ir(vec![
            build_linked_ir_for_source(&resolver, "private_", &map, "libphy", true, false),
            build_linked_ir_for_source(&resolver, "private_", &map, "rom", true, false),
        ]);
        assert_eq!(project_report.functions.len(), 4);
        assert_eq!(
            project_report
                .functions
                .iter()
                .map(|function| (function.source.as_str(), function.identity.as_str()))
                .collect::<Vec<_>>(),
            [
                ("libphy", "libphy::private_helper@0x00001000"),
                ("libphy", "libphy::private_helper@0x00002000"),
                ("rom", "rom::private_helper@0x00001000"),
                ("rom", "rom::private_helper@0x00002000"),
            ]
        );
    }

    #[test]
    fn project_call_linking_requires_one_exported_definition() {
        let unresolved = || LinkedCall {
            kind: "unresolved",
            target: "vendor_child".to_owned(),
            site: Some(0),
            tail: true,
            result_modeled: false,
            semantics: None,
            semantic_operation: None,
            semantic_contract: None,
            replacement_hint: None,
            project_symbol: Some("vendor_child".to_owned()),
            project_candidates: Vec::new(),
            trampoline: None,
            argument_shapes: 1,
            arguments: Vec::new(),
            argument_bindings: Vec::new(),
            typed_arguments: Vec::new(),
            guard_paths: None,
        };

        let mut unique = vec![
            summarize_linked_ir(vec![linked_test_function(
                "parent",
                "vendor_parent",
                "global-or-weak",
                vec![unresolved()],
            )]),
            summarize_linked_ir(vec![linked_test_function(
                "child",
                "vendor_child",
                "global-or-weak",
                Vec::new(),
            )]),
        ];
        link_project_calls(&mut unique);
        let parent = &unique[0].functions[0];
        assert_eq!(parent.calls[0].kind, "project-linked");
        assert_eq!(parent.calls[0].target, "child::vendor_child");
        assert_eq!(parent.dependencies, ["child::vendor_child"]);
        assert!(!parent.complete);

        let mut ambiguous = vec![
            summarize_linked_ir(vec![linked_test_function(
                "parent",
                "vendor_parent",
                "global-or-weak",
                vec![unresolved()],
            )]),
            summarize_linked_ir(vec![linked_test_function(
                "child-a",
                "vendor_child",
                "global-or-weak",
                Vec::new(),
            )]),
            summarize_linked_ir(vec![linked_test_function(
                "child-b",
                "vendor_child",
                "global-or-weak",
                Vec::new(),
            )]),
        ];
        link_project_calls(&mut ambiguous);
        let call = &ambiguous[0].functions[0].calls[0];
        assert_eq!(call.kind, "unresolved");
        assert_eq!(
            call.project_candidates,
            ["child-a::vendor_child", "child-b::vendor_child"]
        );
    }

    #[test]
    fn reachable_effect_summary_keeps_cross_artifact_provenance() {
        let unresolved = LinkedCall {
            kind: "unresolved",
            target: "vendor_child".to_owned(),
            site: Some(0),
            tail: false,
            result_modeled: false,
            semantics: None,
            semantic_operation: None,
            semantic_contract: None,
            replacement_hint: None,
            project_symbol: Some("vendor_child".to_owned()),
            project_candidates: Vec::new(),
            trampoline: None,
            argument_shapes: 1,
            arguments: Vec::new(),
            argument_bindings: Vec::new(),
            typed_arguments: Vec::new(),
            guard_paths: None,
        };
        let mut child = linked_test_function(
            "child",
            "vendor_child",
            "global-or-weak",
            vec![LinkedCall {
                kind: "external",
                target: "wifi_osi::ets_delay_us".to_owned(),
                site: None,
                tail: false,
                result_modeled: true,
                semantics: Some("reviewed delay boundary".to_owned()),
                semantic_operation: Some("time.delay-micros".to_owned()),
                semantic_contract: None,
                replacement_hint: Some("Rust async timer".to_owned()),
                project_symbol: None,
                project_candidates: Vec::new(),
                trampoline: None,
                argument_shapes: 1,
                arguments: vec!["const:0x00000014".to_owned()],
                argument_bindings: Vec::new(),
                typed_arguments: Vec::new(),
                guard_paths: None,
            }],
        );
        child.complete = true;
        child.exact = true;
        child.mmio_accesses.push(LinkedMmioAccess {
            ordinal: 0,
            address: 0x6000_1000,
            width: 32,
            register: "UNKNOWN_60001000".to_owned(),
            access: "write",
            mode: "static",
            path: "entry".to_owned(),
            address_expression: None,
            guard: None,
            predicate_mask: None,
            predicate_expected: None,
            value: Some("const:0x00000001".to_owned()),
            modified_mask: Some(u32::MAX),
            preserved_mask: None,
            inverted_mask: None,
            forced_zero_mask: Some(u32::MAX - 1),
            forced_one_mask: Some(1),
            read_derived_mask: None,
            dynamic_mask: None,
        });
        child.delays.push(LinkedDelay {
            ordinal: 0,
            path: "entry".to_owned(),
            micros: "const:0x00000014".to_owned(),
            constant_micros: Some(20),
        });
        child.context_accesses.push(ContextAccess {
            argument: 0,
            offset: 4,
            access: "write",
            width: 32,
            path: "entry".to_owned(),
            value: Some("const:0x00000001".to_owned()),
            value_pseudo: Some("0x00000001".to_owned()),
            write_mask: Some(u32::MAX),
            preserved_mask: None,
            forced_zero_mask: Some(u32::MAX - 1),
            forced_one_mask: Some(1),
        });

        let mut reports = vec![
            summarize_linked_ir(vec![linked_test_function(
                "parent",
                "vendor_parent",
                "global-or-weak",
                vec![unresolved],
            )]),
            summarize_linked_ir(vec![child]),
        ];
        link_project_calls(&mut reports);
        let report = merge_linked_ir(reports);
        let parent = report
            .functions
            .iter()
            .find(|function| function.identity == "parent::vendor_parent")
            .unwrap();

        assert_eq!(
            parent.effect_summary.reachable_functions,
            ["child::vendor_child"]
        );
        assert_eq!(parent.effect_summary.max_depth, 1);
        assert!(!parent.effect_summary.call_graph_closed);
        assert_eq!(parent.effect_summary.mmio_registers.len(), 1);
        assert_eq!(
            parent.effect_summary.mmio_registers[0].origins,
            ["child::vendor_child"]
        );
        assert_eq!(parent.effect_summary.delays[0].constant_micros, Some(20));
        assert_eq!(
            parent.effect_summary.semantic_operations[0].operation,
            "time.delay-micros"
        );
        assert_eq!(
            parent.effect_summary.semantic_operations[0].origins,
            ["child::vendor_child"]
        );
        assert!(!parent.effect_summary.context_projection_complete);
        assert!(parent.effect_summary.context_fields.is_empty());
        assert!(
            parent
                .effect_summary
                .context_projection_blockers
                .iter()
                .any(|blocker| blocker.contains("no affine binding for child::vendor_child arg0"))
        );
    }

    #[test]
    fn affine_call_bindings_project_transitive_context_fields() {
        let internal = |target: &str, caller_argument: u8, offset: i32| -> LinkedCall {
            LinkedCall {
                kind: "internal",
                target: target.to_owned(),
                site: Some(0x10),
                tail: false,
                result_modeled: false,
                semantics: None,
                semantic_operation: None,
                semantic_contract: None,
                replacement_hint: None,
                project_symbol: None,
                project_candidates: Vec::new(),
                trampoline: None,
                argument_shapes: 1,
                arguments: vec![format!("arg{caller_argument}{offset:+#x}")],
                argument_bindings: vec![LinkedArgumentBinding {
                    position: 0,
                    caller_argument,
                    offset,
                    expression: format!("arg{caller_argument}{offset:+#x}"),
                }],
                typed_arguments: Vec::new(),
                guard_paths: Some(vec![LinkedCallGuardPath {
                    guards: vec![LinkedCallGuard {
                        site: 0x08,
                        condition: "arg1 != 0".to_owned(),
                        operation: "not-equal",
                        taken: true,
                        result_sources: Vec::new(),
                        direct_mmio_sources: Vec::new(),
                    }],
                }]),
            }
        };
        let mut root = linked_test_function(
            "rom",
            "root",
            "global-or-weak",
            vec![internal("rom::middle", 2, 0x20)],
        );
        root.complete = true;
        let mut middle =
            linked_test_function("rom", "middle", "local", vec![internal("rom::leaf", 0, -8)]);
        middle.complete = true;
        let mut leaf = linked_test_function("rom", "leaf", "local", Vec::new());
        leaf.complete = true;
        leaf.calls.push(LinkedCall {
            kind: "external",
            target: "platform::timer_arm".to_owned(),
            site: None,
            tail: false,
            result_modeled: false,
            semantics: Some("typed trampoline".to_owned()),
            semantic_operation: Some("timer.arm-micros".to_owned()),
            semantic_contract: Some(LinkedSemanticContract {
                source: "registered-external-table-slot",
                id: "platform::timer-arm".to_owned(),
                evidence: "exact-pointer-cell-and-slot".to_owned(),
                event_dispatch: None,
            }),
            replacement_hint: Some("Rust async timer registration".to_owned()),
            project_symbol: None,
            project_candidates: Vec::new(),
            trampoline: Some(LinkedTrampoline {
                table: "platform".to_owned(),
                pointer_symbol: "platform_table_ptr".to_owned(),
                backing_symbol: "platform_table".to_owned(),
                version: 1,
                magic: 0x1234_5678,
                table_size: 0x100,
                magic_offset: 0xfc,
                function_id: "timer-arm".to_owned(),
                slot: 0x20,
                c_name: "timer_arm".to_owned(),
                argument_count: 1,
                return_model: "unmodeled".to_owned(),
                operation: "timer.arm-micros".to_owned(),
                return_type: "void".to_owned(),
                replacement_hint: Some("Rust async timer registration".to_owned()),
            }),
            argument_shapes: 1,
            arguments: vec!["arg0 + 0x4".to_owned()],
            argument_bindings: vec![LinkedArgumentBinding {
                position: 0,
                caller_argument: 0,
                offset: 4,
                expression: "arg0 + 0x4".to_owned(),
            }],
            typed_arguments: vec![LinkedCallArgument {
                position: 0,
                name: "timer".to_owned(),
                c_type: "*mut timer".to_owned(),
                direction: "input-output",
                value: "arg0 + 0x4".to_owned(),
            }],
            guard_paths: Some(vec![LinkedCallGuardPath {
                guards: vec![LinkedCallGuard {
                    site: 0x0c,
                    condition: "arg2 == 1".to_owned(),
                    operation: "equal",
                    taken: false,
                    result_sources: Vec::new(),
                    direct_mmio_sources: Vec::new(),
                }],
            }]),
        });
        leaf.context_accesses.push(ContextAccess {
            argument: 0,
            offset: 0x10,
            access: "write",
            width: 32,
            path: "entry / if arg1 != 0".to_owned(),
            value: Some("const:0x00000001".to_owned()),
            value_pseudo: Some("0x00000001".to_owned()),
            write_mask: Some(u32::MAX),
            preserved_mask: None,
            forced_zero_mask: Some(u32::MAX - 1),
            forced_one_mask: Some(1),
        });
        leaf.context_fields = context_fields_for_accesses(&leaf.context_accesses);

        let report = summarize_linked_ir(vec![root, middle, leaf]);
        let root = report
            .functions
            .iter()
            .find(|function| function.identity == "rom::root")
            .unwrap();
        assert!(root.effect_summary.context_projection_complete);
        assert!(root.effect_summary.context_projection_blockers.is_empty());
        assert_eq!(root.effect_summary.context_fields.len(), 1);
        let field = &root.effect_summary.context_fields[0];
        assert_eq!((field.argument, field.offset, field.width), (2, 0x28, 32));
        assert_eq!((field.reads, field.writes), (0, 1));
        assert_eq!(field.write_mask, u32::MAX);
        assert_eq!(field.origins, ["rom::leaf"]);
        assert_eq!(field.write_values, ["0x00000001"]);
        assert!(field.paths[0].contains("rom::root --call@0x00000010--> rom::middle"));
        assert!(field.paths[0].contains("rom::leaf / entry / if arg1 != 0"));
        assert_eq!(root.effect_summary.trampoline_calls.len(), 1);
        let trampoline = &root.effect_summary.trampoline_calls[0];
        assert_eq!(trampoline.origin, "rom::leaf");
        assert_eq!(trampoline.trampoline.slot, 0x20);
        assert_eq!(trampoline.trampoline.operation, "timer.arm-micros");
        assert_eq!(trampoline.arguments[0].binding, "affine-root-context");
        assert_eq!(trampoline.arguments[0].root_argument, Some(2));
        assert_eq!(trampoline.arguments[0].root_offset, Some(0x1c));
        assert_eq!(root.effect_summary.semantic_actions.len(), 1);
        let action = &root.effect_summary.semantic_actions[0];
        assert_eq!(action.operation, "timer.arm-micros");
        assert_eq!(action.origin, "rom::leaf");
        assert_eq!(action.site_path, [Some(0x10), Some(0x10), None]);
        assert!(
            action
                .path
                .contains("rom::root --call@0x00000010--> rom::middle")
        );
        assert!(
            action
                .path
                .ends_with("--semantic@composed--> platform::timer_arm")
        );
        assert_eq!(
            action
                .contract
                .as_ref()
                .map(|contract| contract.id.as_str()),
            Some("platform::timer-arm")
        );
        assert_eq!(action.arguments[0].binding, "affine-root-context");
        assert_eq!(action.arguments[0].root_argument, Some(2));
        assert_eq!(action.arguments[0].root_offset, Some(0x1c));
        let guard_scopes = action.guard_scopes.as_ref().unwrap();
        assert_eq!(guard_scopes.len(), 3);
        assert_eq!(guard_scopes[0].function, "rom::root");
        assert_eq!(guard_scopes[1].function, "rom::middle");
        assert_eq!(guard_scopes[2].function, "rom::leaf");
        assert!(!guard_scopes[2].paths[0].guards[0].taken);
        assert_eq!(report.trampoline_slots.len(), 1);
        assert_eq!(report.trampoline_slots[0].call_shapes, 1);
    }

    #[test]
    fn recursive_effect_summary_reaches_a_fixed_point() {
        let internal = |target: &str| LinkedCall {
            kind: "internal",
            target: target.to_owned(),
            site: Some(0),
            tail: false,
            result_modeled: false,
            semantics: None,
            semantic_operation: None,
            semantic_contract: None,
            replacement_hint: None,
            project_symbol: None,
            project_candidates: Vec::new(),
            trampoline: None,
            argument_shapes: 1,
            arguments: vec!["arg0".to_owned()],
            argument_bindings: vec![LinkedArgumentBinding {
                position: 0,
                caller_argument: 0,
                offset: 0,
                expression: "arg0".to_owned(),
            }],
            typed_arguments: Vec::new(),
            guard_paths: None,
        };
        let mut first = linked_test_function(
            "rom",
            "first",
            "global-or-weak",
            vec![internal("rom::second")],
        );
        first.complete = true;
        first.context_accesses.push(ContextAccess {
            argument: 0,
            offset: 0,
            access: "read",
            width: 32,
            path: "entry".to_owned(),
            value: None,
            value_pseudo: None,
            write_mask: None,
            preserved_mask: None,
            forced_zero_mask: None,
            forced_one_mask: None,
        });
        let mut second = linked_test_function(
            "rom",
            "second",
            "global-or-weak",
            vec![internal("rom::first")],
        );
        second.complete = true;

        let report = summarize_linked_ir(vec![first, second]);
        assert_eq!(report.closed_effect_summaries, 2);
        assert_eq!(report.recursive_effect_summaries, 2);
        for function in &report.functions {
            assert!(function.effect_summary.call_graph_closed);
            assert_eq!(
                function.effect_summary.recursive_functions,
                ["rom::first", "rom::second"]
            );
            assert_eq!(function.effect_summary.reachable_functions.len(), 1);
            assert_eq!(function.effect_summary.max_depth, 1);
            assert!(!function.effect_summary.context_projection_complete);
            assert!(
                function
                    .effect_summary
                    .context_projection_blockers
                    .iter()
                    .any(|blocker| blocker.starts_with("recursive context projection stopped:"))
            );
        }
    }

    #[test]
    fn delay_inventory_preserves_nested_path_and_constant() {
        let flow = DraftReferenceFlow {
            events: vec![DraftReferenceEvent::ComposedCall {
                token: 1,
                symbol: "delay_child".to_owned(),
                arguments: Box::new([]),
                flow: Box::new(DraftReferenceFlow {
                    events: vec![DraftReferenceEvent::DelayMicros {
                        micros: SymbolicValue::Constant(20),
                    }],
                    terminator: DraftReferenceTerminator::Return(SymbolicValue::Constant(0)),
                }),
                result_modeled: true,
            }],
            terminator: DraftReferenceTerminator::Return(SymbolicValue::Constant(0)),
        };
        let mut delays = Vec::new();
        collect_delays_from_flow(&flow, "entry", &mut delays);

        assert_eq!(delays.len(), 1);
        assert_eq!(delays[0].ordinal, 0);
        assert_eq!(delays[0].path, "entry / call delay_child");
        assert_eq!(delays[0].micros, "const:0x00000014");
        assert_eq!(delays[0].constant_micros, Some(20));
    }
}
