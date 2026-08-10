//! Data model shared by linked-IR analysis and report renderers.

use serde::Serialize;

use crate::MemoryObjectRoot;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub(crate) struct LinkedCallArgument {
    pub(crate) position: usize,
    pub(crate) name: String,
    pub(crate) c_type: String,
    pub(crate) direction: &'static str,
    pub(crate) value: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub(crate) struct LinkedArgumentBinding {
    pub(crate) position: usize,
    pub(crate) caller_argument: u8,
    pub(crate) offset: i32,
    pub(crate) expression: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
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

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub(crate) struct LinkedEventDispatchArgumentRole {
    pub(crate) role: &'static str,
    pub(crate) argument: &'static str,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub(crate) struct LinkedEventDispatchContract {
    pub(crate) mechanism: &'static str,
    pub(crate) execution_context: &'static str,
    pub(crate) receiver: Option<&'static str>,
    pub(crate) argument_roles: Vec<LinkedEventDispatchArgumentRole>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub(crate) struct LinkedSemanticContract {
    pub(crate) source: &'static str,
    pub(crate) id: String,
    pub(crate) evidence: String,
    pub(crate) event_dispatch: Option<LinkedEventDispatchContract>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
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

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub(crate) struct LinkedReturnProvenance {
    pub(crate) exact: bool,
    pub(crate) known_zero_bits: u32,
    pub(crate) known_one_bits: u32,
    pub(crate) unknown_bits: u32,
    pub(crate) sources: Vec<LinkedReturnBitSource>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
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

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
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

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
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

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub(crate) struct LinkedDirectMmioPredicate {
    pub(crate) site: u32,
    pub(crate) condition: String,
    pub(crate) operation: &'static str,
    pub(crate) sources: Vec<LinkedDirectMmioPredicateSource>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub(crate) struct LinkedCallGuard {
    pub(crate) site: u32,
    pub(crate) condition: String,
    pub(crate) operation: &'static str,
    pub(crate) taken: bool,
    pub(crate) result_sources: Vec<LinkedCallGuardResultSource>,
    pub(crate) direct_mmio_sources: Vec<LinkedDirectMmioPredicateSource>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub(crate) struct LinkedCallGuardPath {
    pub(crate) guards: Vec<LinkedCallGuard>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub(crate) struct LinkedCallGuardScope {
    pub(crate) function: String,
    pub(crate) paths: Vec<LinkedCallGuardPath>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
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

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
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

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum LinkedMemoryObject {
    Argument {
        index: u8,
    },
    Global {
        member: Option<String>,
        symbol: String,
    },
    Dereferenced {
        pointer: Box<LinkedMemoryObject>,
        pointer_offset: i64,
    },
    Absolute {
        address_space: String,
        address: u32,
    },
    Indexed {
        object: Box<LinkedMemoryObject>,
        argument: u8,
        stride: i64,
    },
}

impl LinkedMemoryObject {
    pub(crate) fn from_root(value: MemoryObjectRoot, address_space: &str) -> Self {
        Self::from_root_ref(&value, address_space)
    }

    fn from_root_ref(value: &MemoryObjectRoot, address_space: &str) -> Self {
        match value {
            MemoryObjectRoot::Argument { index } => Self::Argument { index: *index },
            MemoryObjectRoot::RelocatedSymbol { member, symbol } => Self::Global {
                member: member.clone(),
                symbol: symbol.clone(),
            },
            MemoryObjectRoot::Dereferenced {
                pointer,
                pointer_offset,
            } => Self::Dereferenced {
                pointer: Box::new(Self::from_root_ref(pointer, address_space)),
                pointer_offset: *pointer_offset,
            },
            MemoryObjectRoot::Absolute { address } => Self::Absolute {
                address_space: address_space.to_owned(),
                address: *address,
            },
            MemoryObjectRoot::Indexed {
                root,
                argument,
                stride,
            } => Self::Indexed {
                object: Box::new(Self::from_root_ref(root, address_space)),
                argument: *argument,
                stride: *stride,
            },
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub(crate) struct MemoryObjectAccess {
    pub(crate) object: LinkedMemoryObject,
    pub(crate) offset: i64,
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct MemoryObjectField {
    pub(crate) object: LinkedMemoryObject,
    pub(crate) offset: i64,
    pub(crate) width: u8,
    pub(crate) reads: usize,
    pub(crate) writes: usize,
    pub(crate) write_mask: u32,
    pub(crate) paths: Vec<String>,
    pub(crate) write_values: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
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

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct LinkedMmioBitRange {
    pub(crate) least_significant_bit: u8,
    pub(crate) most_significant_bit: u8,
    pub(crate) mask: u32,
    pub(crate) write_shapes: usize,
    pub(crate) functions: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
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

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
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

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
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

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub(crate) struct LinkedDelay {
    pub(crate) ordinal: usize,
    pub(crate) path: String,
    pub(crate) micros: String,
    pub(crate) constant_micros: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct LinkedSummaryMmio {
    pub(crate) address: u32,
    pub(crate) width: u8,
    pub(crate) access_shapes: usize,
    pub(crate) accesses: Vec<&'static str>,
    pub(crate) modes: Vec<&'static str>,
    pub(crate) origins: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct LinkedSummaryDelay {
    pub(crate) micros: String,
    pub(crate) constant_micros: Option<u32>,
    pub(crate) delay_shapes: usize,
    pub(crate) origins: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct LinkedSummarySemantic {
    pub(crate) operation: String,
    pub(crate) call_shapes: usize,
    pub(crate) targets: Vec<String>,
    pub(crate) replacement_hints: Vec<String>,
    pub(crate) origins: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct LinkedSummaryMemoryField {
    pub(crate) object: LinkedMemoryObject,
    pub(crate) offset: i64,
    pub(crate) width: u8,
    pub(crate) reads: usize,
    pub(crate) writes: usize,
    pub(crate) write_mask: u32,
    pub(crate) origins: Vec<String>,
    pub(crate) paths: Vec<String>,
    pub(crate) write_values: Vec<String>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
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

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub(crate) struct LinkedProjectedTrampolineCall {
    pub(crate) trampoline: LinkedTrampoline,
    pub(crate) origin: String,
    pub(crate) path: String,
    pub(crate) argument_shapes: usize,
    pub(crate) arguments: Vec<LinkedProjectedCallArgument>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
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

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub(crate) struct LinkedEventDispatchBinding {
    pub(crate) role: &'static str,
    pub(crate) argument: LinkedProjectedCallArgument,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct LinkedEventDispatch {
    pub(crate) semantic_action_index: usize,
    pub(crate) mechanism: &'static str,
    pub(crate) execution_context: &'static str,
    pub(crate) receiver: Option<String>,
    pub(crate) interface_complete: bool,
    pub(crate) blockers: Vec<String>,
    pub(crate) bindings: Vec<LinkedEventDispatchBinding>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct LinkedTrampolineSlot {
    pub(crate) trampoline: LinkedTrampoline,
    pub(crate) arguments: Vec<LinkedCallArgument>,
    pub(crate) call_shapes: usize,
    pub(crate) functions: Vec<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
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
    pub(crate) memory_fields: Vec<LinkedSummaryMemoryField>,
    pub(crate) trampoline_calls: Vec<LinkedProjectedTrampolineCall>,
    pub(crate) semantic_actions: Vec<LinkedProjectedSemanticAction>,
    pub(crate) event_dispatches: Vec<LinkedEventDispatch>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct SemanticBoundary {
    pub(crate) operation: String,
    pub(crate) call_shapes: usize,
    pub(crate) functions: Vec<String>,
    pub(crate) targets: Vec<String>,
    pub(crate) replacement_hints: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct LinkedDiagnosticFragment {
    pub(crate) first_ordinal: usize,
    pub(crate) occurrences: usize,
    pub(crate) message: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct LinkedDiagnostic {
    /// Stable identity of the underlying analysis limitation. The identifier
    /// is derived from the classified root fragment, not from presentation
    /// truncation or the function through which it was observed.
    pub(crate) root_id: String,
    pub(crate) kind: &'static str,
    pub(crate) site: Option<u32>,
    pub(crate) rendered: String,
    pub(crate) original_fragments: usize,
    pub(crate) fragments: Vec<LinkedDiagnosticFragment>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub(crate) struct ScenarioArgumentAssignment {
    pub(crate) index: u8,
    pub(crate) value: u32,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub(crate) struct ScenarioMmioReadAssignment {
    pub(crate) address: u32,
    pub(crate) mask: u32,
    pub(crate) expected: u32,
    pub(crate) values: Vec<u32>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub(crate) struct ScenarioSuggestionVariant {
    pub(crate) name: &'static str,
    pub(crate) arguments: Vec<ScenarioArgumentAssignment>,
    pub(crate) mmio_reads: Vec<ScenarioMmioReadAssignment>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub(crate) struct ScenarioSuggestion {
    pub(crate) kind: &'static str,
    pub(crate) site: Option<u32>,
    pub(crate) evidence: String,
    pub(crate) variants: Vec<ScenarioSuggestionVariant>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct LinkedDecodeBlocker {
    pub(crate) address: u64,
    pub(crate) width: u8,
    pub(crate) raw: u32,
    pub(crate) class: &'static str,
    pub(crate) linear_control_flow: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
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
    pub(crate) memory_accesses: Vec<MemoryObjectAccess>,
    pub(crate) memory_fields: Vec<MemoryObjectField>,
    pub(crate) scenario_suggestions: Vec<ScenarioSuggestion>,
    pub(crate) effect_summary: LinkedEffectSummary,
    pub(crate) call_graph_diagnostics: Vec<LinkedDiagnostic>,
    pub(crate) direct_diagnostics: Vec<LinkedDiagnostic>,
    pub(crate) reference_diagnostics: Vec<LinkedDiagnostic>,
    pub(crate) decode_blockers: Vec<LinkedDecodeBlocker>,
    pub(crate) pseudo: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
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
    pub(crate) memory_functions: usize,
    pub(crate) memory_accesses: usize,
    pub(crate) memory_fields: usize,
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
    pub(crate) projected_memory_fields: usize,
    pub(crate) scenario_suggestions: usize,
}
