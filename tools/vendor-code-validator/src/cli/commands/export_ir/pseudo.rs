//! Pseudo-Rust linked-IR report rendering.

use std::fmt::Write as _;

use super::*;

pub(super) fn write_pseudo(
    path: &Path,
    artifacts: &[IrArtifactInput],
    report: &LinkedIrReport,
    include_reachable: bool,
) -> Result<()> {
    let mut output = String::new();
    output.push_str("// Best-effort vendor-code pseudo-Rust generated from:\n");
    for artifact in artifacts {
        writeln!(
            output,
            "// - {}: {}",
            artifact.source,
            artifact.path.display()
        )
        .expect("writing to String cannot fail");
    }
    if artifacts.len() > 1 {
        output.push_str(
            "// Named primary artifacts use independent address spaces; unique exported-symbol edges provide reachable inventories, and only recovered affine call bindings project context fields.\n",
        );
    }
    writeln!(
        output,
        "// Selection: {}.",
        if include_reachable {
            "symbol-prefix roots plus reachable internal callees from each primary artifact"
        } else {
            "symbol-prefix roots only"
        }
    )
    .expect("writing to String cannot fail");
    output
        .push_str("// This is analysis IR, not compilable Rust and not a completeness claim.\n\n");
    for register in &report.mmio_registers {
        for candidate in &register.field_candidates {
            writeln!(
                output,
                "// MMIO-FIELD-CANDIDATE: {:#010x} [{}] bits={}-{} mask={:#010x} writes={} predicates={} polls={}",
                register.address,
                register.names.join(" | "),
                candidate.least_significant_bit,
                candidate.most_significant_bit,
                candidate.mask,
                candidate.write_shapes,
                candidate.predicate_shapes,
                candidate.poll_shapes,
            )
            .expect("writing to String cannot fail");
            writeln!(
                output,
                "//   FUNCTIONS: {}",
                candidate.functions.join(" | ")
            )
            .expect("writing to String cannot fail");
            if !candidate.semantic_operations.is_empty() {
                writeln!(
                    output,
                    "//   GUARDED-SEMANTICS: {} (roots: {})",
                    candidate.semantic_operations.join(" | "),
                    candidate.semantic_roots.join(" | ")
                )
                .expect("writing to String cannot fail");
            }
            for evidence in &candidate.predicate_evidence {
                writeln!(
                    output,
                    "//   PREDICATE: kind={} function={} producer={} producer-path={} condition=({}) operation={} taken={} effective={} comparison={} register-value={}{}",
                    evidence.kind,
                    evidence.function,
                    evidence.producer.as_deref().unwrap_or("-"),
                    if evidence.producer_path.is_empty() {
                        "-".to_owned()
                    } else {
                        evidence.producer_path.join(" -> ")
                    },
                    evidence.condition,
                    evidence.operation,
                    evidence
                        .taken
                        .map_or_else(|| "-".to_owned(), |taken| taken.to_string()),
                    evidence.effective_operation.unwrap_or("-"),
                    evidence
                        .comparison_value
                        .map_or_else(|| "-".to_owned(), |value| format!("{value:#010x}")),
                    evidence.register_comparison_value.map_or_else(
                        || "-".to_owned(),
                        |value| format!("{value:#010x}")
                    ),
                    if evidence.inverted { " inverted" } else { "" },
                )
                .expect("writing to String cannot fail");
            }
            for evidence in &candidate.semantic_evidence {
                writeln!(
                    output,
                    "//   GUARDED-SEMANTIC-ACTION: {} target={} origin={} root={} action-sites={}{}",
                    evidence.operation,
                    evidence.action_target,
                    evidence.action_origin,
                    evidence.root,
                    format_site_path(&evidence.action_site_path),
                    evidence
                        .action_site
                        .map_or_else(String::new, |site| format!(" action-site={site:#010x}")),
                )
                .expect("writing to String cannot fail");
                writeln!(
                    output,
                    "//     MMIO-GUARD: in {} scope={} path={}/{} guard={}/{} site={:#010x} effective={} [{} taken={}]{} producer-path={}",
                    evidence.predicate_function,
                    evidence.scope_index + 1,
                    evidence.path_index + 1,
                    evidence.scope_alternatives,
                    evidence.guard_index + 1,
                    evidence.path_guards,
                    evidence.site,
                    evidence.effective_operation,
                    evidence.condition,
                    evidence.taken,
                    evidence
                        .producer
                        .as_ref()
                        .map_or_else(String::new, |producer| format!(" producer={producer}")),
                    evidence.producer_path.join(" -> "),
                )
                .expect("writing to String cannot fail");
                writeln!(
                    output,
                    "//     RESIDUAL-PATH: {}",
                    evidence.residual_path_expression,
                )
                .expect("writing to String cannot fail");
                writeln!(output, "//     SELECTED-PATH: {}", evidence.path_expression,)
                    .expect("writing to String cannot fail");
            }
        }
    }
    if report
        .mmio_registers
        .iter()
        .any(|register| !register.field_candidates.is_empty())
    {
        output.push('\n');
    }
    for function in &report.functions {
        let summary = &function.effect_summary;
        writeln!(output, "// SELECTION: {}", function.selection)
            .expect("writing to String cannot fail");
        writeln!(
            output,
            "// REACHABLE-EFFECTS: call-graph-closed={} max-depth={} functions={} mmio={} delays={} semantics={} semantic-actions={} event-dispatches={} trampolines={} context-fields={} context-projection-complete={} blockers={}",
            summary.call_graph_closed,
            summary.max_depth,
            summary.reachable_functions.len(),
            summary.mmio_registers.len(),
            summary.delays.len(),
            summary.semantic_operations.len(),
            summary.semantic_actions.len(),
            summary.event_dispatches.len(),
            summary.trampoline_calls.len(),
            summary.context_fields.len(),
            summary.context_projection_complete,
            summary.blockers.len(),
        )
        .expect("writing to String cannot fail");
        if !summary.reachable_functions.is_empty() {
            writeln!(
                output,
                "// REACHABLE-FUNCTIONS: {}",
                summary.reachable_functions.join(" | ")
            )
            .expect("writing to String cannot fail");
        }
        for semantic in &summary.semantic_operations {
            writeln!(
                output,
                "// REACHABLE-SEMANTIC: {} via {}",
                semantic.operation,
                semantic.origins.join(" | ")
            )
            .expect("writing to String cannot fail");
        }
        for dispatch in &summary.event_dispatches {
            let action = &summary.semantic_actions[dispatch.semantic_action_index];
            writeln!(
                output,
                "// EVENT-DISPATCH: mechanism={} context={} receiver={} interface-complete={} action={} operation={} target={}",
                dispatch.mechanism,
                dispatch.execution_context,
                dispatch.receiver.as_deref().unwrap_or("unknown"),
                dispatch.interface_complete,
                dispatch.semantic_action_index + 1,
                action.operation,
                action.target,
            )
            .expect("writing to String cannot fail");
            writeln!(output, "//   route: {}", action.path).expect("writing to String cannot fail");
            for binding in &dispatch.bindings {
                let argument = &binding.argument;
                let projected = match (argument.root_argument, argument.root_offset) {
                    (Some(root_argument), Some(root_offset)) => {
                        format!("ctx{root_argument} {root_offset:+#x}")
                    }
                    _ => argument.value.clone(),
                };
                writeln!(
                    output,
                    "//   {} {}: {} = {}",
                    binding.role, argument.name, argument.c_type, projected,
                )
                .expect("writing to String cannot fail");
            }
            match action.guard_scopes.as_deref() {
                Some([]) => output.push_str("//   when: true\n"),
                Some(scopes) => {
                    for scope in scopes {
                        writeln!(
                            output,
                            "//   when in {}: {}",
                            scope.function,
                            format_guard_paths(&scope.paths),
                        )
                        .expect("writing to String cannot fail");
                    }
                }
                None => output.push_str("//   when: unknown (CFG guard unavailable)\n"),
            }
            if !dispatch.blockers.is_empty() {
                writeln!(
                    output,
                    "//   interface blockers: {}",
                    dispatch.blockers.join(" | "),
                )
                .expect("writing to String cannot fail");
            }
        }
        for action in &summary.semantic_actions {
            let contract = action.contract.as_ref().map_or_else(
                || "unqualified".to_owned(),
                |contract| {
                    format!(
                        "{}:{} evidence={}",
                        contract.source, contract.id, contract.evidence
                    )
                },
            );
            writeln!(
                output,
                "// REACHABLE-ACTION: {}({}) argument-shapes={} via {} [contract={}]{}",
                action.operation,
                action.target,
                action.argument_shapes,
                action.path,
                contract,
                action
                    .replacement_hint
                    .as_ref()
                    .map_or_else(String::new, |hint| { format!(" [replacement={hint}]") }),
            )
            .expect("writing to String cannot fail");
            writeln!(
                output,
                "//   lexical-site-path: {}",
                format_site_path(&action.site_path)
            )
            .expect("writing to String cannot fail");
            match action.guard_scopes.as_deref() {
                Some([]) => output.push_str("//   when: true\n"),
                Some(scopes) => {
                    for scope in scopes {
                        writeln!(
                            output,
                            "//   when in {}: {}",
                            scope.function,
                            format_guard_paths(&scope.paths)
                        )
                        .expect("writing to String cannot fail");
                        for (
                            site,
                            condition,
                            operation,
                            taken,
                            producer,
                            operand,
                            comparison_value,
                            source_comparison_value,
                            mmio,
                        ) in guard_mmio_links(&scope.paths)
                        {
                            writeln!(
                                output,
                                "//     MMIO-PREDICATE-SOURCE: {}@{:#010x} result-bits={:#010x} register-bits={:#010x} producer={} producer-path={} return-depth={} site={:#010x} operation={} taken={} effective={} operand={} comparison={} source-value={} result-value={} register-value={} condition=({}){}",
                                mmio.register,
                                mmio.address,
                                mmio.result_bits,
                                mmio.register_bits,
                                producer,
                                mmio.producer_path.join(" -> "),
                                mmio.producer_path.len().saturating_sub(1),
                                site,
                                operation,
                                taken,
                                effective_branch_operation(operation, taken),
                                operand,
                                optional_hex_text(comparison_value),
                                optional_hex_text(source_comparison_value),
                                optional_hex_text(mmio.result_comparison_value),
                                optional_hex_text(mmio.register_comparison_value),
                                condition,
                                if mmio.inverted { " inverted" } else { "" },
                            )
                            .expect("writing to String cannot fail");
                        }
                        for (site, condition, operation, taken, mmio) in
                            guard_direct_mmio_links(&scope.paths)
                        {
                            writeln!(
                                output,
                                "//     DIRECT-MMIO-PREDICATE: {}@{:#010x} register-bits={:#010x} site={:#010x} operation={} taken={} effective={} operand={} comparison={} register-value={} condition=({}){}",
                                mmio.register,
                                mmio.address,
                                mmio.register_bits,
                                site,
                                operation,
                                taken,
                                effective_branch_operation(operation, taken),
                                mmio.operand,
                                optional_hex_text(mmio.comparison_value),
                                optional_hex_text(mmio.register_comparison_value),
                                condition,
                                if mmio.inverted { " inverted" } else { "" },
                            )
                            .expect("writing to String cannot fail");
                        }
                    }
                }
                None => output.push_str("//   when: unknown (CFG guard unavailable)\n"),
            }
            for argument in &action.arguments {
                let projected = match (argument.root_argument, argument.root_offset) {
                    (Some(root_argument), Some(root_offset)) => {
                        format!("ctx{root_argument} {root_offset:+#x}")
                    }
                    _ => argument.value.clone(),
                };
                writeln!(
                    output,
                    "//   {}: {} = {} ({}, {})",
                    argument.name, argument.c_type, projected, argument.direction, argument.binding,
                )
                .expect("writing to String cannot fail");
            }
        }
        for field in &summary.context_fields {
            writeln!(
                output,
                "// REACHABLE-CONTEXT: ctx{}.field_{:+x} width={} reads={} writes={} mask={:#010x} via {}",
                field.argument,
                field.offset,
                field.width,
                field.reads,
                field.writes,
                field.write_mask,
                field.origins.join(" | ")
            )
            .expect("writing to String cannot fail");
        }
        for call in &summary.trampoline_calls {
            writeln!(
                output,
                "// REACHABLE-TRAMPOLINE: {}+{:#x} {} => {} argument-shapes={} via {}",
                call.trampoline.table,
                call.trampoline.slot,
                call.trampoline.c_name,
                call.trampoline.operation,
                call.argument_shapes,
                call.path
            )
            .expect("writing to String cannot fail");
            for argument in call
                .arguments
                .iter()
                .filter(|argument| argument.root_argument.is_some())
            {
                writeln!(
                    output,
                    "//   {}: {} = ctx{} {:+#x} ({})",
                    argument.name,
                    argument.c_type,
                    argument.root_argument.expect("filtered root argument"),
                    argument.root_offset.expect("bound argument has an offset"),
                    argument.direction,
                )
                .expect("writing to String cannot fail");
            }
        }
        writeln!(
            output,
            "// RETURN-PROVENANCE: exact={} known-zero={:#010x} known-one={:#010x} unknown={:#010x}",
            function.return_provenance.exact,
            function.return_provenance.known_zero_bits,
            function.return_provenance.known_one_bits,
            function.return_provenance.unknown_bits,
        )
        .expect("writing to String cannot fail");
        for source in &function.return_provenance.sources {
            writeln!(
                output,
                "// RETURN-SOURCE: {} output={:#010x} source={:#010x}{}{}{}{}{}{}",
                source.kind,
                source.output_bits,
                source.source_bits,
                source
                    .argument
                    .map_or_else(String::new, |argument| format!(" argument=arg{argument}")),
                source
                    .token
                    .map_or_else(String::new, |token| format!(" token={token}")),
                source
                    .target
                    .as_ref()
                    .map_or_else(String::new, |target| format!(" target={target}")),
                source
                    .address
                    .map_or_else(String::new, |address| format!(" address={address:#010x}")),
                source
                    .register
                    .as_ref()
                    .map_or_else(String::new, |register| format!(" register={register}")),
                if source.inverted { " inverted" } else { "" },
            )
            .expect("writing to String cannot fail");
        }
        for predicate in &function.direct_mmio_predicates {
            for source in &predicate.sources {
                writeln!(
                    output,
                    "// DIRECT-MMIO-PREDICATE: {}@{:#010x} bits={:#010x} site={:#010x} operation={} operand={} comparison={} register-value={} condition=({}){}",
                    source.register,
                    source.address,
                    source.register_bits,
                    predicate.site,
                    predicate.operation,
                    source.operand,
                    source
                        .comparison_value
                        .map_or_else(|| "-".to_owned(), |value| format!("{value:#010x}")),
                    source.register_comparison_value.map_or_else(
                        || "-".to_owned(),
                        |value| format!("{value:#010x}")
                    ),
                    predicate.condition,
                    if source.inverted { " inverted" } else { "" },
                )
                .expect("writing to String cannot fail");
            }
        }
        output.push_str(&function.pseudo);
        output.push('\n');
    }
    fs::write(path, output)?;
    println!("PSEUDO-IR\t{}", path.display());
    Ok(())
}
