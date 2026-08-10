//! Direct call-graph exploration and guarded MMIO provenance.

use super::*;

#[derive(Default)]
pub(super) struct DirectCallGraph {
    pub(super) calls: BTreeSet<LinkedCall>,
    pub(super) direct_mmio_predicates: BTreeSet<LinkedDirectMmioPredicate>,
    pub(super) blockers: BTreeSet<String>,
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
            } => ("external-result", call_token, bit),
            _ => continue,
        };
        *sources.entry((kind, token)).or_default() |= 1_u32 << bit;
        recovered_bits = true;
    }
    if recovered_bits {
        return;
    }
    match value {
        SymbolicValue::Expression { left, right, .. } => {
            collect_guard_result_source_bits(left, sources);
            collect_guard_result_source_bits(right, sources);
        }
        SymbolicValue::WideSignedDivide {
            dividend_low,
            dividend_high,
            divisor_low,
            divisor_high,
            ..
        } => {
            for value in [dividend_low, dividend_high, divisor_low, divisor_high] {
                collect_guard_result_source_bits(value, sources);
            }
        }
        _ => {}
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
            } => ("external-result", call_token, bit, inverted),
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
        DraftReferenceEvent::Call { token, target, .. }
        | DraftReferenceEvent::ScratchCall { token, target, .. }
        | DraftReferenceEvent::TailCall { token, target, .. } => {
            Some((*token, identities.target(*target)))
        }
        DraftReferenceEvent::ComposedCall { token, symbol, .. }
        | DraftReferenceEvent::ComposedCallWithScratch { token, symbol, .. } => {
            Some((*token, symbol.clone()))
        }
        DraftReferenceEvent::ReviewedExternalCall {
            token, candidates, ..
        } => Some((
            *token,
            candidates
                .iter()
                .map(|candidate| format!("{}::{}", candidate.contract, candidate.name))
                .collect::<Vec<_>>()
                .join(" | "),
        )),
        _ => None,
    }
}

pub(super) fn name_call_results(expression: &str, call_results: &BTreeMap<u32, String>) -> String {
    let bytes = expression.as_bytes();
    let mut output = String::with_capacity(expression.len());
    let mut index = 0;
    while index < bytes.len() {
        let prefix_len = if bytes[index..].starts_with(b"external") {
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
                output.push('_');
                output.push_str(&token.to_string());
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

pub(super) fn collect_guarded_direct_event(
    event: &DraftReferenceEvent,
    resolver: &ReferenceResolver,
    identities: &IrIdentityCatalog,
    svd: &MmioMap,
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
    let mut queue = VecDeque::from([BTreeMap::<u32, bool>::new()]);
    let mut queued = BTreeSet::from([BTreeMap::<u32, bool>::new()]);
    let mut explored_states = 0usize;

    while let Some(forced_branches) = queue.pop_front() {
        if explored_states >= MAX_CALL_GRAPH_STATES {
            record_blocker(
                &mut result.blockers,
                format!(
                    "call graph exceeds the exploration limit of {MAX_CALL_GRAPH_STATES} states"
                ),
            );
            break;
        }
        explored_states += 1;
        let trace = match direct::trace_structural_program_with_branches_bounded(
            symbol,
            &program,
            svd,
            &resolver.relocated_calls,
            &resolver.pointer_context,
            None,
            &forced_branches,
            direct::StructuralTraceBudget {
                max_instruction_steps: MAX_CALL_GRAPH_INSTRUCTION_STEPS_PER_TRACE,
                max_events: MAX_CALL_GRAPH_EVENTS_PER_TRACE,
            },
        ) {
            Ok(trace) => trace,
            Err(error) => {
                record_blocker(&mut result.blockers, error.to_string());
                continue;
            }
        };
        let mut evidence = DirectTraceEvidence::default();
        for event in &trace.reference_events {
            collect_guarded_direct_event(event, resolver, identities, svd, &mut evidence);
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

        let Some(branch) = trace.unresolved_branch else {
            continue;
        };
        if forced_branches.len() >= MAX_CALL_GRAPH_BRANCH_DECISIONS {
            record_blocker(
                &mut result.blockers,
                format!(
                    "call graph exceeds the limit of {MAX_CALL_GRAPH_BRANCH_DECISIONS} branch decisions per path at {:#010x}",
                    branch.site
                ),
            );
            continue;
        }
        for taken in [false, true] {
            let mut next = forced_branches.clone();
            if next.insert(branch.site, taken).is_some() {
                record_blocker(
                    &mut result.blockers,
                    format!(
                        "call graph revisits branch {:#010x}; that path is incomplete",
                        branch.site
                    ),
                );
            } else if queued.insert(next.clone()) {
                queue.push_back(next);
            }
        }
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
