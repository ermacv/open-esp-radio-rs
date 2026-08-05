//! MMIO register and candidate-field index construction.

use super::*;

#[derive(Default)]
struct MmioFieldCandidateAccumulator {
    write_shapes: usize,
    predicate_shapes: usize,
    poll_shapes: usize,
    functions: BTreeSet<String>,
    access_functions: BTreeSet<String>,
    predicate_functions: BTreeSet<String>,
    predicate_evidence: BTreeSet<LinkedMmioFieldPredicateEvidence>,
    semantic_operations: BTreeSet<String>,
    semantic_roots: BTreeSet<String>,
    semantic_evidence: BTreeSet<LinkedMmioFieldSemanticEvidence>,
}

#[derive(Default)]
struct MmioRegisterAccumulator {
    names: BTreeSet<String>,
    read_shapes: usize,
    write_shapes: usize,
    poll_shapes: usize,
    predicate_shapes: usize,
    static_shapes: usize,
    indexed_candidate_shapes: usize,
    whole_register_write_shapes: usize,
    whole_register_predicate_shapes: usize,
    whole_register_poll_shapes: usize,
    read_modify_write_shapes: usize,
    write_masks: BTreeSet<u32>,
    predicate_masks: BTreeSet<u32>,
    poll_masks: BTreeSet<u32>,
    candidate_bit_ranges: BTreeMap<(u8, u8, u32), (usize, BTreeSet<String>)>,
    field_candidates: BTreeMap<(u8, u8, u32), MmioFieldCandidateAccumulator>,
    functions: BTreeSet<String>,
}

fn candidate_bit_ranges(mask: u32, width: u8) -> Vec<(u8, u8, u32)> {
    let mask = mask & width_mask(width);
    let mut output = Vec::new();
    let mut bit = 0_u8;
    while bit < width {
        if mask & (1_u32 << bit) == 0 {
            bit += 1;
            continue;
        }
        let first = bit;
        while bit + 1 < width && mask & (1_u32 << (bit + 1)) != 0 {
            bit += 1;
        }
        let last = bit;
        let range_width = last - first + 1;
        let range_mask = if range_width == 32 {
            u32::MAX
        } else {
            ((1_u32 << range_width) - 1) << first
        };
        output.push((first, last, range_mask));
        bit += 1;
    }
    output
}

fn record_access_field_mask(
    entry: &mut MmioRegisterAccumulator,
    mask: u32,
    width: u8,
    function: &str,
    access: &'static str,
    evidence: Option<LinkedMmioFieldPredicateEvidence>,
) {
    let full_mask = width_mask(width);
    let mask = mask & full_mask;
    if mask == 0 || mask == full_mask {
        return;
    }
    for range in candidate_bit_ranges(mask, width) {
        let candidate = entry.field_candidates.entry(range).or_default();
        match access {
            "write" => candidate.write_shapes += 1,
            "poll" => candidate.poll_shapes += 1,
            _ => unreachable!("field access evidence has a closed vocabulary"),
        }
        candidate.functions.insert(function.to_owned());
        candidate.access_functions.insert(function.to_owned());
        if let Some(mut evidence) = evidence.clone() {
            evidence.register_comparison_value = evidence
                .register_comparison_value
                .map(|value| value & range.2);
            candidate.predicate_functions.insert(function.to_owned());
            candidate.predicate_evidence.insert(evidence);
        }
    }
}

fn record_predicate_field_mask(
    entry: &mut MmioRegisterAccumulator,
    mask: u32,
    width: u8,
    predicate_function: &str,
    evidence: &[LinkedMmioFieldPredicateEvidence],
) {
    let full_mask = width_mask(width);
    let mask = mask & full_mask;
    if mask == 0 || mask == full_mask {
        return;
    }
    for range in candidate_bit_ranges(mask, width) {
        let candidate = entry.field_candidates.entry(range).or_default();
        candidate.predicate_shapes += 1;
        candidate.functions.insert(predicate_function.to_owned());
        candidate
            .predicate_functions
            .insert(predicate_function.to_owned());
        for evidence in evidence {
            let mut evidence = evidence.clone();
            evidence.register_comparison_value = evidence
                .register_comparison_value
                .map(|value| value & range.2);
            candidate
                .functions
                .extend(evidence.producer_path.iter().cloned());
            if let Some(producer) = evidence.producer_path.last() {
                candidate.access_functions.insert(producer.clone());
            }
            candidate.predicate_evidence.insert(evidence);
        }
    }
}

