//! Persistent MMIO register and field-candidate projection.

use serde::{Deserialize, Serialize};

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
