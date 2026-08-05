//! JSON encoders for linked-IR value objects.

use super::*;

pub(super) fn write_optional_string(output: &mut String, value: Option<&str>) {
    if let Some(value) = value {
        write_string(output, value);
    } else {
        output.push_str("null");
    }
}

pub(super) fn write_optional_hex(output: &mut String, value: Option<u32>) {
    if let Some(value) = value {
        write_string(output, &format!("{value:#010x}"));
    } else {
        output.push_str("null");
    }
}

pub(super) fn write_site_path(output: &mut String, site_path: &[Option<u32>]) {
    output.push('[');
    for (site_index, site) in site_path.iter().enumerate() {
        if site_index != 0 {
            output.push_str(", ");
        }
        if let Some(site) = site {
            write_string(output, &format!("{site:#010x}"));
        } else {
            output.push_str("null");
        }
    }
    output.push(']');
}

pub(super) fn write_return_provenance(output: &mut String, provenance: &LinkedReturnProvenance) {
    write!(
        output,
        "{{\"exact\": {}, \"known_zero_bits\": \"{:#010x}\", \"known_one_bits\": \"{:#010x}\", \"unknown_bits\": \"{:#010x}\", \"sources\": [",
        provenance.exact,
        provenance.known_zero_bits,
        provenance.known_one_bits,
        provenance.unknown_bits,
    )
    .expect("writing to String cannot fail");
    for (index, source) in provenance.sources.iter().enumerate() {
        if index != 0 {
            output.push_str(", ");
        }
        output.push_str("{\"kind\": ");
        write_string(output, source.kind);
        write!(
            output,
            ", \"output_lsb\": {}, \"source_lsb\": {}, \"width\": {}, \"output_bits\": \"{:#010x}\", \"source_bits\": \"{:#010x}\", \"inverted\": {}, \"argument\": ",
            source.output_lsb,
            source.source_lsb,
            source.width,
            source.output_bits,
            source.source_bits,
            source.inverted,
        )
        .expect("writing to String cannot fail");
        if let Some(argument) = source.argument {
            write!(output, "{argument}").expect("writing to String cannot fail");
        } else {
            output.push_str("null");
        }
        output.push_str(", \"token\": ");
        if let Some(token) = source.token {
            write!(output, "{token}").expect("writing to String cannot fail");
        } else {
            output.push_str("null");
        }
        output.push_str(", \"target\": ");
        write_optional_string(output, source.target.as_deref());
        output.push_str(", \"address\": ");
        write_optional_hex(output, source.address);
        output.push_str(", \"register\": ");
        write_optional_string(output, source.register.as_deref());
        output.push('}');
    }
    output.push_str("]}");
}

pub(super) fn write_direct_mmio_source(
    output: &mut String,
    source: &LinkedDirectMmioPredicateSource,
) {
    output.push_str("{\"operand\": ");
    write_string(output, source.operand);
    write!(
        output,
        ", \"read_token\": {}, \"address\": \"{:#010x}\", \"register\": ",
        source.read_token, source.address
    )
    .expect("writing to String cannot fail");
    write_string(output, &source.register);
    write!(
        output,
        ", \"value_bits\": \"{:#010x}\", \"register_bits\": \"{:#010x}\", \"inverted\": {}, \"comparison_value\": ",
        source.value_bits, source.register_bits, source.inverted,
    )
    .expect("writing to String cannot fail");
    write_optional_hex(output, source.comparison_value);
    output.push_str(", \"register_comparison_value\": ");
    write_optional_hex(output, source.register_comparison_value);
    output.push('}');
}

