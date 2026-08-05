//! Linked best-effort function/call IR export.

use std::{collections::BTreeSet, fmt::Write as _, path::Path};

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

fn provenance_summary(report: &LinkedIrReport) -> (usize, usize, usize, usize, usize) {
    let exact_return_functions = report
        .functions
        .iter()
        .filter(|function| function.return_provenance.exact)
        .count();
    let return_source_ranges = report
        .functions
        .iter()
        .map(|function| function.return_provenance.sources.len())
        .sum();
    let mmio_return_sources = report
        .functions
        .iter()
        .flat_map(|function| &function.return_provenance.sources)
        .filter(|source| source.kind == "mmio-read")
        .count();
    let guard_mmio_links = report
        .functions
        .iter()
        .flat_map(|function| &function.calls)
        .filter_map(|call| call.guard_paths.as_deref())
        .flatten()
        .flat_map(|path| &path.guards)
        .flat_map(|guard| &guard.result_sources)
        .map(|source| source.mmio_sources.len())
        .sum();
    let transitive_guard_mmio_links = report
        .functions
        .iter()
        .flat_map(|function| &function.calls)
        .filter_map(|call| call.guard_paths.as_deref())
        .flatten()
        .flat_map(|path| &path.guards)
        .flat_map(|guard| &guard.result_sources)
        .flat_map(|source| &source.mmio_sources)
        .filter(|source| source.producer_path.len() > 1)
        .count();
    (
        exact_return_functions,
        return_source_ranges,
        mmio_return_sources,
        guard_mmio_links,
        transitive_guard_mmio_links,
    )
}

fn field_candidate_summary(report: &LinkedIrReport) -> (usize, usize, usize, usize) {
    let registers = report
        .mmio_registers
        .iter()
        .filter(|register| !register.field_candidates.is_empty())
        .count();
    let candidates = report
        .mmio_registers
        .iter()
        .map(|register| register.field_candidates.len())
        .sum();
    let direct_predicates = report
        .functions
        .iter()
        .map(|function| function.direct_mmio_predicates.len())
        .sum();
    let direct_sources = report
        .functions
        .iter()
        .flat_map(|function| &function.direct_mmio_predicates)
        .map(|predicate| predicate.sources.len())
        .sum();
    (registers, candidates, direct_predicates, direct_sources)
}