struct SemanticFieldEvidence<'a> {
    kind: &'static str,
    mask: u32,
    width: u8,
    operation: &'a str,
    root: &'a str,
    action_target: &'a str,
    action_origin: &'a str,
    action_site: Option<u32>,
    action_site_path: &'a [Option<u32>],
    action_path: &'a str,
    predicate_function: &'a str,
    producer: Option<&'a str>,
    producer_path: &'a [String],
    scope_index: usize,
    scope_alternatives: usize,
    path_index: usize,
    path_expression: &'a str,
    path_guards: usize,
    guard_index: usize,
    residual_path_expression: &'a str,
    site: u32,
    condition: &'a str,
    taken: bool,
    guard_operation: &'static str,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct SemanticFieldLink {
    kind: &'static str,
    address: u32,
    register_bits: u32,
    root: String,
    operation: String,
    action_target: String,
    action_origin: String,
    action_site: Option<u32>,
    action_site_path: Vec<Option<u32>>,
    action_path: String,
    predicate_function: String,
    producer: Option<String>,
    producer_path: Vec<String>,
    scope_index: usize,
    scope_alternatives: usize,
    path_index: usize,
    path_expression: String,
    path_guards: usize,
    guard_index: usize,
    residual_path_expression: String,
    site: u32,
    condition: String,
    guard_operation: &'static str,
    taken: bool,
}

fn record_semantic_field_link(
    entry: &mut MmioRegisterAccumulator,
    evidence: SemanticFieldEvidence<'_>,
) {
    let full_mask = width_mask(evidence.width);
    let mask = evidence.mask & full_mask;
    if mask == 0 || mask == full_mask {
        return;
    }
    for range in candidate_bit_ranges(mask, evidence.width) {
        let candidate = entry.field_candidates.entry(range).or_default();
        candidate
            .semantic_operations
            .insert(evidence.operation.to_owned());
        candidate.semantic_roots.insert(evidence.root.to_owned());
        candidate
            .semantic_evidence
            .insert(LinkedMmioFieldSemanticEvidence {
                kind: evidence.kind,
                root: evidence.root.to_owned(),
                operation: evidence.operation.to_owned(),
                action_target: evidence.action_target.to_owned(),
                action_origin: evidence.action_origin.to_owned(),
                action_site: evidence.action_site,
                action_site_path: evidence.action_site_path.to_vec(),
                action_path: evidence.action_path.to_owned(),
                predicate_function: evidence.predicate_function.to_owned(),
                producer: evidence.producer.map(str::to_owned),
                producer_path: evidence.producer_path.to_vec(),
                scope_index: evidence.scope_index,
                scope_alternatives: evidence.scope_alternatives,
                path_index: evidence.path_index,
                path_expression: evidence.path_expression.to_owned(),
                path_guards: evidence.path_guards,
                guard_index: evidence.guard_index,
                residual_path_expression: evidence.residual_path_expression.to_owned(),
                site: evidence.site,
                condition: evidence.condition.to_owned(),
                taken: evidence.taken,
                effective_operation: effective_branch_operation(
                    evidence.guard_operation,
                    evidence.taken,
                ),
            });
        candidate
            .predicate_functions
            .insert(evidence.predicate_function.to_owned());
        candidate
            .functions
            .insert(evidence.predicate_function.to_owned());
        candidate
            .functions
            .extend(evidence.producer_path.iter().cloned());
        if let Some(producer) = evidence.producer_path.last() {
            candidate.access_functions.insert(producer.clone());
        }
    }
}

fn unique_mmio_widths(
    index: &BTreeMap<(u32, u8), MmioRegisterAccumulator>,
) -> BTreeMap<u32, Option<u8>> {
    let mut widths = BTreeMap::new();
    for &(address, width) in index.keys() {
        widths
            .entry(address)
            .and_modify(|known| {
                if *known != Some(width) {
                    *known = None;
                }
            })
            .or_insert(Some(width));
    }
    widths
}