pub(super) fn write_guard_paths(output: &mut String, paths: Option<&[LinkedCallGuardPath]>) {
    let Some(paths) = paths else {
        output.push_str("null");
        return;
    };
    output.push('[');
    for (path_index, path) in paths.iter().enumerate() {
        if path_index != 0 {
            output.push_str(", ");
        }
        output.push_str("{\"expression\": ");
        write_string(output, &format_guard_path(path));
        output.push_str(", \"guards\": [");
        for (guard_index, guard) in path.guards.iter().enumerate() {
            if guard_index != 0 {
                output.push_str(", ");
            }
            write!(
                output,
                "{{\"site\": \"{:#010x}\", \"condition\": ",
                guard.site
            )
            .expect("writing to String cannot fail");
            write_string(output, &guard.condition);
            output.push_str(", \"operation\": ");
            write_string(output, guard.operation);
            write!(
                output,
                ", \"taken\": {}, \"effective_operation\": ",
                guard.taken,
            )
            .expect("writing to String cannot fail");
            write_string(
                output,
                effective_branch_operation(guard.operation, guard.taken),
            );
            output.push_str(", \"result_sources\": [");
            for (source_index, source) in guard.result_sources.iter().enumerate() {
                if source_index != 0 {
                    output.push_str(", ");
                }
                output.push_str("{\"kind\": ");
                write_string(output, source.kind);
                write!(output, ", \"token\": {}, \"target\": ", source.token)
                    .expect("writing to String cannot fail");
                write_optional_string(output, source.target.as_deref());
                output.push_str(", \"operand\": ");
                write_string(output, source.operand);
                output.push_str(", \"value_bits\": ");
                write_optional_hex(output, source.value_bits);
                write!(
                    output,
                    ", \"source_bits\": \"{:#010x}\", \"inverted\": {}, \"comparison_value\": ",
                    source.source_bits, source.inverted,
                )
                .expect("writing to String cannot fail");
                write_optional_hex(output, source.comparison_value);
                output.push_str(", \"source_comparison_value\": ");
                write_optional_hex(output, source.source_comparison_value);
                output.push_str(", \"producer_return_exact\": ");
                if let Some(exact) = source.producer_return_exact {
                    write!(output, "{exact}").expect("writing to String cannot fail");
                } else {
                    output.push_str("null");
                }
                output.push_str(", \"mmio_sources\": [");
                for (mmio_index, mmio) in source.mmio_sources.iter().enumerate() {
                    if mmio_index != 0 {
                        output.push_str(", ");
                    }
                    write!(
                        output,
                        "{{\"address\": \"{:#010x}\", \"register\": ",
                        mmio.address
                    )
                    .expect("writing to String cannot fail");
                    write_string(output, &mmio.register);
                    output.push_str(", \"producer_path\": ");
                    write_strings(output, &mmio.producer_path);
                    write!(
                        output,
                        ", \"return_depth\": {}, \"result_bits\": \"{:#010x}\", \"register_bits\": \"{:#010x}\", \"inverted\": {}, \"result_comparison_value\": ",
                        mmio.producer_path.len().saturating_sub(1),
                        mmio.result_bits,
                        mmio.register_bits,
                        mmio.inverted,
                    )
                    .expect("writing to String cannot fail");
                    write_optional_hex(output, mmio.result_comparison_value);
                    output.push_str(", \"register_comparison_value\": ");
                    write_optional_hex(output, mmio.register_comparison_value);
                    output.push('}');
                }
                output.push_str("]}");
            }
            output.push_str("], \"direct_mmio_sources\": [");
            for (source_index, source) in guard.direct_mmio_sources.iter().enumerate() {
                if source_index != 0 {
                    output.push_str(", ");
                }
                write_direct_mmio_source(output, source);
            }
            output.push_str("]}");
        }
        output.push_str("]}");
    }
    output.push(']');
}

pub(super) fn write_guard_scopes(output: &mut String, scopes: Option<&[LinkedCallGuardScope]>) {
    let Some(scopes) = scopes else {
        output.push_str("null");
        return;
    };
    output.push('[');
    for (scope_index, scope) in scopes.iter().enumerate() {
        if scope_index != 0 {
            output.push_str(", ");
        }
        output.push_str("{\"function\": ");
        write_string(output, &scope.function);
        output.push_str(", \"expression\": ");
        write_string(output, &format_guard_paths(&scope.paths));
        output.push_str(", \"paths\": ");
        write_guard_paths(output, Some(&scope.paths));
        output.push('}');
    }
    output.push(']');
}

