//! Linked best-effort function/call IR export.

use std::{fmt::Write as _, path::Path};

use super::super::json::{write_artifact, write_string, write_strings};
use super::super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
struct IrArtifactInput {
    source: String,
    path: PathBuf,
    explicitly_named: bool,
}

fn valid_source_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn named_artifact(source: &str, path: &str) -> Result<IrArtifactInput> {
    if !valid_source_id(source) {
        return Err(format!("invalid artifact source id {source:?}").into());
    }
    if path.is_empty() {
        return Err("artifact path must not be empty".into());
    }
    Ok(IrArtifactInput {
        source: source.to_owned(),
        path: PathBuf::from(path),
        explicitly_named: true,
    })
}

fn parse_artifact(value: &str) -> Result<IrArtifactInput> {
    if let Some((source, path)) = value
        .split_once('=')
        .filter(|(source, path)| valid_source_id(source) && !path.is_empty())
    {
        return named_artifact(source, path);
    }
    if value.is_empty() {
        return Err("--artifact requires a path or SOURCE=PATH".into());
    }
    Ok(IrArtifactInput {
        source: "primary".to_owned(),
        path: PathBuf::from(value),
        explicitly_named: false,
    })
}

fn source_artifact_option(argument: &str) -> Option<&str> {
    argument
        .strip_prefix("--source-artifact:")
        .filter(|source| !source.is_empty())
}

fn validate_artifact_inputs(artifacts: &[IrArtifactInput], companions: &[PathBuf]) -> Result<bool> {
    if artifacts.is_empty() {
        return Err("ir export requires at least one --artifact PATH or SOURCE=PATH".into());
    }
    if artifacts.len() > 1 && artifacts.iter().any(|artifact| !artifact.explicitly_named) {
        return Err("multiple IR artifacts must use unique SOURCE=PATH names".into());
    }
    if artifacts.len() > 1 && !companions.is_empty() {
        return Err("--companion is only supported with one primary IR artifact".into());
    }
    let mut sources = BTreeSet::new();
    for artifact in artifacts {
        if !sources.insert(artifact.source.clone()) {
            return Err(format!("duplicate artifact source {:?}", artifact.source).into());
        }
    }
    Ok(artifacts.len() > 1 || artifacts[0].explicitly_named)
}

