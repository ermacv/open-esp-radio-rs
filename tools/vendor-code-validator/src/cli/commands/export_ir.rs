//! Linked best-effort function/call IR export.

use std::{fmt::Write as _, path::Path};

use super::super::json::{write_artifact, write_string, write_strings};
use super::super::*;

fn print_report(report: &LinkedIrReport) {
    for function in &report.functions {
        let address = function.address.map_or_else(
            || "relocatable".to_owned(),
            |address| format!("{address:#010x}"),
        );
        println!(
            "FUNCTION\t{}\tbinding={}\taddress={}\tobject-offset={:#010x}\tsize={}\tflow={}\tcomplete={}\texact={}\tcalls={}",
            function.identity,
            function.binding,
            address,
            function.object_offset,
            function.size,
            function.flow_kind,
            function.complete,
            function.exact,
            function.calls.len(),
        );
        for call in &function.calls {
            let site = call
                .site
                .map_or_else(|| "-".to_owned(), |site| format!("{site:#010x}"));
            println!(
                "CALL\t{}\t{}\t{}\tsite={}\ttail={}\tresult-modeled={}\toperation={}\treplacement={}\t{}",
                function.identity,
                call.kind,
                call.target,
                site,
                call.tail,
                call.result_modeled,
                call.semantic_operation.as_deref().unwrap_or("-"),
                call.replacement_hint.as_deref().unwrap_or("-"),
                call.semantics.as_deref().unwrap_or("-"),
            );
            for argument in &call.typed_arguments {
                println!(
                    "CALL-ARG\t{}\t{}\tposition={}\tname={}\ttype={}\tdirection={}\tvalue={}",
                    function.identity,
                    call.target,
                    argument.position,
                    argument.name,
                    argument.c_type,
                    argument.direction,
                    argument.value,
                );
            }
        }
        for field in &function.context_fields {
            println!(
                "CONTEXT-FIELD\t{}\targ={}\toffset={:+#x}\twidth={}\treads={}\twrites={}\twrite-mask={:#010x}\tpaths={}\tvalues={}",
                function.identity,
                field.argument,
                field.offset,
                field.width,
                field.reads,
                field.writes,
                field.write_mask,
                field.paths.len(),
                field.write_values.join(" | "),
            );
        }
        for access in &function.context_accesses {
            let mask = |value: Option<u32>| {
                value.map_or_else(|| "-".to_owned(), |value| format!("{value:#010x}"))
            };
            println!(
                "CONTEXT\t{}\targ={}\toffset={:+#x}\twidth={}\taccess={}\twrite-mask={}\tpreserved-mask={}\tforced-zero={}\tforced-one={}\tpath={}\tvalue={}",
                function.identity,
                access.argument,
                access.offset,
                access.width,
                access.access,
                mask(access.write_mask),
                mask(access.preserved_mask),
                mask(access.forced_zero_mask),
                mask(access.forced_one_mask),
                access.path,
                access.value_pseudo.as_deref().unwrap_or("-"),
            );
        }
        for blocker in &function.direct_blockers {
            println!("IR-DIAGNOSTIC\t{}\tdirect\t{blocker}", function.identity);
        }
        for blocker in &function.reference_blockers {
            println!("IR-DIAGNOSTIC\t{}\treference\t{blocker}", function.identity);
        }
        for blocker in &function.call_graph_blockers {
            println!(
                "IR-DIAGNOSTIC\t{}\tcall-graph\t{blocker}",
                function.identity
            );
        }
    }
    for boundary in &report.semantic_boundaries {
        println!(
            "SEMANTIC\t{}\tcall-shapes={}\tfunctions={}\ttargets={}\treplacements={}",
            boundary.operation,
            boundary.call_shapes,
            boundary.functions.join(","),
            boundary.targets.join(","),
            boundary.replacement_hints.join(" | "),
        );
    }
    println!(
        "SUMMARY\tfunctions={}\texported={}\tlocal={}\tcontext-functions={}\tcontext-fields={}\tcontext-accesses={}\tsemantic-operations={}\tsemantic-calls={}\tcomplete={}\tstructured={}\tinternal-calls={}\texternal-calls={}\tunresolved-calls={}",
        report.functions.len(),
        report.exported_functions,
        report.local_functions,
        report.context_functions,
        report.context_fields,
        report.context_accesses,
        report.semantic_boundaries.len(),
        report.semantic_calls,
        report.complete_functions,
        report.structured_functions,
        report.internal_calls,
        report.external_calls,
        report.unresolved_calls,
    );
}

fn write_pseudo(path: &Path, artifact: &Path, report: &LinkedIrReport) -> Result<()> {
    let mut output = String::new();
    writeln!(
        output,
        "// Best-effort vendor-code pseudo-Rust generated from {}.",
        artifact.display()
    )
    .expect("writing to String cannot fail");
    output
        .push_str("// This is analysis IR, not compilable Rust and not a completeness claim.\n\n");
    for function in &report.functions {
        output.push_str(&function.pseudo);
        output.push('\n');
    }
    fs::write(path, output)?;
    println!("PSEUDO-IR\t{}", path.display());
    Ok(())
}

