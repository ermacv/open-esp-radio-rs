//! JSON linked-IR report rendering.

use std::fmt::Write as _;

use super::*;

mod values;

use values::*;

pub(super) fn render_json_report(
    artifacts: &[IrArtifactInput],
    companions: &[PathBuf],
    symbol_prefix: &str,
    entry_contract: EntryContractRef,
    report: &LinkedIrReport,
    include_reachable: bool,
) -> Result<String> {
    let mut output = String::new();
    output.push_str("{\n  \"schema_version\": 30,\n  \"command\": \"ir-export\",\n");
    output.push_str("  \"analysis_mode\": \"best-effort\",\n");
    output.push_str("  \"linkage_mode\": ");
    write_string(
        &mut output,
        if artifacts.len() > 1 {
            "independent-artifacts"
        } else {
            "primary-with-companions"
        },
    );
    output.push_str(",\n  \"project_call_linkage\": ");
    write_string(
        &mut output,
        if artifacts.len() > 1 {
            "unique-exported-symbol-only"
        } else {
            "primary-resolver"
        },
    );
    output.push_str(",\n");
    output.push_str("  \"selection_mode\": ");
    write_string(
        &mut output,
        if include_reachable {
            "symbol-prefix-with-reachable-internal-callees"
        } else {
            "symbol-prefix-only"
        },
    );
    writeln!(output, ",\n  \"include_reachable\": {include_reachable},")
        .expect("writing to String cannot fail");
    output.push_str("  \"effect_summary_mode\": \"reachable-inventory-origin-preserving\",\n");
    output.push_str("  \"call_compaction_mode\": \"stable-identity-universal-affine-bindings\",\n");
    output.push_str("  \"diagnostic_compaction_mode\": \"exact-semicolon-fragment-inventory\",\n");
    output.push_str("  \"context_projection_mode\": \"affine-simple-call-paths\",\n");
    output.push_str(
        "  \"return_provenance_mode\": \"exact-bit-ranges-with-constant-and-unknown-masks\",\n",
    );
    output.push_str(
        "  \"semantic_action_mode\": \"lexical-site-paths-factorized-cfg-guards-affine-root-bindings\",\n",
    );
    output.push_str("  \"event_dispatch_mode\": \"reviewed-contract-declared-role-projection\",\n");
    output.push_str("  \"event_dispatch_effect_completeness_claim\": false,\n");
    output.push_str("  \"event_dispatch_receiver_inference_mode\": \"none\",\n");
    output
        .push_str("  \"event_dispatch_receiver_source_mode\": \"reviewed-contract-or-unknown\",\n");
    output.push_str(
        "  \"cfg_guard_mode\": \"forced-branch-paths-minimized-dnf-factorized-by-function\",\n",
    );
    output.push_str(
        "  \"cfg_guard_expression_mode\": \"pseudo-rust-aligned-bit-masks-with-symbolic-fallback\",\n",
    );
    output.push_str(
        "  \"cfg_guard_result_source_mode\": \"bit-provenance-with-operand-comparison-mapping-and-producer-targets\",\n",
    );
    output.push_str(
        "  \"cfg_guard_mmio_linkage_mode\": \"recursive-exact-bit-projection-with-producer-paths\",\n",
    );
    output.push_str(
        "  \"mmio_field_candidate_mode\": \"contiguous-subregister-write-poll-and-direct-guard-evidence\",\n",
    );
    output.push_str(
        "  \"direct_mmio_predicate_mode\": \"exact-bit-provenance-with-constant-comparison-mapping\",\n",
    );
    output.push_str(
        "  \"semantic_field_guard_mode\": \"action-identity-and-path-coordinate-preserving\",\n",
    );
    output.push_str("  \"direct_mmio_predicate_completeness_claim\": false,\n");
    output.push_str("  \"mmio_field_semantics_claim\": false,\n");
    output.push_str("  \"cfg_guard_completeness_claim\": false,\n");
    output.push_str("  \"trampoline_inventory_mode\": \"registered-versioned-slots-only\",\n");
    output.push_str("  \"completeness_claim\": false,\n  \"artifacts\": [");
    for (index, artifact) in artifacts.iter().enumerate() {
        if index != 0 {
            output.push_str(", ");
        }
        output.push_str("{\"source\": ");
        write_string(&mut output, &artifact.source);
        output.push_str(", \"artifact\": ");
        write_artifact(&mut output, &artifact.path)?;
        output.push('}');
    }
    output.push_str("],\n  \"companions\": [");
    for (index, companion) in companions.iter().enumerate() {
        if index != 0 {
            output.push_str(", ");
        }
        write_artifact(&mut output, companion)?;
    }
    output.push_str("],\n  \"symbol_prefix\": ");
    write_string(&mut output, symbol_prefix);
    output.push_str(",\n  \"entry_contract\": ");
    write_string(&mut output, entry_contract.id());
    let root_functions = report
        .functions
        .iter()
        .filter(|function| function.selection == "symbol-prefix-root")
        .count();
    let included_reachable_functions = report.functions.len() - root_functions;
    let (
        exact_return_functions,
        return_source_ranges,
        mmio_return_sources,
        guard_mmio_links,
        transitive_guard_mmio_links,
    ) = provenance_summary(report);
    let (
        mmio_field_candidate_registers,
        mmio_field_candidates,
        direct_mmio_predicates,
        direct_mmio_predicate_sources,
    ) = field_candidate_summary(report);
    writeln!(
        output,
        ",\n  \"summary\": {{\"artifacts\": {}, \"functions\": {}, \"root_functions\": {}, \"included_reachable_functions\": {}, \"exported\": {}, \"local\": {}, \"mmio_registers\": {}, \"mmio_functions\": {}, \"mmio_access_shapes\": {}, \"mmio_field_candidate_registers\": {}, \"mmio_field_candidates\": {}, \"direct_mmio_predicates\": {}, \"direct_mmio_predicate_sources\": {}, \"delay_functions\": {}, \"delay_shapes\": {}, \"context_functions\": {}, \"context_fields\": {}, \"context_accesses\": {}, \"semantic_operations\": {}, \"semantic_calls\": {}, \"trampoline_slots\": {}, \"trampoline_calls\": {}, \"complete\": {}, \"structured\": {}, \"internal_calls\": {}, \"external_calls\": {}, \"call_argument_shapes\": {}, \"project_linked_calls\": {}, \"ambiguous_project_calls\": {}, \"unresolved_calls\": {}, \"closed_effect_summaries\": {}, \"recursive_effect_summaries\": {}, \"complete_context_projections\": {}, \"projected_context_fields\": {}, \"exact_return_functions\": {}, \"return_source_ranges\": {}, \"mmio_return_sources\": {}, \"guard_mmio_links\": {}, \"transitive_guard_mmio_links\": {}}},",
        artifacts.len(),
        report.functions.len(),
        root_functions,
        included_reachable_functions,
        report.exported_functions,
        report.local_functions,
        report.mmio_registers.len(),
        report.mmio_functions,
        report.mmio_access_shapes,
        mmio_field_candidate_registers,
        mmio_field_candidates,
        direct_mmio_predicates,
        direct_mmio_predicate_sources,
        report.delay_functions,
        report.delay_shapes,
        report.context_functions,
        report.context_fields,
        report.context_accesses,
        report.semantic_boundaries.len(),
        report.semantic_calls,
        report.trampoline_slots.len(),
        report.trampoline_calls,
        report.complete_functions,
        report.structured_functions,
        report.internal_calls,
        report.external_calls,
        report.call_argument_shapes,
        report.project_linked_calls,
        report.ambiguous_project_calls,
        report.unresolved_calls,
        report.closed_effect_summaries,
        report.recursive_effect_summaries,
        report.complete_context_projections,
        report.projected_context_fields,
        exact_return_functions,
        return_source_ranges,
        mmio_return_sources,
        guard_mmio_links,
        transitive_guard_mmio_links,
    )
    .expect("writing to String cannot fail");
    output.push_str("  \"mmio_registers\": [");
    for (index, register) in report.mmio_registers.iter().enumerate() {
        if index != 0 {
            output.push_str(", ");
        }
        write!(
            output,
            "{{\"address\": \"{:#010x}\", \"width\": {}, \"names\": ",
            register.address, register.width
        )
        .expect("writing to String cannot fail");
        write_strings(&mut output, &register.names);
        write!(
            output,
            ", \"read_shapes\": {}, \"write_shapes\": {}, \"poll_shapes\": {}, \"predicate_shapes\": {}, \"static_shapes\": {}, \"indexed_candidate_shapes\": {}, \"whole_register_write_shapes\": {}, \"whole_register_predicate_shapes\": {}, \"whole_register_poll_shapes\": {}, \"read_modify_write_shapes\": {}, \"write_masks\": [",
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
        )
        .expect("writing to String cannot fail");
        for (mask_index, mask) in register.write_masks.iter().enumerate() {
            if mask_index != 0 {
                output.push_str(", ");
            }
            write_string(&mut output, &format!("{mask:#010x}"));
        }
        output.push_str("], \"predicate_masks\": [");
        for (mask_index, mask) in register.predicate_masks.iter().enumerate() {
            if mask_index != 0 {
                output.push_str(", ");
            }
            write_string(&mut output, &format!("{mask:#010x}"));
        }
        output.push_str("], \"poll_masks\": [");
        for (mask_index, mask) in register.poll_masks.iter().enumerate() {
            if mask_index != 0 {
                output.push_str(", ");
            }
            write_string(&mut output, &format!("{mask:#010x}"));
        }
        output.push_str("], \"candidate_bit_ranges\": [");
        for (range_index, range) in register.candidate_bit_ranges.iter().enumerate() {
            if range_index != 0 {
                output.push_str(", ");
            }
            write!(
                output,
                "{{\"least_significant_bit\": {}, \"most_significant_bit\": {}, \"mask\": \"{:#010x}\", \"write_shapes\": {}, \"functions\": ",
                range.least_significant_bit,
                range.most_significant_bit,
                range.mask,
                range.write_shapes,
            )
            .expect("writing to String cannot fail");
            write_strings(&mut output, &range.functions);
            output.push('}');
        }
        output.push_str("], \"field_candidates\": [");
        for (candidate_index, candidate) in register.field_candidates.iter().enumerate() {
            if candidate_index != 0 {
                output.push_str(", ");
            }
            write!(
                output,
                "{{\"least_significant_bit\": {}, \"most_significant_bit\": {}, \"mask\": \"{:#010x}\", \"write_shapes\": {}, \"predicate_shapes\": {}, \"poll_shapes\": {}, \"functions\": ",
                candidate.least_significant_bit,
                candidate.most_significant_bit,
                candidate.mask,
                candidate.write_shapes,
                candidate.predicate_shapes,
                candidate.poll_shapes,
            )
            .expect("writing to String cannot fail");
            write_strings(&mut output, &candidate.functions);
            output.push_str(", \"access_functions\": ");
            write_strings(&mut output, &candidate.access_functions);
            output.push_str(", \"predicate_functions\": ");
            write_strings(&mut output, &candidate.predicate_functions);
            output.push_str(", \"predicate_evidence\": [");
            for (evidence_index, evidence) in candidate.predicate_evidence.iter().enumerate() {
                if evidence_index != 0 {
                    output.push_str(", ");
                }
                output.push_str("{\"kind\": ");
                write_string(&mut output, evidence.kind);
                output.push_str(", \"function\": ");
                write_string(&mut output, &evidence.function);
                output.push_str(", \"producer\": ");
                write_optional_string(&mut output, evidence.producer.as_deref());
                output.push_str(", \"producer_path\": ");
                write_strings(&mut output, &evidence.producer_path);
                output.push_str(", \"site\": ");
                write_optional_hex(&mut output, evidence.site);
                output.push_str(", \"path\": ");
                write_optional_string(&mut output, evidence.path.as_deref());
                output.push_str(", \"condition\": ");
                write_string(&mut output, &evidence.condition);
                output.push_str(", \"operation\": ");
                write_string(&mut output, evidence.operation);
                output.push_str(", \"taken\": ");
                if let Some(taken) = evidence.taken {
                    write!(output, "{taken}").expect("writing to String cannot fail");
                } else {
                    output.push_str("null");
                }
                output.push_str(", \"effective_operation\": ");
                write_optional_string(&mut output, evidence.effective_operation);
                output.push_str(", \"operand\": ");
                write_optional_string(&mut output, evidence.operand);
                output.push_str(", \"comparison_value\": ");
                write_optional_hex(&mut output, evidence.comparison_value);
                output.push_str(", \"register_comparison_value\": ");
                write_optional_hex(&mut output, evidence.register_comparison_value);
                write!(output, ", \"inverted\": {}}}", evidence.inverted)
                    .expect("writing to String cannot fail");
            }
            output.push(']');
            output.push_str(", \"semantic_operations\": ");
            write_strings(&mut output, &candidate.semantic_operations);
            output.push_str(", \"semantic_roots\": ");
            write_strings(&mut output, &candidate.semantic_roots);
            output.push_str(", \"semantic_evidence\": [");
            for (evidence_index, evidence) in candidate.semantic_evidence.iter().enumerate() {
                if evidence_index != 0 {
                    output.push_str(", ");
                }
                output.push_str("{\"kind\": ");
                write_string(&mut output, evidence.kind);
                output.push_str(", \"root\": ");
                write_string(&mut output, &evidence.root);
                output.push_str(", \"operation\": ");
                write_string(&mut output, &evidence.operation);
                output.push_str(", \"action_target\": ");
                write_string(&mut output, &evidence.action_target);
                output.push_str(", \"action_origin\": ");
                write_string(&mut output, &evidence.action_origin);
                output.push_str(", \"action_site\": ");
                write_optional_hex(&mut output, evidence.action_site);
                output.push_str(", \"action_site_path\": ");
                write_site_path(&mut output, &evidence.action_site_path);
                output.push_str(", \"action_path\": ");
                write_string(&mut output, &evidence.action_path);
                output.push_str(", \"predicate_function\": ");
                write_string(&mut output, &evidence.predicate_function);
                output.push_str(", \"producer\": ");
                write_optional_string(&mut output, evidence.producer.as_deref());
                output.push_str(", \"producer_path\": ");
                write_strings(&mut output, &evidence.producer_path);
                write!(
                    output,
                    ", \"scope_index\": {}, \"scope_alternatives\": {}, \"path_index\": {}, \"path_expression\": ",
                    evidence.scope_index, evidence.scope_alternatives, evidence.path_index,
                )
                .expect("writing to String cannot fail");
                write_string(&mut output, &evidence.path_expression);
                write!(
                    output,
                    ", \"path_guards\": {}, \"guard_index\": {}, \"residual_path_expression\": ",
                    evidence.path_guards, evidence.guard_index,
                )
                .expect("writing to String cannot fail");
                write_string(&mut output, &evidence.residual_path_expression);
                write!(
                    output,
                    ", \"site\": \"{:#010x}\", \"condition\": ",
                    evidence.site
                )
                .expect("writing to String cannot fail");
                write_string(&mut output, &evidence.condition);
                write!(output, ", \"taken\": {}", evidence.taken)
                    .expect("writing to String cannot fail");
                output.push_str(", \"effective_operation\": ");
                write_string(&mut output, evidence.effective_operation);
                output.push('}');
            }
            output.push(']');
            output.push('}');
        }
        output.push_str("], \"functions\": ");
        write_strings(&mut output, &register.functions);
        output.push('}');
    }
    output.push_str("],\n");
    output.push_str("  \"semantic_boundaries\": [");
    for (index, boundary) in report.semantic_boundaries.iter().enumerate() {
        if index != 0 {
            output.push_str(", ");
        }
        output.push_str("{\"operation\": ");
        write_string(&mut output, &boundary.operation);
        write!(
            output,
            ", \"call_shapes\": {}, \"functions\": ",
            boundary.call_shapes
        )
        .expect("writing to String cannot fail");
        write_strings(&mut output, &boundary.functions);
        output.push_str(", \"targets\": ");
        write_strings(&mut output, &boundary.targets);
        output.push_str(", \"replacement_hints\": ");
        write_strings(&mut output, &boundary.replacement_hints);
        output.push('}');
    }
    output.push_str("],\n");
    output.push_str("  \"trampoline_slots\": [");
    for (index, slot) in report.trampoline_slots.iter().enumerate() {
        if index != 0 {
            output.push_str(", ");
        }
        output.push_str("{\"trampoline\": ");
        write_trampoline(&mut output, &slot.trampoline);
        write!(
            output,
            ", \"call_shapes\": {}, \"functions\": ",
            slot.call_shapes
        )
        .expect("writing to String cannot fail");
        write_strings(&mut output, &slot.functions);
        output.push_str(", \"arguments\": [");
        for (argument_index, argument) in slot.arguments.iter().enumerate() {
            if argument_index != 0 {
                output.push_str(", ");
            }
            write!(output, "{{\"position\": {}, \"name\": ", argument.position)
                .expect("writing to String cannot fail");
            write_string(&mut output, &argument.name);
            output.push_str(", \"c_type\": ");
            write_string(&mut output, &argument.c_type);
            output.push_str(", \"direction\": ");
            write_string(&mut output, argument.direction);
            output.push('}');
        }
        output.push_str("]}");
    }
    output.push_str("],\n");
    output.push_str("  \"functions\": [\n");
    for (index, function) in report.functions.iter().enumerate() {
        output.push_str("    {\"source\": ");
        write_string(&mut output, &function.source);
        output.push_str(", \"identity\": ");
        write_string(&mut output, &function.identity);
        output.push_str(", \"selection\": ");
        write_string(&mut output, function.selection);
        output.push_str(", \"member\": ");
        write_optional_string(&mut output, function.member.as_deref());
        output.push_str(", \"symbol\": ");
        write_string(&mut output, &function.symbol);
        output.push_str(", \"binding\": ");
        write_string(&mut output, function.binding);
        output.push_str(", \"address\": ");
        if let Some(address) = function.address {
            write_string(&mut output, &format!("{address:#010x}"));
        } else {
            output.push_str("null");
        }
        write!(
            output,
            ", \"object_offset\": \"{:#010x}\", \"size\": {}, \"flow_kind\": ",
            function.object_offset, function.size
        )
        .expect("writing to String cannot fail");
        write_string(&mut output, function.flow_kind);
        write!(
            output,
            ", \"complete\": {}, \"exact\": {}, \"return_value\": ",
            function.complete, function.exact
        )
        .expect("writing to String cannot fail");
        write_string(&mut output, &function.return_value);
        output.push_str(", \"return_provenance\": ");
        write_return_provenance(&mut output, &function.return_provenance);
        output.push_str(", \"dependencies\": ");
        write_strings(&mut output, &function.dependencies);
        output.push_str(", \"calls\": [");
        for (call_index, call) in function.calls.iter().enumerate() {
            if call_index != 0 {
                output.push_str(", ");
            }
            output.push_str("{\"kind\": ");
            write_string(&mut output, call.kind);
            output.push_str(", \"target\": ");
            write_string(&mut output, &call.target);
            output.push_str(", \"site\": ");
            if let Some(site) = call.site {
                write_string(&mut output, &format!("{site:#010x}"));
            } else {
                output.push_str("null");
            }
            write!(
                output,
                ", \"tail\": {}, \"result_modeled\": {}, \"semantics\": ",
                call.tail, call.result_modeled
            )
            .expect("writing to String cannot fail");
            write_optional_string(&mut output, call.semantics.as_deref());
            output.push_str(", \"semantic_operation\": ");
            write_optional_string(&mut output, call.semantic_operation.as_deref());
            output.push_str(", \"semantic_contract\": ");
            write_semantic_contract(&mut output, call.semantic_contract.as_ref());
            output.push_str(", \"replacement_hint\": ");
            write_optional_string(&mut output, call.replacement_hint.as_deref());
            output.push_str(", \"trampoline\": ");
            if let Some(trampoline) = call.trampoline.as_ref() {
                write_trampoline(&mut output, trampoline);
            } else {
                output.push_str("null");
            }
            output.push_str(", \"project_symbol\": ");
            write_optional_string(&mut output, call.project_symbol.as_deref());
            output.push_str(", \"project_candidates\": ");
            write_strings(&mut output, &call.project_candidates);
            write!(output, ", \"argument_shapes\": {}", call.argument_shapes)
                .expect("writing to String cannot fail");
            output.push_str(", \"arguments\": ");
            write_strings(&mut output, &call.arguments);
            output.push_str(", \"argument_bindings\": [");
            for (binding_index, binding) in call.argument_bindings.iter().enumerate() {
                if binding_index != 0 {
                    output.push_str(", ");
                }
                write!(
                    output,
                    "{{\"position\": {}, \"caller_argument\": {}, \"offset\": {}, \"offset_hex\": ",
                    binding.position, binding.caller_argument, binding.offset
                )
                .expect("writing to String cannot fail");
                write_string(&mut output, &format!("{:+#x}", binding.offset));
                output.push_str(", \"expression\": ");
                write_string(&mut output, &binding.expression);
                output.push('}');
            }
            output.push_str("], \"cfg_guard_paths\": ");
            write_guard_paths(&mut output, call.guard_paths.as_deref());
            output.push_str(", \"typed_arguments\": [");
            for (argument_index, argument) in call.typed_arguments.iter().enumerate() {
                if argument_index != 0 {
                    output.push_str(", ");
                }
                write!(output, "{{\"position\": {}, \"name\": ", argument.position)
                    .expect("writing to String cannot fail");
                write_string(&mut output, &argument.name);
                output.push_str(", \"c_type\": ");
                write_string(&mut output, &argument.c_type);
                output.push_str(", \"direction\": ");
                write_string(&mut output, argument.direction);
                output.push_str(", \"value\": ");
                write_string(&mut output, &argument.value);
                output.push('}');
            }
            output.push(']');
            output.push('}');
        }
        output.push_str("], \"direct_mmio_predicates\": [");
        for (predicate_index, predicate) in function.direct_mmio_predicates.iter().enumerate() {
            if predicate_index != 0 {
                output.push_str(", ");
            }
            write!(
                output,
                "{{\"site\": \"{:#010x}\", \"condition\": ",
                predicate.site
            )
            .expect("writing to String cannot fail");
            write_string(&mut output, &predicate.condition);
            output.push_str(", \"operation\": ");
            write_string(&mut output, predicate.operation);
            output.push_str(", \"sources\": [");
            for (source_index, source) in predicate.sources.iter().enumerate() {
                if source_index != 0 {
                    output.push_str(", ");
                }
                write_direct_mmio_source(&mut output, source);
            }
            output.push_str("]}");
        }
        output.push_str("], \"mmio_accesses\": [");
        for (access_index, access) in function.mmio_accesses.iter().enumerate() {
            if access_index != 0 {
                output.push_str(", ");
            }
            write!(
                output,
                "{{\"ordinal\": {}, \"address\": \"{:#010x}\", \"width\": {}, \"register\": ",
                access.ordinal, access.address, access.width
            )
            .expect("writing to String cannot fail");
            write_string(&mut output, &access.register);
            output.push_str(", \"access\": ");
            write_string(&mut output, access.access);
            output.push_str(", \"mode\": ");
            write_string(&mut output, access.mode);
            output.push_str(", \"path\": ");
            write_string(&mut output, &access.path);
            output.push_str(", \"address_expression\": ");
            write_optional_string(&mut output, access.address_expression.as_deref());
            output.push_str(", \"guard\": ");
            write_optional_string(&mut output, access.guard.as_deref());
            output.push_str(", \"predicate_mask\": ");
            write_optional_hex(&mut output, access.predicate_mask);
            output.push_str(", \"predicate_expected\": ");
            write_optional_hex(&mut output, access.predicate_expected);
            output.push_str(", \"value\": ");
            write_optional_string(&mut output, access.value.as_deref());
            for (name, value) in [
                ("modified_mask", access.modified_mask),
                ("preserved_mask", access.preserved_mask),
                ("inverted_mask", access.inverted_mask),
                ("forced_zero_mask", access.forced_zero_mask),
                ("forced_one_mask", access.forced_one_mask),
                ("read_derived_mask", access.read_derived_mask),
                ("dynamic_mask", access.dynamic_mask),
            ] {
                write!(output, ", \"{name}\": ").expect("writing to String cannot fail");
                write_optional_hex(&mut output, value);
            }
            output.push('}');
        }
        output.push_str("], \"delays\": [");
        for (delay_index, delay) in function.delays.iter().enumerate() {
            if delay_index != 0 {
                output.push_str(", ");
            }
            write!(output, "{{\"ordinal\": {}, \"path\": ", delay.ordinal)
                .expect("writing to String cannot fail");
            write_string(&mut output, &delay.path);
            output.push_str(", \"micros\": ");
            write_string(&mut output, &delay.micros);
            output.push_str(", \"constant_micros\": ");
            if let Some(micros) = delay.constant_micros {
                write!(output, "{micros}").expect("writing to String cannot fail");
            } else {
                output.push_str("null");
            }
            output.push('}');
        }
        output.push_str("], \"context_fields\": [");
        for (field_index, field) in function.context_fields.iter().enumerate() {
            if field_index != 0 {
                output.push_str(", ");
            }
            write!(
                output,
                "{{\"argument\": {}, \"offset\": {}, \"offset_hex\": ",
                field.argument, field.offset
            )
            .expect("writing to String cannot fail");
            write_string(&mut output, &format!("{:+#x}", field.offset));
            write!(
                output,
                ", \"width\": {}, \"reads\": {}, \"writes\": {}, \"write_mask\": ",
                field.width, field.reads, field.writes
            )
            .expect("writing to String cannot fail");
            write_string(&mut output, &format!("{:#010x}", field.write_mask));
            output.push_str(", \"paths\": ");
            write_strings(&mut output, &field.paths);
            output.push_str(", \"write_values\": ");
            write_strings(&mut output, &field.write_values);
            output.push('}');
        }
        output.push_str("], \"context_accesses\": [");
        for (access_index, access) in function.context_accesses.iter().enumerate() {
            if access_index != 0 {
                output.push_str(", ");
            }
            write!(
                output,
                "{{\"argument\": {}, \"offset\": {}, \"offset_hex\": ",
                access.argument, access.offset
            )
            .expect("writing to String cannot fail");
            write_string(&mut output, &format!("{:+#x}", access.offset));
            write!(output, ", \"width\": {}, \"access\": ", access.width)
                .expect("writing to String cannot fail");
            write_string(&mut output, access.access);
            output.push_str(", \"path\": ");
            write_string(&mut output, &access.path);
            output.push_str(", \"value\": ");
            write_optional_string(&mut output, access.value.as_deref());
            output.push_str(", \"value_pseudo\": ");
            write_optional_string(&mut output, access.value_pseudo.as_deref());
            for (name, mask) in [
                ("write_mask", access.write_mask),
                ("preserved_mask", access.preserved_mask),
                ("forced_zero_mask", access.forced_zero_mask),
                ("forced_one_mask", access.forced_one_mask),
            ] {
                write!(output, ", \"{name}\": ").expect("writing to String cannot fail");
                if let Some(mask) = mask {
                    write_string(&mut output, &format!("{mask:#010x}"));
                } else {
                    output.push_str("null");
                }
            }
            output.push('}');
        }
        output.push_str("], \"effect_summary\": {");
        let summary = &function.effect_summary;
        write!(
            output,
            "\"call_graph_closed\": {}, \"max_depth\": {}, \"reachable_functions\": ",
            summary.call_graph_closed, summary.max_depth
        )
        .expect("writing to String cannot fail");
        write_strings(&mut output, &summary.reachable_functions);
        output.push_str(", \"recursive_functions\": ");
        write_strings(&mut output, &summary.recursive_functions);
        output.push_str(", \"blockers\": ");
        write_strings(&mut output, &summary.blockers);
        output.push_str(", \"mmio_registers\": [");
        for (mmio_index, mmio) in summary.mmio_registers.iter().enumerate() {
            if mmio_index != 0 {
                output.push_str(", ");
            }
            write!(
                output,
                "{{\"address\": \"{:#010x}\", \"width\": {}, \"access_shapes\": {}, \"accesses\": ",
                mmio.address, mmio.width, mmio.access_shapes
            )
            .expect("writing to String cannot fail");
            write_strings(&mut output, &mmio.accesses);
            output.push_str(", \"modes\": ");
            write_strings(&mut output, &mmio.modes);
            output.push_str(", \"origins\": ");
            write_strings(&mut output, &mmio.origins);
            output.push('}');
        }
        output.push_str("], \"delays\": [");
        for (delay_index, delay) in summary.delays.iter().enumerate() {
            if delay_index != 0 {
                output.push_str(", ");
            }
            output.push_str("{\"micros\": ");
            write_string(&mut output, &delay.micros);
            output.push_str(", \"constant_micros\": ");
            if let Some(micros) = delay.constant_micros {
                write!(output, "{micros}").expect("writing to String cannot fail");
            } else {
                output.push_str("null");
            }
            write!(
                output,
                ", \"delay_shapes\": {}, \"origins\": ",
                delay.delay_shapes
            )
            .expect("writing to String cannot fail");
            write_strings(&mut output, &delay.origins);
            output.push('}');
        }
        output.push_str("], \"semantic_operations\": [");
        for (semantic_index, semantic) in summary.semantic_operations.iter().enumerate() {
            if semantic_index != 0 {
                output.push_str(", ");
            }
            output.push_str("{\"operation\": ");
            write_string(&mut output, &semantic.operation);
            write!(
                output,
                ", \"call_shapes\": {}, \"targets\": ",
                semantic.call_shapes
            )
            .expect("writing to String cannot fail");
            write_strings(&mut output, &semantic.targets);
            output.push_str(", \"replacement_hints\": ");
            write_strings(&mut output, &semantic.replacement_hints);
            output.push_str(", \"origins\": ");
            write_strings(&mut output, &semantic.origins);
            output.push('}');
        }
        output.push_str("], \"semantic_actions\": [");
        for (action_index, action) in summary.semantic_actions.iter().enumerate() {
            if action_index != 0 {
                output.push_str(", ");
            }
            output.push_str("{\"operation\": ");
            write_string(&mut output, &action.operation);
            output.push_str(", \"target\": ");
            write_string(&mut output, &action.target);
            output.push_str(", \"semantic_contract\": ");
            write_semantic_contract(&mut output, action.contract.as_ref());
            output.push_str(", \"replacement_hint\": ");
            write_optional_string(&mut output, action.replacement_hint.as_deref());
            output.push_str(", \"origin\": ");
            write_string(&mut output, &action.origin);
            output.push_str(", \"path\": ");
            write_string(&mut output, &action.path);
            output.push_str(", \"site\": ");
            if let Some(site) = action.site {
                write_string(&mut output, &format!("{site:#010x}"));
            } else {
                output.push_str("null");
            }
            output.push_str(", \"site_path\": ");
            write_site_path(&mut output, &action.site_path);
            output.push_str(", \"cfg_guard_scopes\": ");
            write_guard_scopes(&mut output, action.guard_scopes.as_deref());
            write!(
                output,
                ", \"argument_shapes\": {}, \"arguments\": ",
                action.argument_shapes
            )
            .expect("writing to String cannot fail");
            write_projected_arguments(&mut output, &action.arguments);
            output.push('}');
        }
        output.push_str("], \"event_dispatches\": [");
        for (dispatch_index, dispatch) in summary.event_dispatches.iter().enumerate() {
            if dispatch_index != 0 {
                output.push_str(", ");
            }
            write!(
                output,
                "{{\"semantic_action_index\": {}, \"mechanism\": ",
                dispatch.semantic_action_index,
            )
            .expect("writing to String cannot fail");
            write_string(&mut output, dispatch.mechanism);
            output.push_str(", \"execution_context\": ");
            write_string(&mut output, dispatch.execution_context);
            output.push_str(", \"receiver\": ");
            write_optional_string(&mut output, dispatch.receiver.as_deref());
            write!(
                output,
                ", \"interface_complete\": {}, \"blockers\": ",
                dispatch.interface_complete,
            )
            .expect("writing to String cannot fail");
            write_strings(&mut output, &dispatch.blockers);
            output.push_str(", \"bindings\": [");
            for (binding_index, binding) in dispatch.bindings.iter().enumerate() {
                if binding_index != 0 {
                    output.push_str(", ");
                }
                output.push_str("{\"role\": ");
                write_string(&mut output, binding.role);
                output.push_str(", \"argument\": ");
                write_projected_argument(&mut output, &binding.argument);
                output.push('}');
            }
            output.push_str("]}");
        }
        write!(
            output,
            "], \"context_projection_complete\": {}, \"context_projection_blockers\": ",
            summary.context_projection_complete
        )
        .expect("writing to String cannot fail");
        write_strings(&mut output, &summary.context_projection_blockers);
        output.push_str(", \"context_fields\": [");
        for (field_index, field) in summary.context_fields.iter().enumerate() {
            if field_index != 0 {
                output.push_str(", ");
            }
            write!(
                output,
                "{{\"argument\": {}, \"offset\": {}, \"offset_hex\": ",
                field.argument, field.offset
            )
            .expect("writing to String cannot fail");
            write_string(&mut output, &format!("{:+#x}", field.offset));
            write!(
                output,
                ", \"width\": {}, \"reads\": {}, \"writes\": {}, \"write_mask\": \"{:#010x}\", \"origins\": ",
                field.width, field.reads, field.writes, field.write_mask
            )
            .expect("writing to String cannot fail");
            write_strings(&mut output, &field.origins);
            output.push_str(", \"paths\": ");
            write_strings(&mut output, &field.paths);
            output.push_str(", \"write_values\": ");
            write_strings(&mut output, &field.write_values);
            output.push('}');
        }
        output.push_str("], \"trampoline_calls\": [");
        for (call_index, call) in summary.trampoline_calls.iter().enumerate() {
            if call_index != 0 {
                output.push_str(", ");
            }
            output.push_str("{\"trampoline\": ");
            write_trampoline(&mut output, &call.trampoline);
            output.push_str(", \"origin\": ");
            write_string(&mut output, &call.origin);
            output.push_str(", \"path\": ");
            write_string(&mut output, &call.path);
            write!(output, ", \"argument_shapes\": {}", call.argument_shapes)
                .expect("writing to String cannot fail");
            output.push_str(", \"arguments\": ");
            write_projected_arguments(&mut output, &call.arguments);
            output.push('}');
        }
        output.push_str("]}, \"direct_blockers\": ");
        write_strings(&mut output, &function.direct_blockers);
        output.push_str(", \"reference_blockers\": ");
        write_strings(&mut output, &function.reference_blockers);
        output.push_str(", \"call_graph_blockers\": ");
        write_strings(&mut output, &function.call_graph_blockers);
        output.push_str(
            ", \"diagnostics\": {\"mode\": \"exact-semicolon-fragment-inventory\", \"direct\": ",
        );
        write_diagnostics(&mut output, &function.direct_diagnostics);
        output.push_str(", \"reference\": ");
        write_diagnostics(&mut output, &function.reference_diagnostics);
        output.push_str(", \"call_graph\": ");
        write_diagnostics(&mut output, &function.call_graph_diagnostics);
        output.push('}');
        output.push_str(", \"pseudo\": ");
        write_string(&mut output, &function.pseudo);
        output.push('}');
        output.push_str(if index + 1 == report.functions.len() {
            "\n"
        } else {
            ",\n"
        });
    }
    output.push_str("  ]\n}\n");
    Ok(output)
}

pub(super) fn write_json_report(
    path: &Path,
    artifacts: &[IrArtifactInput],
    companions: &[PathBuf],
    symbol_prefix: &str,
    entry_contract: EntryContractRef,
    report: &LinkedIrReport,
    include_reachable: bool,
) -> Result<()> {
    fs::write(
        path,
        render_json_report(
            artifacts,
            companions,
            symbol_prefix,
            entry_contract,
            report,
            include_reachable,
        )?,
    )?;
    println!("JSON-REPORT\t{}", path.display());
    Ok(())
}
