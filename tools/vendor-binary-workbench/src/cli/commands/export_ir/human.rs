//! Tabular human-readable linked-IR report rendering.

use super::*;

pub(super) fn print_report(
    artifacts: &[IrArtifactInput],
    report: &LinkedIrReport,
    include_reachable: bool,
) {
    outputln!(
        "PROJECT\tlinkage={}\tcall-linkage={}\tselection={}\tcall-compaction=stable-identity-universal-affine-bindings\tdiagnostic-compaction=exact-semicolon-fragment-inventory\tcontext-projection=affine-simple-call-paths\treturn-provenance=exact-bit-ranges-with-constant-and-unknown-masks\tsemantic-actions=lexical-site-paths-factorized-cfg-guards-affine-root-bindings\tevent-dispatch=reviewed-contract-declared-role-projection\tevent-dispatch-effect-completeness-claim=false\tevent-dispatch-receiver-inference=none\tevent-dispatch-receiver-source=reviewed-contract-or-unknown\tcfg-guards=forced-branch-paths-minimized-dnf-factorized-by-function\tcfg-guard-expressions=pseudo-rust-aligned-bit-masks-with-symbolic-fallback\tcfg-guard-result-sources=bit-provenance-with-operand-comparison-mapping-and-producer-targets\tcfg-guard-mmio-linkage=recursive-exact-bit-projection-with-producer-paths\tdirect-mmio-predicates=exact-bit-provenance-with-constant-comparison-mapping\tsemantic-field-guards=action-identity-and-path-coordinate-preserving\tdirect-mmio-predicate-completeness-claim=false\tscenario-suggestions=structural-candidates-require-concrete-replay\tscenario-suggestion-proof-claim=false\tmmio-field-candidates=contiguous-subregister-write-poll-and-direct-guard-evidence\tmmio-field-semantics-claim=false\tcfg-guard-completeness-claim=false\tartifacts={}",
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
        outputln!(
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
        outputln!(
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
        outputln!(
            "RETURN-PROVENANCE\t{}\texact={}\tknown-zero-bits={:#010x}\tknown-one-bits={:#010x}\tunknown-bits={:#010x}\tsources={}",
            function.identity,
            function.return_provenance.exact,
            function.return_provenance.known_zero_bits,
            function.return_provenance.known_one_bits,
            function.return_provenance.unknown_bits,
            function.return_provenance.sources.len(),
        );
        for source in &function.return_provenance.sources {
            outputln!(
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
                outputln!(
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
        for suggestion in &function.scenario_suggestions {
            for variant in &suggestion.variants {
                let arguments = variant
                    .arguments
                    .iter()
                    .map(|argument| format!("a{}={:#010x}", argument.index, argument.value))
                    .collect::<Vec<_>>()
                    .join(",");
                let reads = variant
                    .mmio_reads
                    .iter()
                    .map(|read| {
                        format!(
                            "{:#010x}=[{}]",
                            read.address,
                            read.values
                                .iter()
                                .map(|value| format!("{value:#010x}"))
                                .collect::<Vec<_>>()
                                .join(",")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(";");
                outputln!(
                    "SCENARIO-SUGGESTION\t{}\tkind={}\tsite={}\tvariant={}\targuments={}\treads={}\tevidence={}",
                    function.identity,
                    suggestion.kind,
                    suggestion
                        .site
                        .map_or_else(|| "-".to_owned(), |site| format!("{site:#010x}")),
                    variant.name,
                    arguments,
                    reads,
                    suggestion.evidence,
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
            outputln!(
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
                outputln!(
                    "CALL-GUARD\t{}\t{}\t{}",
                    function.identity,
                    call.target,
                    format_guard_paths(paths),
                );
            }
            for argument in &call.typed_arguments {
                outputln!(
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
                outputln!(
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
            outputln!(
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
            outputln!(
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
            outputln!(
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
            outputln!(
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
        for field in &function.memory_fields {
            outputln!(
                "MEMORY-FIELD\t{}\tobject={}\toffset={:+#x}\twidth={}\treads={}\twrites={}\twrite-mask={:#010x}\tpaths={}\tvalues={}",
                function.identity,
                memory_object_label(&field.object),
                field.offset,
                field.width,
                field.reads,
                field.writes,
                field.write_mask,
                field.paths.len(),
                field.write_values.join(" | "),
            );
        }
        for access in &function.memory_accesses {
            outputln!(
                "MEMORY\t{}\tobject={}\toffset={:+#x}\twidth={}\taccess={}\tpath={}\tvalue={}",
                function.identity,
                memory_object_label(&access.object),
                access.offset,
                access.width,
                access.access,
                access.path,
                access.value.as_deref().unwrap_or("-"),
            );
        }
        for blocker in &function.direct_blockers {
            outputln!("IR-DIAGNOSTIC\t{}\tdirect\t{blocker}", function.identity);
        }
        for blocker in &function.reference_blockers {
            outputln!("IR-DIAGNOSTIC\t{}\treference\t{blocker}", function.identity);
        }
        for blocker in &function.call_graph_blockers {
            outputln!(
                "IR-DIAGNOSTIC\t{}\tcall-graph\t{blocker}",
                function.identity
            );
        }
        let summary = &function.effect_summary;
        outputln!(
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
            outputln!(
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
            outputln!(
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
            outputln!(
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
            outputln!(
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
                    outputln!(
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
                        outputln!(
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
                        outputln!(
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
                outputln!(
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
            outputln!(
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
                outputln!(
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
            outputln!(
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
        for field in &summary.memory_fields {
            outputln!(
                "EFFECT-MEMORY-FIELD\t{}\tobject={}\toffset={:+#x}\twidth={}\treads={}\twrites={}\twrite-mask={:#010x}\torigins={}\tpaths={}\tvalues={}",
                function.identity,
                memory_object_label(&field.object),
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
            outputln!(
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
                outputln!(
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
            outputln!("EFFECT-BLOCKER\t{}\t{}", function.identity, blocker);
        }
        for blocker in &summary.context_projection_blockers {
            outputln!("EFFECT-CONTEXT-BLOCKER\t{}\t{}", function.identity, blocker);
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
        outputln!(
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
            outputln!(
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
                outputln!(
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
                outputln!(
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
        outputln!(
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
        outputln!(
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
            outputln!(
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
    outputln!(
        "SUMMARY\tartifacts={}\tfunctions={}\troot-functions={}\tincluded-reachable-functions={}\texported={}\tlocal={}\tmmio-registers={}\tmmio-functions={}\tmmio-access-shapes={}\tmmio-field-candidate-registers={}\tmmio-field-candidates={}\tdirect-mmio-predicates={}\tdirect-mmio-predicate-sources={}\tdelay-functions={}\tdelay-shapes={}\tcontext-functions={}\tcontext-fields={}\tcontext-accesses={}\tmemory-functions={}\tmemory-fields={}\tmemory-accesses={}\tsemantic-operations={}\tsemantic-calls={}\ttrampoline-slots={}\ttrampoline-calls={}\tcomplete={}\tstructured={}\tinternal-calls={}\texternal-calls={}\tcall-argument-shapes={}\tproject-linked-calls={}\tambiguous-project-calls={}\tunresolved-calls={}\tclosed-effect-summaries={}\trecursive-effect-summaries={}\tcomplete-context-projections={}\tprojected-context-fields={}\tprojected-memory-fields={}\texact-return-functions={}\treturn-source-ranges={}\tmmio-return-sources={}\tguard-mmio-links={}\ttransitive-guard-mmio-links={}\tscenario-suggestions={}",
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
        report.memory_functions,
        report.memory_fields,
        report.memory_accesses,
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
        report.projected_memory_fields,
        exact_return_functions,
        return_source_ranges,
        mmio_return_sources,
        guard_mmio_links,
        transitive_guard_mmio_links,
        report.scenario_suggestions,
    );
}

fn memory_object_label(object: &LinkedMemoryObject) -> String {
    match object {
        LinkedMemoryObject::Argument { index } => format!("argument:{index}"),
        LinkedMemoryObject::Global { member, symbol } => format!(
            "global:{}::{symbol}",
            member.as_deref().unwrap_or("<linked>")
        ),
        LinkedMemoryObject::DereferencedGlobal {
            member,
            symbol,
            pointer_offset,
        } => format!(
            "dereferenced-global:{}::{symbol}{pointer_offset:+#x}",
            member.as_deref().unwrap_or("<linked>")
        ),
        LinkedMemoryObject::Absolute {
            address_space,
            address,
        } => format!("absolute:{address_space}:{address:#010x}"),
    }
}
