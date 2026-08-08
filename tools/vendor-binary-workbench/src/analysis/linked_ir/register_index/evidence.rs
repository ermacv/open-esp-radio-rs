//! Candidate-field evidence accumulation shared by register-index builders.

use super::super::*;

#[derive(Default)]
pub(in crate::analysis::linked_ir) struct MmioFieldCandidateAccumulator {
    pub(in crate::analysis::linked_ir) write_shapes: usize,
    pub(in crate::analysis::linked_ir) predicate_shapes: usize,
    pub(in crate::analysis::linked_ir) poll_shapes: usize,
    pub(in crate::analysis::linked_ir) functions: BTreeSet<String>,
    pub(in crate::analysis::linked_ir) access_functions: BTreeSet<String>,
    pub(in crate::analysis::linked_ir) predicate_functions: BTreeSet<String>,
    pub(in crate::analysis::linked_ir) predicate_evidence:
        BTreeSet<LinkedMmioFieldPredicateEvidence>,
    pub(in crate::analysis::linked_ir) semantic_operations: BTreeSet<String>,
    pub(in crate::analysis::linked_ir) semantic_roots: BTreeSet<String>,
    pub(in crate::analysis::linked_ir) semantic_evidence: BTreeSet<LinkedMmioFieldSemanticEvidence>,
}

#[derive(Default)]
pub(in crate::analysis::linked_ir) struct MmioRegisterAccumulator {
    pub(in crate::analysis::linked_ir) names: BTreeSet<String>,
    pub(in crate::analysis::linked_ir) read_shapes: usize,
    pub(in crate::analysis::linked_ir) write_shapes: usize,
    pub(in crate::analysis::linked_ir) poll_shapes: usize,
    pub(in crate::analysis::linked_ir) predicate_shapes: usize,
    pub(in crate::analysis::linked_ir) static_shapes: usize,
    pub(in crate::analysis::linked_ir) indexed_candidate_shapes: usize,
    pub(in crate::analysis::linked_ir) whole_register_write_shapes: usize,
    pub(in crate::analysis::linked_ir) whole_register_predicate_shapes: usize,
    pub(in crate::analysis::linked_ir) whole_register_poll_shapes: usize,
    pub(in crate::analysis::linked_ir) read_modify_write_shapes: usize,
    pub(in crate::analysis::linked_ir) write_masks: BTreeSet<u32>,
    pub(in crate::analysis::linked_ir) predicate_masks: BTreeSet<u32>,
    pub(in crate::analysis::linked_ir) poll_masks: BTreeSet<u32>,
    pub(in crate::analysis::linked_ir) candidate_bit_ranges:
        BTreeMap<(u8, u8, u32), (usize, BTreeSet<String>)>,
    pub(in crate::analysis::linked_ir) field_candidates:
        BTreeMap<(u8, u8, u32), MmioFieldCandidateAccumulator>,
    pub(in crate::analysis::linked_ir) functions: BTreeSet<String>,
}

pub(in crate::analysis::linked_ir) fn candidate_bit_ranges(
    mask: u32,
    width: u8,
) -> Vec<(u8, u8, u32)> {
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

pub(in crate::analysis::linked_ir) fn record_access_field_mask(
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

pub(in crate::analysis::linked_ir) fn record_predicate_field_mask(
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

pub(in crate::analysis::linked_ir) struct SemanticFieldEvidence<'a> {
    pub(in crate::analysis::linked_ir) kind: &'static str,
    pub(in crate::analysis::linked_ir) mask: u32,
    pub(in crate::analysis::linked_ir) width: u8,
    pub(in crate::analysis::linked_ir) operation: &'a str,
    pub(in crate::analysis::linked_ir) root: &'a str,
    pub(in crate::analysis::linked_ir) action_target: &'a str,
    pub(in crate::analysis::linked_ir) action_origin: &'a str,
    pub(in crate::analysis::linked_ir) action_site: Option<u32>,
    pub(in crate::analysis::linked_ir) action_site_path: &'a [Option<u32>],
    pub(in crate::analysis::linked_ir) action_path: &'a str,
    pub(in crate::analysis::linked_ir) predicate_function: &'a str,
    pub(in crate::analysis::linked_ir) producer: Option<&'a str>,
    pub(in crate::analysis::linked_ir) producer_path: &'a [String],
    pub(in crate::analysis::linked_ir) scope_index: usize,
    pub(in crate::analysis::linked_ir) scope_alternatives: usize,
    pub(in crate::analysis::linked_ir) path_index: usize,
    pub(in crate::analysis::linked_ir) path_expression: &'a str,
    pub(in crate::analysis::linked_ir) path_guards: usize,
    pub(in crate::analysis::linked_ir) guard_index: usize,
    pub(in crate::analysis::linked_ir) residual_path_expression: &'a str,
    pub(in crate::analysis::linked_ir) site: u32,
    pub(in crate::analysis::linked_ir) condition: &'a str,
    pub(in crate::analysis::linked_ir) taken: bool,
    pub(in crate::analysis::linked_ir) guard_operation: &'static str,
}

pub(in crate::analysis::linked_ir) fn record_semantic_field_link(
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