fn print_report(artifacts: &[IrArtifactInput], report: &LinkedIrReport, include_reachable: bool) {
    println!(
        "PROJECT\tlinkage={}\tcall-linkage={}\tselection={}\tcall-compaction=stable-identity-universal-affine-bindings\tdiagnostic-compaction=exact-semicolon-fragment-inventory\tcontext-projection=affine-simple-call-paths\treturn-provenance=exact-bit-ranges-with-constant-and-unknown-masks\tsemantic-actions=lexical-site-paths-factorized-cfg-guards-affine-root-bindings\tevent-dispatch=reviewed-semantic-operation-role-projection\tevent-dispatch-effect-completeness-claim=false\tevent-dispatch-receiver-inference=none\tcfg-guards=forced-branch-paths-minimized-dnf-factorized-by-function\tcfg-guard-expressions=pseudo-rust-aligned-bit-masks-with-symbolic-fallback\tcfg-guard-result-sources=bit-provenance-with-operand-comparison-mapping-and-producer-targets\tcfg-guard-mmio-linkage=recursive-exact-bit-projection-with-producer-paths\tdirect-mmio-predicates=exact-bit-provenance-with-constant-comparison-mapping\tsemantic-field-guards=action-identity-and-path-coordinate-preserving\tdirect-mmio-predicate-completeness-claim=false\tmmio-field-candidates=contiguous-subregister-write-poll-and-direct-guard-evidence\tmmio-field-semantics-claim=false\tcfg-guard-completeness-claim=false\tartifacts={}",
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
        println!(
            "RETURN-PROVENANCE\t{}\texact={}\tknown-zero-bits={:#010x}\tknown-one-bits={:#010x}\tunknown-bits={:#010x}\tsources={}",
            function.identity,
            function.return_provenance.exact,
            function.return_provenance.known_zero_bits,
            function.return_provenance.known_one_bits,
            function.return_provenance.unknown_bits,
            function.return_provenance.sources.len(),
        );
        for source in &function.return_provenance.sources {
            println!(
                "RETURN-SOURCE\t{}\t{}\toutput-bits={:#010x}\tsource-bits={:#010x}\toutput-lsb={}\tsource-lsb={}\twidth={}\tinverted={}\targument={}\ttoken={}\ttarget={}\taddress={}\tregister={}",
                function.identity,
                source.kind,
                source.output_bits,
                source.source_bits,
                source.output_lsb,
                source.source_lsb,
                source.width,
                source.inverted,
                source
                    .argument
                    .map_or_else(|| "-".to_owned(), |value| value.to_string()),
                source
                    .token
                    .map_or_else(|| "-".to_owned(), |value| value.to_string()),
                source.target.as_deref().unwrap_or("-"),
                source
                    .address
                    .map_or_else(|| "-".to_owned(), |value| format!("{value:#010x}")),
                source.register.as_deref().unwrap_or("-"),
            );
        }
        for predicate in &function.direct_mmio_predicates {
            for source in &predicate.sources {
                let optional_hex = |value: Option<u32>| {
                    value.map_or_else(|| "-".to_owned(), |value| format!("{value:#010x}"))
                };
                println!(
                    "MMIO-PREDICATE\t{}\tsite={:#010x}\toperation={}\tcondition={}\toperand={}\tread-token={}\t{:#010x}\tregister={}\tvalue-bits={:#010x}\tregister-bits={:#010x}\tinverted={}\tcomparison-value={}\tregister-comparison-value={}",
                    function.identity,
                    predicate.site,
                    predicate.operation,
                    predicate.condition,
                    source.operand,
                    source.read_token,
                    source.address,
                    source.register,
                    source.value_bits,
                    source.register_bits,
                    source.inverted,
                    optional_hex(source.comparison_value),
                    optional_hex(source.register_comparison_value),
                );
            }
        }
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
            let guard_paths = call
                .guard_paths
                .as_ref()
                .map_or_else(|| "unknown".to_owned(), |paths| paths.len().to_string());
            println!(
                "CALL\t{}\t{}\t{}\tsite={}\ttail={}\tresult-modeled={}\toperation={}\tsemantic-source={}\tsemantic-contract={}\tsemantic-evidence={}\treplacement={}\ttrampoline={}\tproject-symbol={}\tproject-candidates={}\targument-shapes={}\taffine-bindings={}\tcfg-guard-paths={}\t{}",
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
                guard_paths,
                call.semantics.as_deref().unwrap_or("-"),
            );
            if call.semantic_operation.is_some()
                && let Some(paths) = call.guard_paths.as_deref()
            {
                println!(
                    "CALL-GUARD\t{}\t{}\t{}",
                    function.identity,
                    call.target,
                    format_guard_paths(paths),
                );
            }
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
                "MMIO\t{}\tordinal={}\t{:#010x}\twidth={}\tregister={}\taccess={}\tmode={}\tpath={}\taddress-expression={}\tguard={}\tpredicate-mask={}\tpredicate-expected={}\tvalue={}\tmodified={}\tpreserved={}\tinverted={}\tforced-zero={}\tforced-one={}\tread-derived={}\tdynamic={}",
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
                mask(access.predicate_mask),
                mask(access.predicate_expected),
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
            "EFFECT-SUMMARY\t{}\tcall-graph-closed={}\tmax-depth={}\treachable-functions={}\trecursive-functions={}\tmmio-registers={}\tdelays={}\tsemantic-operations={}\tsemantic-actions={}\tevent-dispatches={}\ttrampoline-calls={}\tcontext-projection-complete={}\tcontext-fields={}\tblockers={}\tcontext-blockers={}",
            function.identity,
            summary.call_graph_closed,
            summary.max_depth,
            summary.reachable_functions.join(","),
            summary.recursive_functions.join(","),
            summary.mmio_registers.len(),
            summary.delays.len(),
            summary.semantic_operations.len(),
            summary.semantic_actions.len(),
            summary.event_dispatches.len(),
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
            let guard_scopes = action
                .guard_scopes
                .as_ref()
                .map_or_else(|| "unknown".to_owned(), |scopes| scopes.len().to_string());
            println!(
                "EFFECT-ACTION\t{}\t{}\ttarget={}\tsite={}\tsite-path={}\tcfg-guard-scopes={}\targument-shapes={}\torigin={}\tpath={}\tsemantic-source={}\tsemantic-contract={}\tsemantic-evidence={}\treplacement={}",
                function.identity,
                action.operation,
                action.target,
                site,
                format_site_path(&action.site_path),
                guard_scopes,
                action.argument_shapes,
                action.origin,
                action.path,
                semantic_source,
                semantic_contract,
                semantic_evidence,
                action.replacement_hint.as_deref().unwrap_or("-"),
            );
            if let Some(scopes) = action.guard_scopes.as_deref() {
                for scope in scopes {
                    println!(
                        "EFFECT-ACTION-GUARD\t{}\t{}\tscope={}\talternatives={}\texpression={}",
                        function.identity,
                        action.operation,
                        scope.function,
                        scope.paths.len(),
                        format_guard_paths(&scope.paths),
                    );
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
                        println!(
                            "EFFECT-ACTION-GUARD-MMIO\t{}\t{}\tscope={}\tproducer={}\tproducer-path={}\treturn-depth={}\taddress={:#010x}\tregister={}\tresult-bits={:#010x}\tregister-bits={:#010x}\tsite={:#010x}\tcondition={}\toperation={}\ttaken={}\teffective-operation={}\toperand={}\tcomparison-value={}\tsource-comparison-value={}\tresult-comparison-value={}\tregister-comparison-value={}\tinverted={}",
                            function.identity,
                            action.operation,
                            scope.function,
                            producer,
                            mmio.producer_path.join(" -> "),
                            mmio.producer_path.len().saturating_sub(1),
                            mmio.address,
                            mmio.register,
                            mmio.result_bits,
                            mmio.register_bits,
                            site,
                            condition,
                            operation,
                            taken,
                            effective_branch_operation(operation, taken),
                            operand,
                            optional_hex_text(comparison_value),
                            optional_hex_text(source_comparison_value),
                            optional_hex_text(mmio.result_comparison_value),
                            optional_hex_text(mmio.register_comparison_value),
                            mmio.inverted,
                        );
                    }
                    for (site, condition, operation, taken, mmio) in
                        guard_direct_mmio_links(&scope.paths)
                    {
                        println!(
                            "EFFECT-ACTION-GUARD-DIRECT-MMIO\t{}\t{}\tscope={}\taddress={:#010x}\tregister={}\tregister-bits={:#010x}\tsite={:#010x}\tcondition={}\toperation={}\ttaken={}\teffective-operation={}\toperand={}\tcomparison-value={}\tregister-comparison-value={}\tinverted={}",
                            function.identity,
                            action.operation,
                            scope.function,
                            mmio.address,
                            mmio.register,
                            mmio.register_bits,
                            site,
                            condition,
                            operation,
                            taken,
                            effective_branch_operation(operation, taken),
                            mmio.operand,
                            optional_hex_text(mmio.comparison_value),
                            optional_hex_text(mmio.register_comparison_value),
                            mmio.inverted,
                        );
                    }
                }
            }
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
        for dispatch in &summary.event_dispatches {
            let action = &summary.semantic_actions[dispatch.semantic_action_index];
            println!(
                "EFFECT-EVENT-DISPATCH\t{}\tmechanism={}\texecution-context={}\treceiver={}\tinterface-complete={}\tsemantic-action-index={}\toperation={}\ttarget={}\torigin={}\tsite-path={}\tcfg-guard-scopes={}\tpath={}\tblockers={}",
                function.identity,
                dispatch.mechanism,
                dispatch.execution_context,
                dispatch.receiver.as_deref().unwrap_or("unknown"),
                dispatch.interface_complete,
                dispatch.semantic_action_index,
                action.operation,
                action.target,
                action.origin,
                format_site_path(&action.site_path),
                action
                    .guard_scopes
                    .as_ref()
                    .map_or_else(|| "unknown".to_owned(), |scopes| scopes.len().to_string()),
                action.path,
                dispatch.blockers.join(" | "),
            );
            for binding in &dispatch.bindings {
                let argument = &binding.argument;
                println!(
                    "EFFECT-EVENT-DISPATCH-ARG\t{}\t{}\trole={}\tposition={}\tname={}\ttype={}\tdirection={}\tvalue={}\tbinding={}\troot-arg={}\troot-offset={}",
                    function.identity,
                    dispatch.semantic_action_index,
                    binding.role,
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
        println!(
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
            println!(
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
                println!(
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
                println!(
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
    println!(
        "SUMMARY\tartifacts={}\tfunctions={}\troot-functions={}\tincluded-reachable-functions={}\texported={}\tlocal={}\tmmio-registers={}\tmmio-functions={}\tmmio-access-shapes={}\tmmio-field-candidate-registers={}\tmmio-field-candidates={}\tdirect-mmio-predicates={}\tdirect-mmio-predicate-sources={}\tdelay-functions={}\tdelay-shapes={}\tcontext-functions={}\tcontext-fields={}\tcontext-accesses={}\tsemantic-operations={}\tsemantic-calls={}\ttrampoline-slots={}\ttrampoline-calls={}\tcomplete={}\tstructured={}\tinternal-calls={}\texternal-calls={}\tcall-argument-shapes={}\tproject-linked-calls={}\tambiguous-project-calls={}\tunresolved-calls={}\tclosed-effect-summaries={}\trecursive-effect-summaries={}\tcomplete-context-projections={}\tprojected-context-fields={}\texact-return-functions={}\treturn-source-ranges={}\tmmio-return-sources={}\tguard-mmio-links={}\ttransitive-guard-mmio-links={}",
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

fn write_site_path(output: &mut String, site_path: &[Option<u32>]) {
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

fn optional_hex_text(value: Option<u32>) -> String {
    value.map_or_else(|| "-".to_owned(), |value| format!("{value:#010x}"))
}

type ProducerMmioGuardLink = (
    u32,
    String,
    &'static str,
    bool,
    String,
    &'static str,
    Option<u32>,
    Option<u32>,
    LinkedCallGuardMmioSource,
);

fn guard_mmio_links(paths: &[LinkedCallGuardPath]) -> Vec<ProducerMmioGuardLink> {
    paths
        .iter()
        .flat_map(|path| &path.guards)
        .flat_map(|guard| {
            guard.result_sources.iter().flat_map(|source| {
                source.mmio_sources.iter().cloned().map(|mmio| {
                    (
                        guard.site,
                        guard.condition.clone(),
                        guard.operation,
                        guard.taken,
                        source
                            .target
                            .clone()
                            .unwrap_or_else(|| "unknown".to_owned()),
                        source.operand,
                        source.comparison_value,
                        source.source_comparison_value,
                        mmio,
                    )
                })
            })
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

type DirectMmioGuardLink = (
    u32,
    String,
    &'static str,
    bool,
    LinkedDirectMmioPredicateSource,
);

fn guard_direct_mmio_links(paths: &[LinkedCallGuardPath]) -> Vec<DirectMmioGuardLink> {
    paths
        .iter()
        .flat_map(|path| &path.guards)
        .flat_map(|guard| {
            guard.direct_mmio_sources.iter().cloned().map(|source| {
                (
                    guard.site,
                    guard.condition.clone(),
                    guard.operation,
                    guard.taken,
                    source,
                )
            })
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn write_return_provenance(output: &mut String, provenance: &LinkedReturnProvenance) {
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

fn write_direct_mmio_source(output: &mut String, source: &LinkedDirectMmioPredicateSource) {
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

fn write_guard_paths(output: &mut String, paths: Option<&[LinkedCallGuardPath]>) {
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

fn write_guard_scopes(output: &mut String, scopes: Option<&[LinkedCallGuardScope]>) {
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

fn write_projected_argument(output: &mut String, argument: &LinkedProjectedCallArgument) {
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

fn write_projected_arguments(output: &mut String, arguments: &[LinkedProjectedCallArgument]) {
    output.push('[');
    for (argument_index, argument) in arguments.iter().enumerate() {
        if argument_index != 0 {
            output.push_str(", ");
        }
        write_projected_argument(output, argument);
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
    output.push_str("{\n  \"schema_version\": 29,\n  \"command\": \"ir-export\",\n");
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
    output
        .push_str("  \"event_dispatch_mode\": \"reviewed-semantic-operation-role-projection\",\n");
    output.push_str("  \"event_dispatch_effect_completeness_claim\": false,\n");
    output.push_str("  \"event_dispatch_receiver_inference_mode\": \"none\",\n");
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
