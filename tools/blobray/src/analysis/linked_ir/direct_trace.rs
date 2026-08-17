//! Direct call-graph exploration and guarded MMIO provenance.

use super::*;

#[derive(Default)]
pub(super) struct DirectCallGraph {
    pub(super) calls: BTreeSet<LinkedCall>,
    pub(super) direct_mmio_predicates: BTreeSet<LinkedDirectMmioPredicate>,
    pub(super) blockers: BTreeSet<String>,
    pub(super) site_effects: BTreeSet<LinkedInstructionEffect>,
}

pub(super) const MAX_SITE_EFFECT_VARIANTS: usize = 16;

pub(super) fn record_site_effect(
    result: &mut DirectCallGraph,
    site_effect_counts: &mut BTreeMap<u32, usize>,
    truncated_effect_sites: &mut BTreeSet<u32>,
    effect: LinkedInstructionEffect,
) {
    let site = effect.site();
    if result.site_effects.contains(&effect) {
        return;
    }
    let variants = site_effect_counts.entry(site).or_default();
    if *variants < MAX_SITE_EFFECT_VARIANTS {
        result.site_effects.insert(effect);
        *variants += 1;
    } else if truncated_effect_sites.insert(site) {
        if result.blockers.len() < 64 {
            result.blockers.insert(format!(
                "instruction site {site:#010x} exceeds the limit of {MAX_SITE_EFFECT_VARIANTS} distinct effect variants; retained site facts are incomplete"
            ));
        } else {
            result.blockers.insert(
                "additional call-graph diagnostics omitted by the linked-IR exploration budget"
                    .to_owned(),
            );
        }
    }
}

#[derive(Clone)]
pub(super) struct DirectGuardEvidence {
    taken: bool,
    operation: &'static str,
    result_sources: Vec<LinkedCallGuardResultSource>,
    direct_mmio_sources: Vec<LinkedDirectMmioPredicateSource>,
}

pub(super) type DirectGuardState = BTreeMap<(u32, String), DirectGuardEvidence>;

#[derive(Default)]
pub(super) struct DirectTraceEvidence {
    pub(super) guards: DirectGuardState,
    pub(super) call_results: BTreeMap<u32, String>,
    pub(super) calls: BTreeSet<LinkedCall>,
    pub(super) direct_mmio_predicates: BTreeSet<LinkedDirectMmioPredicate>,
}

pub(super) fn current_guard_path(guards: &DirectGuardState) -> LinkedCallGuardPath {
    LinkedCallGuardPath {
        guards: guards
            .iter()
            .map(|((site, condition), evidence)| LinkedCallGuard {
                site: *site,
                condition: condition.clone(),
                operation: evidence.operation,
                taken: evidence.taken,
                result_sources: evidence.result_sources.clone(),
                direct_mmio_sources: evidence.direct_mmio_sources.clone(),
            })
            .collect(),
    }
}

