//! Reachable transitive-effect and projected-context facts.

use std::fmt::Write as _;

use super::super::super::*;
use super::memory_object_label;

pub(super) fn render(mut output: &mut String, function: &LinkedIrFunction) {
    let summary = &function.effect_summary;
    let _ = writeln!(
        &mut output,
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
        let _ = writeln!(
            &mut output,
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
        let _ = writeln!(
            &mut output,
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
        let _ = writeln!(
            &mut output,
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
        let _ = writeln!(
            &mut output,
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
                let _ = writeln!(
                    &mut output,
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
                    let _ = writeln!(
                        &mut output,
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
                    let _ = writeln!(
                        &mut output,
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
            let _ = writeln!(
                &mut output,
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
        let _ = writeln!(
            &mut output,
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
            let _ = writeln!(
                &mut output,
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
        let _ = writeln!(
            &mut output,
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
        let _ = writeln!(
            &mut output,
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
        let _ = writeln!(
            &mut output,
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
            let _ = writeln!(
                &mut output,
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
        let _ = writeln!(
            &mut output,
            "EFFECT-BLOCKER\t{}\t{}",
            function.identity, blocker
        );
    }
    for blocker in &summary.context_projection_blockers {
        let _ = writeln!(
            &mut output,
            "EFFECT-CONTEXT-BLOCKER\t{}\t{}",
            function.identity, blocker
        );
    }
}
