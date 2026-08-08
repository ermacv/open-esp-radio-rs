//! Identity, provenance, calls, and local memory/effect facts.

use std::fmt::Write as _;

use super::super::super::*;
use super::memory_object_label;

pub(super) fn render(mut output: &mut String, function: &LinkedIrFunction) {
    let address = function.address.map_or_else(
        || "relocatable".to_owned(),
        |address| format!("{address:#010x}"),
    );
    let _ = writeln!(
        &mut output,
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
    let _ = writeln!(
        &mut output,
        "RETURN-PROVENANCE\t{}\texact={}\tknown-zero-bits={:#010x}\tknown-one-bits={:#010x}\tunknown-bits={:#010x}\tsources={}",
        function.identity,
        function.return_provenance.exact,
        function.return_provenance.known_zero_bits,
        function.return_provenance.known_one_bits,
        function.return_provenance.unknown_bits,
        function.return_provenance.sources.len(),
    );
    for source in &function.return_provenance.sources {
        let _ = writeln!(
            &mut output,
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
            let _ = writeln!(
                &mut output,
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
            let _ = writeln!(
                &mut output,
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
        let _ = writeln!(
            &mut output,
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
            let _ = writeln!(
                &mut output,
                "CALL-GUARD\t{}\t{}\t{}",
                function.identity,
                call.target,
                format_guard_paths(paths),
            );
        }
        for argument in &call.typed_arguments {
            let _ = writeln!(
                &mut output,
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
            let _ = writeln!(
                &mut output,
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
        let _ = writeln!(
            &mut output,
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
        let _ = writeln!(
            &mut output,
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
        let _ = writeln!(
            &mut output,
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
        let _ = writeln!(
            &mut output,
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
        let _ = writeln!(
            &mut output,
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
        let _ = writeln!(
            &mut output,
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
        let _ = writeln!(
            &mut output,
            "IR-DIAGNOSTIC\t{}\tdirect\t{blocker}",
            function.identity
        );
    }
    for blocker in &function.reference_blockers {
        let _ = writeln!(
            &mut output,
            "IR-DIAGNOSTIC\t{}\treference\t{blocker}",
            function.identity
        );
    }
    for blocker in &function.call_graph_blockers {
        let _ = writeln!(
            &mut output,
            "IR-DIAGNOSTIC\t{}\tcall-graph\t{blocker}",
            function.identity
        );
    }
}