pub(super) fn collect_guard_result_source_bits(
    value: &SymbolicValue,
    sources: &mut BTreeMap<(&'static str, u32), u32>,
) {
    let mut recovered_bits = false;
    for source in value.bits() {
        let (kind, token, bit) = match source {
            BitSource::CallResult {
                call_token, bit, ..
            } => ("call-result", call_token, bit),
            BitSource::ExternalResult {
                call_token, bit, ..
            } => (
                "external-result",
                external_result_call_token(call_token),
                bit,
            ),
            _ => continue,
        };
        *sources.entry((kind, token)).or_default() |= 1_u32 << bit;
        recovered_bits = true;
    }
    if recovered_bits {
        return;
    }
    for nested in value.tree().skip(1) {
        for source in nested.bits() {
            let (kind, token, bit) = match source {
                BitSource::CallResult {
                    call_token, bit, ..
                } => ("call-result", call_token, bit),
                BitSource::ExternalResult {
                    call_token, bit, ..
                } => (
                    "external-result",
                    external_result_call_token(call_token),
                    bit,
                ),
                _ => continue,
            };
            *sources.entry((kind, token)).or_default() |= 1_u32 << bit;
        }
    }
}

#[cfg(test)]
mod floating_provenance_tests {
    use open_radio_vendor_analysis_model::{FloatingPointOperation, FloatingRoundingMode};

    use super::*;

    #[test]
    fn floating_guard_keeps_nested_call_result_provenance() {
        let value = SymbolicValue::FloatingPoint {
            operation: FloatingPointOperation::SignedWordToSingle,
            rounding: FloatingRoundingMode::Dynamic,
            operands: vec![SymbolicValue::CallResult(8)].into_boxed_slice(),
        };
        let mut sources = BTreeMap::new();

        collect_guard_result_source_bits(&value, &mut sources);

        assert_eq!(sources.get(&("call-result", 8)), Some(&u32::MAX));
    }
}

#[derive(Default)]
pub(super) struct GuardResultSourceAccumulator {
    value_bits: u32,
    source_bits: u32,
    comparison_known_bits: u32,
    comparison_one_bits: u32,
    comparison_conflict: bool,
}

pub(super) fn exact_guard_result_operand_sources(
    value: &SymbolicValue,
    operand: &'static str,
    comparison_value: Option<u32>,
    operation: BranchOperation,
    call_results: &BTreeMap<u32, String>,
) -> Vec<LinkedCallGuardResultSource> {
    let mut sources = BTreeMap::<(&'static str, u32, bool), GuardResultSourceAccumulator>::new();
    for (value_bit, source) in value.bits().into_iter().enumerate() {
        let (kind, token, source_bit, inverted) = match source {
            BitSource::CallResult {
                call_token,
                bit,
                inverted,
            } => ("call-result", call_token, bit, inverted),
            BitSource::ExternalResult {
                call_token,
                bit,
                inverted,
            } => (
                "external-result",
                external_result_call_token(call_token),
                bit,
                inverted,
            ),
            _ => continue,
        };
        let entry = sources.entry((kind, token, inverted)).or_default();
        entry.value_bits |= 1_u32 << value_bit;
        entry.source_bits |= 1_u32 << source_bit;
        let Some(comparison_value) = comparison_value.filter(|_| {
            matches!(
                operation,
                BranchOperation::Equal | BranchOperation::NotEqual
            )
        }) else {
            continue;
        };
        let source_mask = 1_u32 << source_bit;
        let expected = (comparison_value & (1_u32 << value_bit) != 0) ^ inverted;
        if entry.comparison_known_bits & source_mask != 0 {
            let previous = entry.comparison_one_bits & source_mask != 0;
            entry.comparison_conflict |= previous != expected;
        } else {
            entry.comparison_known_bits |= source_mask;
            if expected {
                entry.comparison_one_bits |= source_mask;
            }
        }
    }
    sources
        .into_iter()
        .map(
            |((kind, token, inverted), evidence)| LinkedCallGuardResultSource {
                kind,
                token,
                target: call_results.get(&token).cloned(),
                operand,
                value_bits: Some(evidence.value_bits),
                source_bits: evidence.source_bits,
                inverted,
                comparison_value,
                source_comparison_value: (!evidence.comparison_conflict
                    && evidence.comparison_known_bits == evidence.source_bits)
                    .then_some(evidence.comparison_one_bits),
                producer_return_exact: None,
                mmio_sources: Vec::new(),
            },
        )
        .collect()
}

pub(super) fn guard_result_operand_sources(
    value: &SymbolicValue,
    operand: &'static str,
    comparison_value: Option<u32>,
    operation: BranchOperation,
    call_results: &BTreeMap<u32, String>,
) -> Vec<LinkedCallGuardResultSource> {
    let exact = exact_guard_result_operand_sources(
        value,
        operand,
        comparison_value,
        operation,
        call_results,
    );
    if !exact.is_empty() {
        return exact;
    }
    let mut fallback = BTreeMap::new();
    collect_guard_result_source_bits(value, &mut fallback);
    fallback
        .into_iter()
        .map(|((kind, token), source_bits)| LinkedCallGuardResultSource {
            kind,
            token,
            target: call_results.get(&token).cloned(),
            operand,
            value_bits: None,
            source_bits,
            inverted: false,
            comparison_value,
            source_comparison_value: None,
            producer_return_exact: None,
            mmio_sources: Vec::new(),
        })
        .collect()
}

pub(super) fn guard_result_sources(
    condition: &BranchCondition,
    call_results: &BTreeMap<u32, String>,
) -> Vec<LinkedCallGuardResultSource> {
    let mut sources = guard_result_operand_sources(
        &condition.left,
        "left",
        condition.right.as_constant(),
        condition.operation,
        call_results,
    );
    sources.extend(guard_result_operand_sources(
        &condition.right,
        "right",
        condition.left.as_constant(),
        condition.operation,
        call_results,
    ));
    sources.sort();
    sources.dedup();
    sources
}

#[derive(Default)]
pub(super) struct DirectMmioSourceAccumulator {
    value_bits: u32,
    register_bits: u32,
    comparison_known_bits: u32,
    comparison_one_bits: u32,
    comparison_conflict: bool,
}

pub(super) fn direct_mmio_operand_sources(
    value: &SymbolicValue,
    operand: &'static str,
    comparison_value: Option<u32>,
    operation: BranchOperation,
    svd: &MmioMap,
) -> Vec<LinkedDirectMmioPredicateSource> {
    let mut sources = BTreeMap::<(u32, u32, bool), DirectMmioSourceAccumulator>::new();
    for (value_bit, source) in value.bits().into_iter().enumerate() {
        let BitSource::Register {
            read_token,
            address,
            bit: register_bit,
            inverted,
        } = source
        else {
            continue;
        };
        let entry = sources.entry((read_token, address, inverted)).or_default();
        entry.value_bits |= 1_u32 << value_bit;
        entry.register_bits |= 1_u32 << register_bit;
        let Some(comparison_value) = comparison_value.filter(|_| {
            matches!(
                operation,
                BranchOperation::Equal | BranchOperation::NotEqual
            )
        }) else {
            continue;
        };
        let register_mask = 1_u32 << register_bit;
        let expected = (comparison_value & (1_u32 << value_bit) != 0) ^ inverted;
        if entry.comparison_known_bits & register_mask != 0 {
            let previous = entry.comparison_one_bits & register_mask != 0;
            entry.comparison_conflict |= previous != expected;
        } else {
            entry.comparison_known_bits |= register_mask;
            if expected {
                entry.comparison_one_bits |= register_mask;
            }
        }
    }
    sources
        .into_iter()
        .map(
            |((read_token, address, inverted), evidence)| LinkedDirectMmioPredicateSource {
                operand,
                read_token,
                address,
                register: svd.display_register_name(address),
                value_bits: evidence.value_bits,
                register_bits: evidence.register_bits,
                inverted,
                comparison_value,
                register_comparison_value: (!evidence.comparison_conflict
                    && evidence.comparison_known_bits == evidence.register_bits)
                    .then_some(evidence.comparison_one_bits),
            },
        )
        .collect()
}

pub(super) fn direct_mmio_predicate_sources(
    condition: &BranchCondition,
    svd: &MmioMap,
) -> Vec<LinkedDirectMmioPredicateSource> {
    let mut sources = direct_mmio_operand_sources(
        &condition.left,
        "left",
        condition.right.as_constant(),
        condition.operation,
        svd,
    );
    sources.extend(direct_mmio_operand_sources(
        &condition.right,
        "right",
        condition.left.as_constant(),
        condition.operation,
        svd,
    ));
    sources.sort();
    sources.dedup();
    sources
}

pub(super) fn call_result_identity(
    event: &DraftReferenceEvent,
    identities: &IrIdentityCatalog,
) -> Option<(u32, String)> {
    match event {
        DraftReferenceEvent::Call {
            token,
            site,
            target,
            ..
        }
        | DraftReferenceEvent::ScratchCall {
            token,
            site,
            target,
            ..
        }
        | DraftReferenceEvent::TailCall {
            token,
            site,
            target,
            ..
        } => Some((
            *token,
            format!("{}@{site:#010x}", identities.target(*target)),
        )),
        DraftReferenceEvent::ComposedCall { token, symbol, .. }
        | DraftReferenceEvent::ComposedCallWithScratch { token, symbol, .. } => {
            Some((*token, symbol.clone()))
        }
        DraftReferenceEvent::ReviewedExternalCall {
            token,
            site,
            candidates,
            ..
        } => Some((
            *token,
            format!(
                "{}@{site:#010x}",
                candidates
                    .iter()
                    .map(|candidate| format!("{}::{}", candidate.contract, candidate.name))
                    .collect::<Vec<_>>()
                    .join(" | ")
            ),
        )),
        _ => None,
    }
}

pub(super) fn name_call_results(expression: &str, call_results: &BTreeMap<u32, String>) -> String {
    let bytes = expression.as_bytes();
    let mut output = String::with_capacity(expression.len());
    let mut index = 0;
    while index < bytes.len() {
        let prefix_len = if bytes[index..].starts_with(b"external-result:") {
            Some(16)
        } else if bytes[index..].starts_with(b"call-result:") {
            Some(12)
        } else if bytes[index..].starts_with(b"external") {
            Some(8)
        } else if bytes[index..].starts_with(b"call") {
            Some(4)
        } else {
            None
        };
        if let Some(prefix_len) = prefix_len {
            let digits_start = index + prefix_len;
            let mut digits_end = digits_start;
            while digits_end < bytes.len() && bytes[digits_end].is_ascii_digit() {
                digits_end += 1;
            }
            if digits_end != digits_start
                && let Ok(token) = expression[digits_start..digits_end].parse::<u32>()
                && let Some(target) = call_results.get(&token)
            {
                output.push_str("result_of_");
                output.push_str(&pseudo_identifier(target));
                index = digits_end;
                continue;
            }
        }
        let character = expression[index..]
            .chars()
            .next()
            .expect("index is within the expression");
        output.push(character);
        index += character.len_utf8();
    }
    output
}

fn memory_root_name(root: &open_radio_vendor_analysis_model::MemoryObjectRoot) -> String {
    use open_radio_vendor_analysis_model::MemoryObjectRoot;
    match root {
        MemoryObjectRoot::Argument { index } => format!("arg{index}"),
        MemoryObjectRoot::RelocatedSymbol { member, symbol } => {
            format!("{}::{symbol}", member.as_deref().unwrap_or("linked"))
        }
        MemoryObjectRoot::Dereferenced {
            pointer,
            pointer_offset,
        } => format!("*({}{pointer_offset:+#x})", memory_root_name(pointer)),
        MemoryObjectRoot::Absolute { address } => format!("absolute:{address:#010x}"),
        MemoryObjectRoot::Indexed {
            root,
            argument,
            stride,
        } => format!("{}[arg{argument}*{stride:#x}]", memory_root_name(root)),
        MemoryObjectRoot::ZeroedAllocation { call_token } => {
            format!("zeroed-allocation:{call_token}")
        }
        MemoryObjectRoot::OpaqueExternalObject { call_token } => {
            format!("opaque-external-object:{call_token}")
        }
    }
}

fn name_memory_reads(
    expression: &str,
    read_sources: &BTreeMap<u32, open_radio_vendor_analysis_model::MemoryObjectLocation>,
    resolver: &ReferenceResolver,
) -> String {
    const PREFIX: &str = "ram:read";
    let mut output = String::with_capacity(expression.len());
    let mut remainder = expression;
    while let Some(start) = remainder.find(PREFIX) {
        output.push_str(&remainder[..start]);
        let token_start = start + PREFIX.len();
        let token_len = remainder[token_start..]
            .bytes()
            .take_while(u8::is_ascii_digit)
            .count();
        if token_len == 0 {
            output.push_str(PREFIX);
            remainder = &remainder[token_start..];
            continue;
        }
        let token_end = token_start + token_len;
        let token = remainder[token_start..token_end].parse::<u32>().ok();
        if let Some(location) = token.and_then(|token| read_sources.get(&token)) {
            output.push_str("memory:");
            let mut offset = location.offset;
            if let open_radio_vendor_analysis_model::MemoryObjectRoot::Absolute { address } =
                &location.root
                && let Ok(resolved_address) = u32::try_from(i64::from(*address) + location.offset)
                && let Some((member, symbol, symbol_offset)) =
                    resolver.data_symbol_location(resolved_address, 32)
            {
                output.push_str(member.unwrap_or("linked"));
                output.push_str("::");
                output.push_str(symbol);
                offset = symbol_offset;
            } else {
                output.push_str(&memory_root_name(&location.root));
            }
            if offset != 0 {
                output.push_str(&format!("{offset:+#x}"));
            }
        } else {
            output.push_str(&remainder[start..token_end]);
        }
        remainder = &remainder[token_end..];
    }
    output.push_str(remainder);
    output
}

pub(super) fn collect_guarded_direct_event(
    event: &DraftReferenceEvent,
    resolver: &ReferenceResolver,
    identities: &IrIdentityCatalog,
    svd: &MmioMap,
    read_sources: &BTreeMap<u32, open_radio_vendor_analysis_model::MemoryObjectLocation>,
    evidence: &mut DirectTraceEvidence,
) {
    if let DraftReferenceEvent::BranchDecision { condition, taken } = event {
        let rendered_condition =
            name_call_results(&branch_expression(condition), &evidence.call_results);
        let direct_mmio_sources = direct_mmio_predicate_sources(condition, svd);
        if !direct_mmio_sources.is_empty() {
            evidence
                .direct_mmio_predicates
                .insert(LinkedDirectMmioPredicate {
                    site: condition.site,
                    condition: rendered_condition.clone(),
                    operation: branch_operation(condition.operation),
                    sources: direct_mmio_sources.clone(),
                });
        }
        evidence.guards.insert(
            (condition.site, rendered_condition),
            DirectGuardEvidence {
                taken: *taken,
                operation: branch_operation(condition.operation),
                result_sources: guard_result_sources(condition, &evidence.call_results),
                direct_mmio_sources,
            },
        );
        return;
    }

    let mut event_calls = BTreeSet::new();
    collect_call_event(event, resolver, identities, &mut event_calls);
    for mut call in event_calls {
        for argument in &mut call.arguments {
            *argument = name_call_results(argument, &evidence.call_results);
            *argument = name_memory_reads(argument, read_sources, resolver);
        }
        for argument in &mut call.typed_arguments {
            argument.value = name_call_results(&argument.value, &evidence.call_results);
            argument.value = name_memory_reads(&argument.value, read_sources, resolver);
        }
        call.guard_paths = Some(vec![current_guard_path(&evidence.guards)]);
        evidence.calls.insert(call);
    }
    if let Some((token, target)) = call_result_identity(event, identities) {
        evidence.call_results.insert(token, target);
    }
}

pub(super) fn explore_direct_calls(
    symbol: &artifact::ArtifactSymbolDefinition,
    resolver: &ReferenceResolver,
    identities: &IrIdentityCatalog,
    svd: &MmioMap,
) -> DirectCallGraph {
    const MAX_BLOCKERS: usize = 64;
    const MAX_BLOCKER_CHARS: usize = 2_048;

    fn record_blocker(blockers: &mut BTreeSet<String>, message: impl Into<String>) {
        if blockers.len() >= MAX_BLOCKERS {
            blockers.insert(
                "additional call-graph diagnostics omitted by the linked-IR exploration budget"
                    .to_owned(),
            );
            return;
        }
        let message = message.into();
        let mut chars = message.chars();
        let bounded = chars.by_ref().take(MAX_BLOCKER_CHARS).collect::<String>();
        blockers.insert(if chars.next().is_some() {
            format!("{bounded}… [diagnostic truncated]")
        } else {
            bounded
        });
    }

    let mut result = DirectCallGraph::default();
    let program = match direct::StructuralProgram::decode(symbol) {
        Ok(program) => program,
        Err(error) => {
            record_blocker(&mut result.blockers, error.to_string());
            return result;
        }
    };
    let mut site_effect_counts = BTreeMap::<u32, usize>::new();
    let mut truncated_effect_sites = BTreeSet::<u32>::new();
    let summary = direct::explore_structural_program_bounded(
        symbol,
        &program,
        svd,
        &resolver.relocated_calls,
        &resolver.pointer_context,
        None,
        direct::StructuralTraceBudget {
            max_instruction_steps: MAX_CALL_GRAPH_INSTRUCTION_STEPS_PER_TRACE,
            max_events: MAX_CALL_GRAPH_EVENTS_PER_TRACE,
        },
        MAX_CALL_GRAPH_STATES,
        MAX_CALL_GRAPH_BRANCH_DECISIONS,
        |trace| {
            let trace = match trace {
                Ok(trace) => trace,
                Err(error) => {
                    record_blocker(&mut result.blockers, error.to_string());
                    return;
                }
            };
            for effect in instruction_effects_for_trace(&trace, resolver, &[], &[]) {
                record_site_effect(
                    &mut result,
                    &mut site_effect_counts,
                    &mut truncated_effect_sites,
                    effect,
                );
            }
            let mut evidence = DirectTraceEvidence::default();
            let read_sources = memory_read_sources_for_trace(&trace);
            for event in &trace.reference_events {
                collect_guarded_direct_event(
                    event,
                    resolver,
                    identities,
                    svd,
                    &read_sources,
                    &mut evidence,
                );
            }
            result.calls.append(&mut evidence.calls);
            result
                .direct_mmio_predicates
                .append(&mut evidence.direct_mmio_predicates);
            for relocation in symbol.relocations.iter().filter(|relocation| {
                matches!(
                    relocation.kind,
                    artifact::RelocationKind::Call | artifact::RelocationKind::CallPlt
                )
            }) {
                let unresolved = format!(
                    "unresolved-call-relocation at {:#x}: {}",
                    relocation.address, relocation.symbol
                );
                if trace
                    .reference_blockers
                    .iter()
                    .any(|blocker| blocker == &unresolved)
                {
                    result.calls.insert(LinkedCall {
                        kind: "unresolved",
                        target: relocation.symbol.clone(),
                        site: Some(relocation.address),
                        tail: artifact::relocated_call_is_tail(symbol, relocation.address)
                            .unwrap_or(false),
                        result_modeled: false,
                        execution_model: None,
                        semantics: Some(
                            "unresolved call relocation; arguments and callee effects are unavailable"
                                .to_owned(),
                        ),
                        semantic_operation: None,
                        semantic_contract: None,
                        replacement_hint: None,
                        project_symbol: Some(relocation.symbol.clone()),
                        project_candidates: Vec::new(),
                        trampoline: None,
                        argument_shapes: 1,
                        arguments: Vec::new(),
                        argument_bindings: Vec::new(),
                        typed_arguments: Vec::new(),
                        guard_paths: Some(vec![current_guard_path(&evidence.guards)]),
                    });
                }
            }
            for blocker in &trace.reference_blockers {
                record_blocker(&mut result.blockers, blocker);
            }
        },
    );
    for limit in summary.limits {
        let message = match limit {
            direct::StructuralExplorationLimit::States { maximum } => {
                format!("call graph exceeds the exploration limit of {maximum} states")
            }
            direct::StructuralExplorationLimit::BranchDecisions { site, maximum } => format!(
                "call graph exceeds the limit of {maximum} branch decisions per path at {site:#010x}"
            ),
            direct::StructuralExplorationLimit::RevisitedBranch { site } => {
                format!("call graph revisits branch {site:#010x}; that path is incomplete")
            }
        };
        record_blocker(&mut result.blockers, message);
    }

    result
}

pub(super) fn collect_calls_from_event(
    event: &DraftReferenceEvent,
    resolver: &ReferenceResolver,
    identities: &IrIdentityCatalog,
    calls: &mut BTreeSet<LinkedCall>,
) {
    collect_call_event(event, resolver, identities, calls);
    match event {
        DraftReferenceEvent::BoundedPoll {
            body, on_exhausted, ..
        } => {
            collect_calls_from_flow(body, resolver, identities, calls);
            if let Some(event) = on_exhausted.as_deref() {
                collect_calls_from_event(event, resolver, identities, calls);
            }
        }
        DraftReferenceEvent::PollFlow { body, .. } => {
            collect_calls_from_flow(body, resolver, identities, calls);
        }
        DraftReferenceEvent::SymmetricCalibrationSearch {
            initial_read,
            setup,
            write_candidate,
            sample,
            ..
        } => {
            for flow in [initial_read, setup, write_candidate, sample] {
                collect_calls_from_flow(flow, resolver, identities, calls);
            }
        }
        // A composed call's nested flow belongs to the callee. The caller edge
        // above is direct; recursively collecting it would create transitive
        // edges and obscure the actual call graph.
        _ => {}
    }
}

pub(super) fn collect_calls_from_flow(
    flow: &DraftReferenceFlow,
    resolver: &ReferenceResolver,
    identities: &IrIdentityCatalog,
    calls: &mut BTreeSet<LinkedCall>,
) {
    for event in &flow.events {
        collect_calls_from_event(event, resolver, identities, calls);
    }
    if let DraftReferenceTerminator::Branch {
        taken, not_taken, ..
    } = &flow.terminator
    {
        collect_calls_from_flow(taken, resolver, identities, calls);
        collect_calls_from_flow(not_taken, resolver, identities, calls);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use open_radio_vendor_analysis_model::{MemoryObjectLocation, MemoryObjectRoot};

    #[test]
    fn ram_read_argument_uses_the_containing_linked_data_symbol() {
        let resolver = ReferenceResolver {
            symbols: Vec::new(),
            symbols_by_address: BTreeMap::new(),
            symbol_ids: BTreeMap::new(),
            exported_symbol_keys: BTreeSet::new(),
            relocated_calls: direct::StructuralRelocatedCalls::new(),
            pointer_context: direct::StructuralPointerContext::default(),
            data_symbols: vec![artifact::ArtifactDataSymbolDefinition {
                member: None,
                name: "g_wifi_menuconfig".to_owned(),
                address: 0x1008_9a20,
                size: 0x80,
                exported: true,
            }],
            data_objects: Vec::new(),
            projected_direct_semantics: BTreeMap::new(),
            projected_origins: BTreeMap::new(),
        };
        let sources = BTreeMap::from([(
            7,
            MemoryObjectLocation {
                root: MemoryObjectRoot::Absolute {
                    address: 0x1008_9a54,
                },
                offset: 0,
            },
        )]);

        assert_eq!(
            name_memory_reads("ram:read7", &sources, &resolver),
            "memory:linked::g_wifi_menuconfig+0x34"
        );
    }
}