pub(super) fn write_trampoline(output: &mut String, trampoline: &LinkedTrampoline) {
    output.push_str("{\"table\": ");
    write_string(output, &trampoline.table);
    output.push_str(", \"pointer_symbol\": ");
    write_string(output, &trampoline.pointer_symbol);
    output.push_str(", \"backing_symbol\": ");
    write_string(output, &trampoline.backing_symbol);
    write!(
        output,
        ", \"version\": {}, \"magic\": \"{:#010x}\", \"table_size\": {}, \"table_size_hex\": \"{:#x}\", \"magic_offset\": {}, \"magic_offset_hex\": \"{:#x}\", \"function_id\": ",
        trampoline.version,
        trampoline.magic,
        trampoline.table_size,
        trampoline.table_size,
        trampoline.magic_offset,
        trampoline.magic_offset,
    )
    .expect("writing to String cannot fail");
    write_string(output, &trampoline.function_id);
    write!(
        output,
        ", \"slot\": {}, \"slot_hex\": \"{:#x}\", \"c_name\": ",
        trampoline.slot, trampoline.slot
    )
    .expect("writing to String cannot fail");
    write_string(output, &trampoline.c_name);
    write!(
        output,
        ", \"argument_count\": {}, \"return_model\": ",
        trampoline.argument_count
    )
    .expect("writing to String cannot fail");
    write_string(output, &trampoline.return_model);
    output.push_str(", \"operation\": ");
    write_string(output, &trampoline.operation);
    output.push_str(", \"return_type\": ");
    write_string(output, &trampoline.return_type);
    output.push_str(", \"replacement_hint\": ");
    write_optional_string(output, trampoline.replacement_hint.as_deref());
    output.push('}');
}

pub(super) fn write_semantic_contract(
    output: &mut String,
    contract: Option<&LinkedSemanticContract>,
) {
    let Some(contract) = contract else {
        output.push_str("null");
        return;
    };
    output.push_str("{\"source\": ");
    write_string(output, contract.source);
    output.push_str(", \"id\": ");
    write_string(output, &contract.id);
    output.push_str(", \"evidence\": ");
    write_string(output, &contract.evidence);
    output.push('}');
}

pub(super) fn write_projected_argument(
    output: &mut String,
    argument: &LinkedProjectedCallArgument,
) {
    write!(output, "{{\"position\": {}, \"name\": ", argument.position)
        .expect("writing to String cannot fail");
    write_string(output, &argument.name);
    output.push_str(", \"c_type\": ");
    write_string(output, &argument.c_type);
    output.push_str(", \"direction\": ");
    write_string(output, argument.direction);
    output.push_str(", \"value\": ");
    write_string(output, &argument.value);
    output.push_str(", \"binding\": ");
    write_string(output, argument.binding);
    output.push_str(", \"root_argument\": ");
    if let Some(root_argument) = argument.root_argument {
        write!(output, "{root_argument}").expect("writing to String cannot fail");
    } else {
        output.push_str("null");
    }
    output.push_str(", \"root_offset\": ");
    if let Some(root_offset) = argument.root_offset {
        write!(output, "{root_offset}").expect("writing to String cannot fail");
    } else {
        output.push_str("null");
    }
    output.push_str(", \"root_offset_hex\": ");
    if let Some(root_offset) = argument.root_offset {
        write_string(output, &format!("{root_offset:+#x}"));
    } else {
        output.push_str("null");
    }
    output.push('}');
}

pub(super) fn write_projected_arguments(
    output: &mut String,
    arguments: &[LinkedProjectedCallArgument],
) {
    output.push('[');
    for (argument_index, argument) in arguments.iter().enumerate() {
        if argument_index != 0 {
            output.push_str(", ");
        }
        write_projected_argument(output, argument);
    }
    output.push(']');
}

pub(super) fn write_diagnostics(output: &mut String, diagnostics: &[LinkedDiagnostic]) {
    output.push('[');
    for (diagnostic_index, diagnostic) in diagnostics.iter().enumerate() {
        if diagnostic_index != 0 {
            output.push_str(", ");
        }
        output.push_str("{\"rendered\": ");
        write_string(output, &diagnostic.rendered);
        write!(
            output,
            ", \"original_fragments\": {}, \"unique_fragments\": {}, \"fragments\": [",
            diagnostic.original_fragments,
            diagnostic.fragments.len(),
        )
        .expect("writing to String cannot fail");
        for (fragment_index, fragment) in diagnostic.fragments.iter().enumerate() {
            if fragment_index != 0 {
                output.push_str(", ");
            }
            write!(
                output,
                "{{\"first_ordinal\": {}, \"occurrences\": {}, \"message\": ",
                fragment.first_ordinal, fragment.occurrences,
            )
            .expect("writing to String cannot fail");
            write_string(output, &fragment.message);
            output.push('}');
        }
        output.push_str("]}");
    }
    output.push(']');
}