pub(super) fn summarize_linked_ir(mut functions: Vec<LinkedIrFunction>) -> LinkedIrReport {
    functions.sort_by(|left, right| left.identity.cmp(&right.identity));
    link_guard_result_mmio_sources(&mut functions);
    populate_effect_summaries(&mut functions);
    let mmio_functions = functions
        .iter()
        .filter(|function| !function.mmio_accesses.is_empty())
        .count();
    let mmio_access_shapes = functions
        .iter()
        .map(|function| function.mmio_accesses.len())
        .sum();
    let delay_functions = functions
        .iter()
        .filter(|function| !function.delays.is_empty())
        .count();
    let delay_shapes = functions.iter().map(|function| function.delays.len()).sum();
    let mut mmio_index = BTreeMap::<(u32, u8), MmioRegisterAccumulator>::new();
    for function in &functions {
        for access in &function.mmio_accesses {
            let entry = mmio_index
                .entry((access.address, access.width))
                .or_default();
            entry.names.insert(access.register.clone());
            match access.access {
                "read" => entry.read_shapes += 1,
                "write" => {
                    entry.write_shapes += 1;
                    let modified_mask = access.modified_mask.unwrap_or(width_mask(access.width));
                    entry.write_masks.insert(modified_mask);
                    entry.whole_register_write_shapes +=
                        usize::from(modified_mask == width_mask(access.width));
                    let register_derived_mask = access.preserved_mask.unwrap_or_default()
                        | access.inverted_mask.unwrap_or_default()
                        | access.read_derived_mask.unwrap_or_default();
                    entry.read_modify_write_shapes += usize::from(register_derived_mask != 0);
                    for range in candidate_bit_ranges(modified_mask, access.width) {
                        let candidate = entry.candidate_bit_ranges.entry(range).or_default();
                        candidate.0 += 1;
                        candidate.1.insert(function.identity.clone());
                    }
                    record_access_field_mask(
                        entry,
                        modified_mask,
                        access.width,
                        &function.identity,
                        "write",
                        None,
                    );
                }
                "poll" => {
                    entry.poll_shapes += 1;
                    let predicate_mask = access
                        .predicate_mask
                        .expect("poll MMIO access has a structured predicate mask")
                        & width_mask(access.width);
                    entry.poll_masks.insert(predicate_mask);
                    entry.whole_register_poll_shapes +=
                        usize::from(predicate_mask == width_mask(access.width));
                    record_access_field_mask(
                        entry,
                        predicate_mask,
                        access.width,
                        &function.identity,
                        "poll",
                        Some(LinkedMmioFieldPredicateEvidence {
                            kind: "poll",
                            function: function.identity.clone(),
                            producer: None,
                            producer_path: Vec::new(),
                            site: None,
                            path: Some(access.path.clone()),
                            condition: access
                                .guard
                                .clone()
                                .expect("poll MMIO access has a predicate expression"),
                            operation: "equal",
                            taken: None,
                            effective_operation: None,
                            operand: Some("read"),
                            comparison_value: access.predicate_expected,
                            register_comparison_value: access.predicate_expected,
                            inverted: false,
                        }),
                    );
                }
                _ => unreachable!("linked MMIO access has a closed access vocabulary"),
            }
            match access.mode {
                "static" => entry.static_shapes += 1,
                "indexed-candidate" => entry.indexed_candidate_shapes += 1,
                _ => unreachable!("linked MMIO access has a closed address-mode vocabulary"),
            }
            entry.functions.insert(function.identity.clone());
        }
    }
    let unique_widths = unique_mmio_widths(&mmio_index);
    for function in &functions {
        for predicate in &function.direct_mmio_predicates {
            for source in &predicate.sources {
                let Some(width) = unique_widths.get(&source.address).copied().flatten() else {
                    continue;
                };
                let entry = mmio_index
                    .get_mut(&(source.address, width))
                    .expect("unique MMIO width comes from the register index");
                let predicate_mask = source.register_bits & width_mask(width);
                entry.predicate_shapes += 1;
                entry.predicate_masks.insert(predicate_mask);
                entry.whole_register_predicate_shapes +=
                    usize::from(predicate_mask == width_mask(width));
                entry.functions.insert(function.identity.clone());
                record_predicate_field_mask(
                    entry,
                    predicate_mask,
                    width,
                    &function.identity,
                    &[LinkedMmioFieldPredicateEvidence {
                        kind: "direct-mmio",
                        function: function.identity.clone(),
                        producer: None,
                        producer_path: vec![function.identity.clone()],
                        site: Some(predicate.site),
                        path: None,
                        condition: predicate.condition.clone(),
                        operation: predicate.operation,
                        taken: None,
                        effective_operation: None,
                        operand: Some(source.operand),
                        comparison_value: source.comparison_value,
                        register_comparison_value: source.register_comparison_value,
                        inverted: source.inverted,
                    }],
                );
            }
        }
    }
    let mut predicate_evidence = BTreeMap::<
        (String, u32, String, Vec<String>, u32, String, u32, u32),
        BTreeSet<LinkedMmioFieldPredicateEvidence>,
    >::new();
    for function in &functions {
        for call in &function.calls {
            let Some(paths) = call.guard_paths.as_deref() else {
                continue;
            };
            for path in paths {
                for guard in &path.guards {
                    for source in &guard.result_sources {
                        let producer = source
                            .target
                            .clone()
                            .unwrap_or_else(|| "unknown".to_owned());
                        for mmio in &source.mmio_sources {
                            predicate_evidence
                                .entry((
                                    function.identity.clone(),
                                    guard.site,
                                    producer.clone(),
                                    mmio.producer_path.clone(),
                                    mmio.address,
                                    mmio.register.clone(),
                                    mmio.result_bits,
                                    mmio.register_bits,
                                ))
                                .or_default()
                                .insert(LinkedMmioFieldPredicateEvidence {
                                    kind: "producer-return",
                                    function: function.identity.clone(),
                                    producer: (producer != "unknown").then_some(producer.clone()),
                                    producer_path: mmio.producer_path.clone(),
                                    site: Some(guard.site),
                                    path: None,
                                    condition: guard.condition.clone(),
                                    operation: guard.operation,
                                    taken: Some(guard.taken),
                                    effective_operation: Some(effective_branch_operation(
                                        guard.operation,
                                        guard.taken,
                                    )),
                                    operand: Some(source.operand),
                                    comparison_value: source.comparison_value,
                                    register_comparison_value: mmio.register_comparison_value,
                                    inverted: mmio.inverted,
                                });
                        }
                    }
                }
            }
        }
    }
    for (
        (
            predicate_function,
            _site,
            _producer,
            producer_path,
            address,
            _register,
            _result_bits,
            register_bits,
        ),
        evidence,
    ) in predicate_evidence
    {
        let Some(width) = unique_widths.get(&address).copied().flatten() else {
            continue;
        };
        let entry = mmio_index
            .get_mut(&(address, width))
            .expect("unique MMIO width comes from the register index");
        let predicate_mask = register_bits & width_mask(width);
        entry.predicate_shapes += 1;
        entry.predicate_masks.insert(predicate_mask);
        entry.whole_register_predicate_shapes += usize::from(predicate_mask == width_mask(width));
        entry.functions.insert(predicate_function.clone());
        entry.functions.extend(producer_path);
        record_predicate_field_mask(
            entry,
            predicate_mask,
            width,
            &predicate_function,
            &evidence.into_iter().collect::<Vec<_>>(),
        );
    }
    let mut semantic_evidence = BTreeSet::<SemanticFieldLink>::new();
    for function in &functions {
        for action in &function.effect_summary.semantic_actions {
            let Some(scopes) = action.guard_scopes.as_deref() else {
                continue;
            };
            for (scope_index, scope) in scopes.iter().enumerate() {
                for (path_index, path) in scope.paths.iter().enumerate() {
                    let path_expression = format_guard_path(path);
                    for (guard_index, guard) in path.guards.iter().enumerate() {
                        let residual_path_expression = format_guard_path_without(path, guard_index);
                        let link = |kind,
                                    address,
                                    register_bits,
                                    producer: Option<String>,
                                    producer_path: Vec<String>| {
                            SemanticFieldLink {
                                kind,
                                address,
                                register_bits,
                                root: function.identity.clone(),
                                operation: action.operation.clone(),
                                action_target: action.target.clone(),
                                action_origin: action.origin.clone(),
                                action_site: action.site,
                                action_site_path: action.site_path.clone(),
                                action_path: action.path.clone(),
                                predicate_function: scope.function.clone(),
                                producer,
                                producer_path,
                                scope_index,
                                scope_alternatives: scope.paths.len(),
                                path_index,
                                path_expression: path_expression.clone(),
                                path_guards: path.guards.len(),
                                guard_index,
                                residual_path_expression: residual_path_expression.clone(),
                                site: guard.site,
                                condition: guard.condition.clone(),
                                guard_operation: guard.operation,
                                taken: guard.taken,
                            }
                        };
                        for mmio in &guard.direct_mmio_sources {
                            semantic_evidence.insert(link(
                                "direct-mmio",
                                mmio.address,
                                mmio.register_bits,
                                Some(scope.function.clone()),
                                vec![scope.function.clone()],
                            ));
                        }
                        for source in &guard.result_sources {
                            for mmio in &source.mmio_sources {
                                semantic_evidence.insert(link(
                                    "producer-return",
                                    mmio.address,
                                    mmio.register_bits,
                                    source.target.clone(),
                                    mmio.producer_path.clone(),
                                ));
                            }
                        }
                    }
                }
            }
        }
    }
    for link in semantic_evidence {
        let Some(width) = unique_widths.get(&link.address).copied().flatten() else {
            continue;
        };
        let entry = mmio_index
            .get_mut(&(link.address, width))
            .expect("unique MMIO width comes from the register index");
        record_semantic_field_link(
            entry,
            SemanticFieldEvidence {
                kind: link.kind,
                mask: link.register_bits,
                width,
                operation: &link.operation,
                root: &link.root,
                action_target: &link.action_target,
                action_origin: &link.action_origin,
                action_site: link.action_site,
                action_site_path: &link.action_site_path,
                action_path: &link.action_path,
                predicate_function: &link.predicate_function,
                producer: link.producer.as_deref(),
                producer_path: &link.producer_path,
                scope_index: link.scope_index,
                scope_alternatives: link.scope_alternatives,
                path_index: link.path_index,
                path_expression: &link.path_expression,
                path_guards: link.path_guards,
                guard_index: link.guard_index,
                residual_path_expression: &link.residual_path_expression,
                site: link.site,
                condition: &link.condition,
                taken: link.taken,
                guard_operation: link.guard_operation,
            },
        );
    }
    let mmio_registers = mmio_index
        .into_iter()
        .map(|((address, width), entry)| LinkedMmioRegister {
            address,
            width,
            names: entry.names.into_iter().collect(),
            read_shapes: entry.read_shapes,
            write_shapes: entry.write_shapes,
            poll_shapes: entry.poll_shapes,
            predicate_shapes: entry.predicate_shapes,
            static_shapes: entry.static_shapes,
            indexed_candidate_shapes: entry.indexed_candidate_shapes,
            whole_register_write_shapes: entry.whole_register_write_shapes,
            whole_register_predicate_shapes: entry.whole_register_predicate_shapes,
            whole_register_poll_shapes: entry.whole_register_poll_shapes,
            read_modify_write_shapes: entry.read_modify_write_shapes,
            write_masks: entry.write_masks.into_iter().collect(),
            predicate_masks: entry.predicate_masks.into_iter().collect(),
            poll_masks: entry.poll_masks.into_iter().collect(),
            candidate_bit_ranges: entry
                .candidate_bit_ranges
                .into_iter()
                .map(
                    |(
                        (least_significant_bit, most_significant_bit, mask),
                        (write_shapes, functions),
                    )| LinkedMmioBitRange {
                        least_significant_bit,
                        most_significant_bit,
                        mask,
                        write_shapes,
                        functions: functions.into_iter().collect(),
                    },
                )
                .collect(),
            field_candidates: entry
                .field_candidates
                .into_iter()
                .map(
                    |((least_significant_bit, most_significant_bit, mask), candidate)| {
                        LinkedMmioFieldCandidate {
                            least_significant_bit,
                            most_significant_bit,
                            mask,
                            write_shapes: candidate.write_shapes,
                            predicate_shapes: candidate.predicate_shapes,
                            poll_shapes: candidate.poll_shapes,
                            functions: candidate.functions.into_iter().collect(),
                            access_functions: candidate.access_functions.into_iter().collect(),
                            predicate_functions: candidate
                                .predicate_functions
                                .into_iter()
                                .collect(),
                            predicate_evidence: candidate.predicate_evidence.into_iter().collect(),
                            semantic_operations: candidate
                                .semantic_operations
                                .into_iter()
                                .collect(),
                            semantic_roots: candidate.semantic_roots.into_iter().collect(),
                            semantic_evidence: candidate.semantic_evidence.into_iter().collect(),
                        }
                    },
                )
                .collect(),
            functions: entry.functions.into_iter().collect(),
        })
        .collect();
    let exported_functions = functions
        .iter()
        .filter(|function| function.binding == "global-or-weak")
        .count();
    let local_functions = functions
        .iter()
        .filter(|function| function.binding == "local")
        .count();
    let context_functions = functions
        .iter()
        .filter(|function| !function.context_accesses.is_empty())
        .count();
    let context_accesses = functions
        .iter()
        .map(|function| function.context_accesses.len())
        .sum();
    let context_fields = functions
        .iter()
        .map(|function| function.context_fields.len())
        .sum();
    let complete_functions = functions
        .iter()
        .filter(|function| function.complete)
        .count();
    let structured_functions = functions
        .iter()
        .filter(|function| function.flow_kind == "structured")
        .count();
    let internal_calls = functions
        .iter()
        .flat_map(|function| &function.calls)
        .filter(|call| call.kind == "internal")
        .count();
    let external_calls = functions
        .iter()
        .flat_map(|function| &function.calls)
        .filter(|call| matches!(call.kind, "external" | "diagnostic"))
        .count();
    let call_argument_shapes = functions
        .iter()
        .flat_map(|function| &function.calls)
        .map(|call| call.argument_shapes)
        .sum();
    let project_linked_calls = functions
        .iter()
        .flat_map(|function| &function.calls)
        .filter(|call| call.kind == "project-linked")
        .count();
    let ambiguous_project_calls = functions
        .iter()
        .flat_map(|function| &function.calls)
        .filter(|call| call.kind == "unresolved" && call.project_candidates.len() > 1)
        .count();
    let unresolved_calls = functions
        .iter()
        .flat_map(|function| &function.calls)
        .filter(|call| call.kind == "unresolved")
        .count();
    let closed_effect_summaries = functions
        .iter()
        .filter(|function| function.effect_summary.call_graph_closed)
        .count();
    let recursive_effect_summaries = functions
        .iter()
        .filter(|function| !function.effect_summary.recursive_functions.is_empty())
        .count();
    let complete_context_projections = functions
        .iter()
        .filter(|function| function.effect_summary.context_projection_complete)
        .count();
    let projected_context_fields = functions
        .iter()
        .map(|function| function.effect_summary.context_fields.len())
        .sum();
    let semantic_calls = functions
        .iter()
        .flat_map(|function| &function.calls)
        .filter(|call| call.semantic_operation.is_some())
        .map(|call| call.argument_shapes)
        .sum();
    let mut semantic_index =
        BTreeMap::<String, (usize, BTreeSet<String>, BTreeSet<String>, BTreeSet<String>)>::new();
    for function in &functions {
        for call in &function.calls {
            let Some(operation) = call.semantic_operation.as_ref() else {
                continue;
            };
            let entry = semantic_index.entry(operation.clone()).or_default();
            entry.0 += call.argument_shapes;
            entry.1.insert(function.identity.clone());
            entry.2.insert(call.target.clone());
            if let Some(replacement) = call.replacement_hint.as_ref() {
                entry.3.insert(replacement.clone());
            }
        }
    }
    let semantic_boundaries = semantic_index
        .into_iter()
        .map(
            |(operation, (call_shapes, functions, targets, replacement_hints))| SemanticBoundary {
                operation,
                call_shapes,
                functions: functions.into_iter().collect(),
                targets: targets.into_iter().collect(),
                replacement_hints: replacement_hints.into_iter().collect(),
            },
        )
        .collect();
    let mut trampoline_index =
        BTreeMap::<LinkedTrampoline, (Vec<LinkedCallArgument>, usize, BTreeSet<String>)>::new();
    for function in &functions {
        for call in &function.calls {
            let Some(trampoline) = call.trampoline.as_ref() else {
                continue;
            };
            let abi_arguments = call
                .typed_arguments
                .iter()
                .cloned()
                .map(|mut argument| {
                    argument.value.clear();
                    argument
                })
                .collect::<Vec<_>>();
            let entry = trampoline_index
                .entry(trampoline.clone())
                .or_insert_with(|| (abi_arguments, 0, BTreeSet::new()));
            entry.1 += call.argument_shapes;
            entry.2.insert(function.identity.clone());
        }
    }
    let trampoline_calls = trampoline_index.values().map(|entry| entry.1).sum();
    let trampoline_slots = trampoline_index
        .into_iter()
        .map(
            |(trampoline, (arguments, call_shapes, functions))| LinkedTrampolineSlot {
                trampoline,
                arguments,
                call_shapes,
                functions: functions.into_iter().collect(),
            },
        )
        .collect();

    LinkedIrReport {
        functions,
        mmio_registers,
        mmio_functions,
        mmio_access_shapes,
        delay_functions,
        delay_shapes,
        semantic_boundaries,
        semantic_calls,
        trampoline_slots,
        trampoline_calls,
        exported_functions,
        local_functions,
        context_functions,
        context_accesses,
        context_fields,
        complete_functions,
        structured_functions,
        internal_calls,
        external_calls,
        call_argument_shapes,
        project_linked_calls,
        ambiguous_project_calls,
        unresolved_calls,
        closed_effect_summaries,
        recursive_effect_summaries,
        complete_context_projections,
        projected_context_fields,
    }
}