fn write_optional_string(output: &mut String, value: Option<&str>) {
    if let Some(value) = value {
        write_string(output, value);
    } else {
        output.push_str("null");
    }
}

fn write_json_report(
    path: &Path,
    artifact: &Path,
    companions: &[PathBuf],
    symbol_prefix: &str,
    entry_contract: EntryContractRef,
    report: &LinkedIrReport,
) -> Result<()> {
    let mut output = String::new();
    output.push_str("{\n  \"schema_version\": 7,\n  \"command\": \"ir-export\",\n");
    output.push_str("  \"analysis_mode\": \"best-effort\",\n");
    output.push_str("  \"completeness_claim\": false,\n  \"artifact\": ");
    write_artifact(&mut output, artifact)?;
    output.push_str(",\n  \"companions\": [");
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
    writeln!(
        output,
        ",\n  \"summary\": {{\"functions\": {}, \"exported\": {}, \"local\": {}, \"context_functions\": {}, \"context_fields\": {}, \"context_accesses\": {}, \"semantic_operations\": {}, \"semantic_calls\": {}, \"complete\": {}, \"structured\": {}, \"internal_calls\": {}, \"external_calls\": {}, \"unresolved_calls\": {}}},",
        report.functions.len(),
        report.exported_functions,
        report.local_functions,
        report.context_functions,
        report.context_fields,
        report.context_accesses,
        report.semantic_boundaries.len(),
        report.semantic_calls,
        report.complete_functions,
        report.structured_functions,
        report.internal_calls,
        report.external_calls,
        report.unresolved_calls,
    )
    .expect("writing to String cannot fail");
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
    output.push_str("  \"functions\": [\n");
    for (index, function) in report.functions.iter().enumerate() {
        output.push_str("    {\"identity\": ");
        write_string(&mut output, &function.identity);
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
            output.push_str(", \"replacement_hint\": ");
            write_optional_string(&mut output, call.replacement_hint.as_deref());
            output.push_str(", \"arguments\": ");
            write_strings(&mut output, &call.arguments);
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
        output.push_str("], \"direct_blockers\": ");
        write_strings(&mut output, &function.direct_blockers);
        output.push_str(", \"reference_blockers\": ");
        write_strings(&mut output, &function.reference_blockers);
        output.push_str(", \"call_graph_blockers\": ");
        write_strings(&mut output, &function.call_graph_blockers);
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
    fs::write(path, output)?;
    println!("JSON-REPORT\t{}", path.display());
    Ok(())
}

pub(super) fn run(
    filtered: Vec<String>,
    svd: &MmioRegisterMap,
    target: &TargetSpec,
) -> Result<bool> {
    let mut artifact = None;
    let mut companions = Vec::new();
    let mut symbol_prefix = String::new();
    let mut pseudo_path = None;
    let mut json_report = None;
    let riscv_harness = harnesses::riscv(&target.harness)?;
    let mut entry_contract = harnesses::entry_contract(&target.harness, "none")?;
    let mut arguments = filtered.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--artifact" => {
                artifact = Some(PathBuf::from(take_value(&mut arguments, "--artifact")?));
            }
            "--companion" => {
                companions.push(PathBuf::from(take_value(&mut arguments, "--companion")?));
            }
            "--symbol-prefix" => {
                symbol_prefix = take_value(&mut arguments, "--symbol-prefix")?;
            }
            "--entry-contract" => {
                entry_contract = harnesses::entry_contract(
                    &target.harness,
                    &take_value(&mut arguments, "--entry-contract")?,
                )?;
            }
            "--pseudo-rust" => {
                pseudo_path = Some(PathBuf::from(take_value(&mut arguments, "--pseudo-rust")?));
            }
            "--json-report" => {
                json_report = Some(PathBuf::from(take_value(&mut arguments, "--json-report")?));
            }
            _ => return Err(format!("unknown ir export option: {argument}").into()),
        }
    }
    let artifact = artifact.ok_or("ir export requires --artifact PATH")?;
    let resolver = ReferenceResolver::load_all_code_with_entry_contract(
        &artifact,
        &companions,
        riscv_harness,
        entry_contract,
    )?;
    let report = build_linked_ir(&resolver, &symbol_prefix, svd);
    if report.functions.is_empty() {
        return Err(format!(
            "no named code symbols start with {symbol_prefix:?} in {}",
            artifact.display()
        )
        .into());
    }

    print_report(&report);
    if let Some(path) = pseudo_path.as_deref() {
        write_pseudo(path, &artifact, &report)?;
    }
    if let Some(path) = json_report.as_deref() {
        write_json_report(
            path,
            &artifact,
            &companions,
            &symbol_prefix,
            entry_contract,
            &report,
        )?;
    }
    Ok(true)
}
