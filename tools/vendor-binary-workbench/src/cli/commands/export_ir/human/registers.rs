//! MMIO register and candidate-field sections.

use std::fmt::Write as _;

use super::super::*;

pub(super) fn render(mut output: &mut String, report: &LinkedIrReport) {
    for register in &report.mmio_registers {
        let write_masks = register
            .write_masks
            .iter()
            .map(|mask| format!("{mask:#010x}"))
            .collect::<Vec<_>>()
            .join("|");
        let candidate_bit_ranges = register
            .candidate_bit_ranges
            .iter()
            .map(|range| {
                format!(
                    "{}-{}:{:#010x}@{}",
                    range.least_significant_bit,
                    range.most_significant_bit,
                    range.mask,
                    range.functions.join(",")
                )
            })
            .collect::<Vec<_>>()
            .join("|");
        let predicate_masks = register
            .predicate_masks
            .iter()
            .map(|mask| format!("{mask:#010x}"))
            .collect::<Vec<_>>()
            .join("|");
        let poll_masks = register
            .poll_masks
            .iter()
            .map(|mask| format!("{mask:#010x}"))
            .collect::<Vec<_>>()
            .join("|");
        let _ = writeln!(
            &mut output,
            "MMIO-REGISTER\t{:#010x}\twidth={}\tnames={}\tread-shapes={}\twrite-shapes={}\tpoll-shapes={}\tpredicate-shapes={}\tstatic-shapes={}\tindexed-candidates={}\twhole-register-writes={}\twhole-register-predicates={}\twhole-register-polls={}\trmw-writes={}\twrite-masks={}\tpredicate-masks={}\tpoll-masks={}\tcandidate-bit-ranges={}\tfield-candidates={}\tfunctions={}",
            register.address,
            register.width,
            register.names.join("|"),
            register.read_shapes,
            register.write_shapes,
            register.poll_shapes,
            register.predicate_shapes,
            register.static_shapes,
            register.indexed_candidate_shapes,
            register.whole_register_write_shapes,
            register.whole_register_predicate_shapes,
            register.whole_register_poll_shapes,
            register.read_modify_write_shapes,
            write_masks,
            predicate_masks,
            poll_masks,
            candidate_bit_ranges,
            register.field_candidates.len(),
            register.functions.join(","),
        );
        for candidate in &register.field_candidates {
            let _ = writeln!(
                &mut output,
                "MMIO-FIELD-CANDIDATE\t{:#010x}\twidth={}\tregisters={}\tbits={}-{}\tmask={:#010x}\twrite-shapes={}\tpredicate-shapes={}\tpoll-shapes={}\tfunctions={}\taccess-functions={}\tpredicate-functions={}\tsemantic-operations={}\tsemantic-roots={}",
                register.address,
                register.width,
                register.names.join("|"),
                candidate.least_significant_bit,
                candidate.most_significant_bit,
                candidate.mask,
                candidate.write_shapes,
                candidate.predicate_shapes,
                candidate.poll_shapes,
                candidate.functions.join(","),
                candidate.access_functions.join(","),
                candidate.predicate_functions.join(","),
                candidate.semantic_operations.join("|"),
                candidate.semantic_roots.join(","),
            );
            for evidence in &candidate.predicate_evidence {
                let optional_hex = |value: Option<u32>| {
                    value.map_or_else(|| "-".to_owned(), |value| format!("{value:#010x}"))
                };
                let _ = writeln!(
                    &mut output,
                    "MMIO-FIELD-PREDICATE\t{:#010x}\tbits={}-{}\tkind={}\tfunction={}\tproducer={}\tproducer-path={}\tsite={}\tpath={}\tcondition={}\toperation={}\ttaken={}\teffective-operation={}\toperand={}\tcomparison-value={}\tregister-comparison-value={}\tinverted={}",
                    register.address,
                    candidate.least_significant_bit,
                    candidate.most_significant_bit,
                    evidence.kind,
                    evidence.function,
                    evidence.producer.as_deref().unwrap_or("-"),
                    if evidence.producer_path.is_empty() {
                        "-".to_owned()
                    } else {
                        evidence.producer_path.join(" -> ")
                    },
                    evidence
                        .site
                        .map_or_else(|| "-".to_owned(), |site| format!("{site:#010x}")),
                    evidence.path.as_deref().unwrap_or("-"),
                    evidence.condition,
                    evidence.operation,
                    evidence
                        .taken
                        .map_or_else(|| "-".to_owned(), |taken| taken.to_string()),
                    evidence.effective_operation.unwrap_or("-"),
                    evidence.operand.unwrap_or("-"),
                    optional_hex(evidence.comparison_value),
                    optional_hex(evidence.register_comparison_value),
                    evidence.inverted,
                );
            }
            for evidence in &candidate.semantic_evidence {
                let _ = writeln!(
                    &mut output,
                    "MMIO-FIELD-SEMANTIC\t{:#010x}\tbits={}-{}\tkind={}\troot={}\toperation={}\taction-target={}\taction-origin={}\taction-site={}\taction-site-path={}\tpredicate-function={}\tproducer={}\tproducer-path={}\tscope-index={}\tscope-alternatives={}\tpath-index={}\tpath-guards={}\tguard-index={}\tsite={:#010x}\tcondition={}\ttaken={}\teffective-operation={}\tresidual-path={}\tpath-expression={}\taction-path={}",
                    register.address,
                    candidate.least_significant_bit,
                    candidate.most_significant_bit,
                    evidence.kind,
                    evidence.root,
                    evidence.operation,
                    evidence.action_target,
                    evidence.action_origin,
                    evidence
                        .action_site
                        .map_or_else(|| "-".to_owned(), |site| format!("{site:#010x}")),
                    format_site_path(&evidence.action_site_path),
                    evidence.predicate_function,
                    evidence.producer.as_deref().unwrap_or("-"),
                    evidence.producer_path.join(" -> "),
                    evidence.scope_index,
                    evidence.scope_alternatives,
                    evidence.path_index,
                    evidence.path_guards,
                    evidence.guard_index,
                    evidence.site,
                    evidence.condition,
                    evidence.taken,
                    evidence.effective_operation,
                    evidence.residual_path_expression,
                    evidence.path_expression,
                    evidence.action_path,
                );
            }
        }
    }
}