fn print_report(artifacts: &[IrArtifactInput], report: &LinkedIrReport, include_reachable: bool) {
    println!(
        "PROJECT\tlinkage={}\tcall-linkage={}\tselection={}\tcall-compaction=stable-identity-universal-affine-bindings\tdiagnostic-compaction=exact-semicolon-fragment-inventory\tcontext-projection=affine-simple-call-paths\tsemantic-actions=lexical-site-paths-affine-root-bindings\tartifacts={}",
        if artifacts.len() > 1 {
            "independent-artifacts"
        } else {
            "primary-with-companions"
        },
        if artifacts.len() > 1 {
            "unique-exported-symbol-only"
        } else {
            "primary-resolver"
        },
        if include_reachable {
            "symbol-prefix-with-reachable-internal-callees"
        } else {
            "symbol-prefix-only"
        },
        artifacts.len()
    );
    for artifact in artifacts {
        let functions = report
            .functions
            .iter()
            .filter(|function| function.source == artifact.source)
            .count();
        println!(
            "ARTIFACT\t{}\t{}\tfunctions={}",
            artifact.source,
            artifact.path.display(),
            functions
        );
    }
    for function in &report.functions {
        let address = function.address.map_or_else(
            || "relocatable".to_owned(),
            |address| format!("{address:#010x}"),
        );
        println!(
            "FUNCTION\t{}\tselection={}\tbinding={}\taddress={}\tobject-offset={:#010x}\tsize={}\tflow={}\tcomplete={}\texact={}\tcalls={}\tcall-argument-shapes={}",
            function.identity,
            function.selection,
            function.binding,
            address,
            function.object_offset,
            function.size,
            function.flow_kind,
            function.complete,
            function.exact,
            function.calls.len(),
            function
                .calls
                .iter()
                .map(|call| call.argument_shapes)
                .sum::<usize>(),
        );
        for call in &function.calls {
            let site = call
                .site
                .map_or_else(|| "-".to_owned(), |site| format!("{site:#010x}"));
            let trampoline = call.trampoline.as_ref().map_or_else(
                || "-".to_owned(),
                |trampoline| {
                    format!(
                        "{}+{:#x}/{}",
                        trampoline.table, trampoline.slot, trampoline.c_name
                    )
                },
            );
            let semantic_source = call
                .semantic_contract
                .as_ref()
                .map_or("-", |contract| contract.source);
            let semantic_contract = call
                .semantic_contract
                .as_ref()
                .map_or("-", |contract| contract.id.as_str());
            let semantic_evidence = call
                .semantic_contract
                .as_ref()
                .map_or("-", |contract| contract.evidence.as_str());
            println!(
                "CALL\t{}\t{}\t{}\tsite={}\ttail={}\tresult-modeled={}\toperation={}\tsemantic-source={}\tsemantic-contract={}\tsemantic-evidence={}\treplacement={}\ttrampoline={}\tproject-symbol={}\tproject-candidates={}\targument-shapes={}\taffine-bindings={}\t{}",
                function.identity,
                call.kind,
                call.target,
                site,
                call.tail,
                call.result_modeled,
                call.semantic_operation.as_deref().unwrap_or("-"),
                semantic_source,
                semantic_contract,
                semantic_evidence,
                call.replacement_hint.as_deref().unwrap_or("-"),
                trampoline,
                call.project_symbol.as_deref().unwrap_or("-"),
                call.project_candidates.join("|"),
                call.argument_shapes,
                call.argument_bindings.len(),
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
            for binding in call.argument_bindings.iter().filter(|binding| {
                binding.offset != 0 || binding.position != usize::from(binding.caller_argument)
            }) {
                println!(
                    "CALL-BINDING\t{}\t{}\tcallee-arg={}\tcaller-arg={}\toffset={:+#x}\texpression={}",
                    function.identity,
                    call.target,
                    binding.position,
                    binding.caller_argument,
                    binding.offset,
                    binding.expression,
                );
            }
        }
        for access in &function.mmio_accesses {
            let mask = |value: Option<u32>| {
                value.map_or_else(|| "-".to_owned(), |value| format!("{value:#010x}"))
            };
            println!(
                "MMIO\t{}\tordinal={}\t{:#010x}\twidth={}\tregister={}\taccess={}\tmode={}\tpath={}\taddress-expression={}\tguard={}\tvalue={}\tmodified={}\tpreserved={}\tinverted={}\tforced-zero={}\tforced-one={}\tread-derived={}\tdynamic={}",
                function.identity,
                access.ordinal,
                access.address,
                access.width,
                access.register,
                access.access,
                access.mode,
                access.path,
                access.address_expression.as_deref().unwrap_or("-"),
                access.guard.as_deref().unwrap_or("-"),
                access.value.as_deref().unwrap_or("-"),
                mask(access.modified_mask),
                mask(access.preserved_mask),
                mask(access.inverted_mask),
                mask(access.forced_zero_mask),
                mask(access.forced_one_mask),
                mask(access.read_derived_mask),
                mask(access.dynamic_mask),
            );
        }
        for delay in &function.delays {
            println!(
                "DELAY\t{}\tordinal={}\tmicros={}\tconstant-micros={}\tpath={}",
                function.identity,
                delay.ordinal,
                delay.micros,
                delay
                    .constant_micros
                    .map_or_else(|| "-".to_owned(), |value| value.to_string()),
                delay.path,
            );
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
        let summary = &function.effect_summary;
        println!(
            "EFFECT-SUMMARY\t{}\tcall-graph-closed={}\tmax-depth={}\treachable-functions={}\trecursive-functions={}\tmmio-registers={}\tdelays={}\tsemantic-operations={}\tsemantic-actions={}\ttrampoline-calls={}\tcontext-projection-complete={}\tcontext-fields={}\tblockers={}\tcontext-blockers={}",
            function.identity,
            summary.call_graph_closed,
            summary.max_depth,
            summary.reachable_functions.join(","),
            summary.recursive_functions.join(","),
            summary.mmio_registers.len(),
            summary.delays.len(),
            summary.semantic_operations.len(),
            summary.semantic_actions.len(),
            summary.trampoline_calls.len(),
            summary.context_projection_complete,
            summary.context_fields.len(),
            summary.blockers.len(),
            summary.context_projection_blockers.len(),
        );
        for mmio in &summary.mmio_registers {
            println!(
                "EFFECT-MMIO\t{}\t{:#010x}\twidth={}\taccess-shapes={}\taccesses={}\tmodes={}\torigins={}",
                function.identity,
                mmio.address,
                mmio.width,
                mmio.access_shapes,
                mmio.accesses.join("|"),
                mmio.modes.join("|"),
                mmio.origins.join(","),
            );
        }
        for delay in &summary.delays {
            println!(
                "EFFECT-DELAY\t{}\tmicros={}\tconstant-micros={}\tdelay-shapes={}\torigins={}",
                function.identity,
                delay.micros,
                delay
                    .constant_micros
                    .map_or_else(|| "-".to_owned(), |value| value.to_string()),
                delay.delay_shapes,
                delay.origins.join(","),
            );
        }
        for semantic in &summary.semantic_operations {
            println!(
                "EFFECT-SEMANTIC\t{}\t{}\tcall-shapes={}\ttargets={}\treplacements={}\torigins={}",
                function.identity,
                semantic.operation,
                semantic.call_shapes,
                semantic.targets.join("|"),
                semantic.replacement_hints.join(" | "),
                semantic.origins.join(","),
            );
        }
        for action in &summary.semantic_actions {
            let site = action
                .site
                .map_or_else(|| "-".to_owned(), |site| format!("{site:#010x}"));
            let semantic_source = action
                .contract
                .as_ref()
                .map_or("-", |contract| contract.source);
            let semantic_contract = action
                .contract
                .as_ref()
                .map_or("-", |contract| contract.id.as_str());
            let semantic_evidence = action
                .contract
                .as_ref()
                .map_or("-", |contract| contract.evidence.as_str());
            println!(
                "EFFECT-ACTION\t{}\t{}\ttarget={}\tsite={}\tsite-path={}\targument-shapes={}\torigin={}\tpath={}\tsemantic-source={}\tsemantic-contract={}\tsemantic-evidence={}\treplacement={}",
                function.identity,
                action.operation,
                action.target,
                site,
                format_site_path(&action.site_path),
                action.argument_shapes,
                action.origin,
                action.path,
                semantic_source,
                semantic_contract,
                semantic_evidence,
                action.replacement_hint.as_deref().unwrap_or("-"),
            );
            for argument in &action.arguments {
                println!(
                    "EFFECT-ACTION-ARG\t{}\t{}\tposition={}\tname={}\ttype={}\tdirection={}\tvalue={}\tbinding={}\troot-arg={}\troot-offset={}",
                    function.identity,
                    action.operation,
                    argument.position,
                    argument.name,
                    argument.c_type,
                    argument.direction,
                    argument.value,
                    argument.binding,
                    argument
                        .root_argument
                        .map_or_else(|| "-".to_owned(), |value| value.to_string()),
                    argument
                        .root_offset
                        .map_or_else(|| "-".to_owned(), |value| format!("{value:+#x}")),
                );
            }
        }
        for field in &summary.context_fields {
            println!(
                "EFFECT-CONTEXT-FIELD\t{}\targ={}\toffset={:+#x}\twidth={}\treads={}\twrites={}\twrite-mask={:#010x}\torigins={}\tpaths={}\tvalues={}",
                function.identity,
                field.argument,
                field.offset,
                field.width,
                field.reads,
                field.writes,
                field.write_mask,
                field.origins.join(","),
                field.paths.join(" | "),
                field.write_values.join(" | "),
            );
        }
        for call in &summary.trampoline_calls {
            println!(
                "EFFECT-TRAMPOLINE\t{}\t{}+{:#x}\tfunction={}\toperation={}\treturn-model={}\treturn-type={}\targument-shapes={}\torigin={}\tpath={}\treplacement={}",
                function.identity,
                call.trampoline.table,
                call.trampoline.slot,
                call.trampoline.c_name,
                call.trampoline.operation,
                call.trampoline.return_model,
                call.trampoline.return_type,
                call.argument_shapes,
                call.origin,
                call.path,
                call.trampoline.replacement_hint.as_deref().unwrap_or("-"),
            );
            for argument in &call.arguments {
                println!(
                    "EFFECT-TRAMPOLINE-ARG\t{}\t{}\tposition={}\tname={}\ttype={}\tdirection={}\tvalue={}\tbinding={}\troot-arg={}\troot-offset={}",
                    function.identity,
                    call.trampoline.c_name,
                    argument.position,
                    argument.name,
                    argument.c_type,
                    argument.direction,
                    argument.value,
                    argument.binding,
                    argument
                        .root_argument
                        .map_or_else(|| "-".to_owned(), |value| value.to_string()),
                    argument
                        .root_offset
                        .map_or_else(|| "-".to_owned(), |value| format!("{value:+#x}")),
                );
            }
        }
        for blocker in &summary.blockers {
            println!("EFFECT-BLOCKER\t{}\t{}", function.identity, blocker);
        }
        for blocker in &summary.context_projection_blockers {
            println!("EFFECT-CONTEXT-BLOCKER\t{}\t{}", function.identity, blocker);
        }
    }
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
        println!(
            "MMIO-REGISTER\t{:#010x}\twidth={}\tnames={}\tread-shapes={}\twrite-shapes={}\tpoll-shapes={}\tstatic-shapes={}\tindexed-candidates={}\twhole-register-writes={}\trmw-writes={}\twrite-masks={}\tcandidate-bit-ranges={}\tfunctions={}",
            register.address,
            register.width,
            register.names.join("|"),
            register.read_shapes,
            register.write_shapes,
            register.poll_shapes,
            register.static_shapes,
            register.indexed_candidate_shapes,
            register.whole_register_write_shapes,
            register.read_modify_write_shapes,
            write_masks,
            candidate_bit_ranges,
            register.functions.join(","),
        );
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
    for slot in &report.trampoline_slots {
        let trampoline = &slot.trampoline;
        println!(
            "TRAMPOLINE-SLOT\t{}+{:#x}\tfunction={}\tid={}\tversion={}\tpointer={}\tbacking={}\tmagic={:#010x}@{:#x}\ttable-size={:#x}\targs={}\treturn-model={}\treturn-type={}\toperation={}\treplacement={}\tcall-shapes={}\tfunctions={}",
            trampoline.table,
            trampoline.slot,
            trampoline.c_name,
            trampoline.function_id,
            trampoline.version,
            trampoline.pointer_symbol,
            trampoline.backing_symbol,
            trampoline.magic,
            trampoline.magic_offset,
            trampoline.table_size,
            trampoline.argument_count,
            trampoline.return_model,
            trampoline.return_type,
            trampoline.operation,
            trampoline.replacement_hint.as_deref().unwrap_or("-"),
            slot.call_shapes,
            slot.functions.join(","),
        );
        for argument in &slot.arguments {
            println!(
                "TRAMPOLINE-ARG\t{}+{:#x}\tposition={}\tname={}\ttype={}\tdirection={}",
                trampoline.table,
                trampoline.slot,
                argument.position,
                argument.name,
                argument.c_type,
                argument.direction,
            );
        }
    }
    let root_functions = report
        .functions
        .iter()
        .filter(|function| function.selection == "symbol-prefix-root")
        .count();
    let included_reachable_functions = report.functions.len() - root_functions;
    println!(
        "SUMMARY\tartifacts={}\tfunctions={}\troot-functions={}\tincluded-reachable-functions={}\texported={}\tlocal={}\tmmio-registers={}\tmmio-functions={}\tmmio-access-shapes={}\tdelay-functions={}\tdelay-shapes={}\tcontext-functions={}\tcontext-fields={}\tcontext-accesses={}\tsemantic-operations={}\tsemantic-calls={}\ttrampoline-slots={}\ttrampoline-calls={}\tcomplete={}\tstructured={}\tinternal-calls={}\texternal-calls={}\tcall-argument-shapes={}\tproject-linked-calls={}\tambiguous-project-calls={}\tunresolved-calls={}\tclosed-effect-summaries={}\trecursive-effect-summaries={}\tcomplete-context-projections={}\tprojected-context-fields={}",
        artifacts.len(),
        report.functions.len(),
        root_functions,
        included_reachable_functions,
        report.exported_functions,
        report.local_functions,
        report.mmio_registers.len(),
        report.mmio_functions,
        report.mmio_access_shapes,
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
    );
}

fn write_pseudo(
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
    for function in &report.functions {
        let summary = &function.effect_summary;
        writeln!(output, "// SELECTION: {}", function.selection)
            .expect("writing to String cannot fail");
        writeln!(
            output,
            "// REACHABLE-EFFECTS: call-graph-closed={} max-depth={} functions={} mmio={} delays={} semantics={} semantic-actions={} trampolines={} context-fields={} context-projection-complete={} blockers={}",
            summary.call_graph_closed,
            summary.max_depth,
            summary.reachable_functions.len(),
            summary.mmio_registers.len(),
            summary.delays.len(),
            summary.semantic_operations.len(),
            summary.semantic_actions.len(),
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

fn write_optional_hex(output: &mut String, value: Option<u32>) {
    if let Some(value) = value {
        write_string(output, &format!("{value:#010x}"));
    } else {
        output.push_str("null");
    }
}

fn format_site_path(site_path: &[Option<u32>]) -> String {
    site_path
        .iter()
        .map(|site| site.map_or_else(|| "unknown".to_owned(), |site| format!("{site:#010x}")))
        .collect::<Vec<_>>()
        .join(" -> ")
}

fn write_trampoline(output: &mut String, trampoline: &LinkedTrampoline) {
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

fn write_semantic_contract(output: &mut String, contract: Option<&LinkedSemanticContract>) {
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

fn write_projected_arguments(output: &mut String, arguments: &[LinkedProjectedCallArgument]) {
    output.push('[');
    for (argument_index, argument) in arguments.iter().enumerate() {
        if argument_index != 0 {
            output.push_str(", ");
        }
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
    output.push(']');
}

fn write_diagnostics(output: &mut String, diagnostics: &[LinkedDiagnostic]) {
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

fn write_json_report(
    path: &Path,
    artifacts: &[IrArtifactInput],
    companions: &[PathBuf],
    symbol_prefix: &str,
    entry_contract: EntryContractRef,
    report: &LinkedIrReport,
    include_reachable: bool,
) -> Result<()> {
    let mut output = String::new();
    output.push_str("{\n  \"schema_version\": 19,\n  \"command\": \"ir-export\",\n");
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
    output.push_str("  \"semantic_action_mode\": \"lexical-site-paths-affine-root-bindings\",\n");
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
    writeln!(
        output,
        ",\n  \"summary\": {{\"artifacts\": {}, \"functions\": {}, \"root_functions\": {}, \"included_reachable_functions\": {}, \"exported\": {}, \"local\": {}, \"mmio_registers\": {}, \"mmio_functions\": {}, \"mmio_access_shapes\": {}, \"delay_functions\": {}, \"delay_shapes\": {}, \"context_functions\": {}, \"context_fields\": {}, \"context_accesses\": {}, \"semantic_operations\": {}, \"semantic_calls\": {}, \"trampoline_slots\": {}, \"trampoline_calls\": {}, \"complete\": {}, \"structured\": {}, \"internal_calls\": {}, \"external_calls\": {}, \"call_argument_shapes\": {}, \"project_linked_calls\": {}, \"ambiguous_project_calls\": {}, \"unresolved_calls\": {}, \"closed_effect_summaries\": {}, \"recursive_effect_summaries\": {}, \"complete_context_projections\": {}, \"projected_context_fields\": {}}},",
        artifacts.len(),
        report.functions.len(),
        root_functions,
        included_reachable_functions,
        report.exported_functions,
        report.local_functions,
        report.mmio_registers.len(),
        report.mmio_functions,
        report.mmio_access_shapes,
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
            ", \"read_shapes\": {}, \"write_shapes\": {}, \"poll_shapes\": {}, \"static_shapes\": {}, \"indexed_candidate_shapes\": {}, \"whole_register_write_shapes\": {}, \"read_modify_write_shapes\": {}, \"write_masks\": [",
            register.read_shapes,
            register.write_shapes,
            register.poll_shapes,
            register.static_shapes,
            register.indexed_candidate_shapes,
            register.whole_register_write_shapes,
            register.read_modify_write_shapes,
        )
        .expect("writing to String cannot fail");
        for (mask_index, mask) in register.write_masks.iter().enumerate() {
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
            output.push_str("], \"typed_arguments\": [");
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
            output.push_str(", \"site_path\": [");
            for (site_index, site) in action.site_path.iter().enumerate() {
                if site_index != 0 {
                    output.push_str(", ");
                }
                if let Some(site) = site {
                    write_string(&mut output, &format!("{site:#010x}"));
                } else {
                    output.push_str("null");
                }
            }
            output.push(']');
            write!(
                output,
                ", \"argument_shapes\": {}, \"arguments\": ",
                action.argument_shapes
            )
            .expect("writing to String cannot fail");
            write_projected_arguments(&mut output, &action.arguments);
            output.push('}');
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
    fs::write(path, output)?;
    println!("JSON-REPORT\t{}", path.display());
    Ok(())
}

pub(super) fn run(
    filtered: Vec<String>,
    svd: &MmioRegisterMap,
    target: &TargetSpec,
) -> Result<bool> {
    let mut artifacts = Vec::new();
    let mut companions = Vec::new();
    let mut symbol_prefix = String::new();
    let mut include_reachable = false;
    let mut pseudo_path = None;
    let mut json_report = None;
    let riscv_harness = harnesses::riscv(&target.harness)?;
    let mut entry_contract = harnesses::entry_contract(&target.harness, "none")?;
    let mut arguments = filtered.into_iter();
    while let Some(argument) = arguments.next() {
        if let Some(source) = source_artifact_option(&argument) {
            artifacts.push(named_artifact(
                source,
                &take_value(&mut arguments, &argument)?,
            )?);
            continue;
        }
        match argument.as_str() {
            "--artifact" => {
                artifacts.push(parse_artifact(&take_value(&mut arguments, "--artifact")?)?);
            }
            "--companion" => {
                companions.push(PathBuf::from(take_value(&mut arguments, "--companion")?));
            }
            "--symbol-prefix" => {
                symbol_prefix = take_value(&mut arguments, "--symbol-prefix")?;
            }
            "--include-reachable" => include_reachable = true,
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
    let namespace_identities = validate_artifact_inputs(&artifacts, &companions)?;
    let mut reports = Vec::with_capacity(artifacts.len());
    for artifact in &artifacts {
        let resolver = ReferenceResolver::load_all_code_with_entry_contract(
            &artifact.path,
            &companions,
            riscv_harness,
            entry_contract,
        )?;
        reports.push(build_linked_ir_for_source(
            &resolver,
            &symbol_prefix,
            svd,
            &artifact.source,
            namespace_identities,
            include_reachable,
        ));
    }
    if artifacts.len() > 1 {
        link_project_calls(&mut reports);
    }
    let report = merge_linked_ir(reports);
    if report.functions.is_empty() {
        return Err(format!(
            "no named code symbols start with {symbol_prefix:?} in any IR artifact"
        )
        .into());
    }

    print_report(&artifacts, &report, include_reachable);
    if let Some(path) = pseudo_path.as_deref() {
        write_pseudo(path, &artifacts, &report, include_reachable)?;
    }
    if let Some(path) = json_report.as_deref() {
        write_json_report(
            path,
            &artifacts,
            &companions,
            &symbol_prefix,
            entry_contract,
            &report,
            include_reachable,
        )?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_input_supports_legacy_paths_and_explicit_source_names() {
        assert_eq!(
            parse_artifact("vendor.a").unwrap(),
            IrArtifactInput {
                source: "primary".to_owned(),
                path: PathBuf::from("vendor.a"),
                explicitly_named: false,
            }
        );
        assert_eq!(
            parse_artifact("libphy=/tmp/vendor=archive.a").unwrap(),
            IrArtifactInput {
                source: "libphy".to_owned(),
                path: PathBuf::from("/tmp/vendor=archive.a"),
                explicitly_named: true,
            }
        );
        assert_eq!(
            parse_artifact("/tmp/vendor=archive.a").unwrap(),
            IrArtifactInput {
                source: "primary".to_owned(),
                path: PathBuf::from("/tmp/vendor=archive.a"),
                explicitly_named: false,
            }
        );
    }

    #[test]
    fn artifact_source_ids_are_stable_machine_keys() {
        assert!(named_artifact("wifi-rom.v1", "rom.elf").is_ok());
        assert!(named_artifact("wifi/rom", "rom.elf").is_err());
        assert!(named_artifact("", "rom.elf").is_err());
    }

    #[test]
    fn project_inputs_require_unique_explicit_sources_and_no_companions() {
        let rom = named_artifact("rom", "rom.elf").unwrap();
        let libphy = named_artifact("libphy", "libphy.a").unwrap();
        assert!(validate_artifact_inputs(&[rom.clone(), libphy], &[]).unwrap());
        assert!(validate_artifact_inputs(&[rom.clone(), rom], &[]).is_err());
        assert!(
            validate_artifact_inputs(
                &[
                    parse_artifact("rom.elf").unwrap(),
                    parse_artifact("libphy.a").unwrap()
                ],
                &[],
            )
            .is_err()
        );
        assert!(
            validate_artifact_inputs(
                &[
                    named_artifact("rom", "rom.elf").unwrap(),
                    named_artifact("libphy", "libphy.a").unwrap()
                ],
                &[PathBuf::from("rom-companion.elf")],
            )
            .is_err()
        );
    }
}
