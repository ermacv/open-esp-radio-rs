//! Construction of the MMIO register and candidate-field catalog.

use super::super::*;
use super::evidence::*;

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

struct SemanticActionLink<'a> {
    root: &'a str,
    operation: &'a str,
    target: &'a str,
    origin: &'a str,
    site: Option<u32>,
    site_path: &'a [Option<u32>],
    path: &'a str,
    scopes: &'a [LinkedCallGuardScope],
}

fn collect_semantic_field_links(
    output: &mut BTreeSet<SemanticFieldLink>,
    action: SemanticActionLink<'_>,
) {
    for (scope_index, scope) in action.scopes.iter().enumerate() {
        for (path_index, path) in scope.paths.iter().enumerate() {
            let path_expression = format_guard_path(path);
            for (guard_index, guard) in path.guards.iter().enumerate() {
                let residual_path_expression = format_guard_path_without(path, guard_index);
                let link =
                    |kind,
                     address,
                     register_bits,
                     producer: Option<String>,
                     producer_path: Vec<String>| SemanticFieldLink {
                        kind,
                        address,
                        register_bits,
                        root: action.root.to_owned(),
                        operation: action.operation.to_owned(),
                        action_target: action.target.to_owned(),
                        action_origin: action.origin.to_owned(),
                        action_site: action.site,
                        action_site_path: action.site_path.to_vec(),
                        action_path: action.path.to_owned(),
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
                    };
                for mmio in &guard.direct_mmio_sources {
                    output.insert(link(
                        "direct-mmio",
                        mmio.address,
                        mmio.register_bits,
                        Some(scope.function.clone()),
                        vec![scope.function.clone()],
                    ));
                }
                for source in &guard.result_sources {
                    for mmio in &source.mmio_sources {
                        output.insert(link(
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

pub(super) fn build_mmio_registers(functions: &[LinkedIrFunction]) -> Vec<LinkedMmioRegister> {
    let mut mmio_index = BTreeMap::<(u32, u8), MmioRegisterAccumulator>::new();
    for function in functions {
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
    for function in functions {
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
    for function in functions {
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
    for function in functions {
        for action in &function.effect_summary.register_semantic_actions {
            let Some(scopes) = action.guard_scopes.as_deref() else {
                continue;
            };
            collect_semantic_field_links(
                &mut semantic_evidence,
                SemanticActionLink {
                    root: &function.identity,
                    operation: &action.operation,
                    target: &action.target,
                    origin: &action.origin,
                    site: action.site,
                    site_path: &action.site_path,
                    path: &action.path,
                    scopes,
                },
            );
        }
        for call in function
            .calls
            .iter()
            .filter(|call| call.semantic_operation.is_some())
        {
            let Some(paths) = call.guard_paths.as_deref() else {
                continue;
            };
            if paths.iter().any(|path| path.guards.is_empty()) {
                continue;
            }
            let scopes = [LinkedCallGuardScope {
                function: function.identity.clone(),
                paths: paths.to_vec(),
            }];
            let site_path = [call.site];
            let site = call
                .site
                .map_or_else(|| "composed".to_owned(), |site| format!("{site:#010x}"));
            let path = format!(
                "{} --semantic@{}--> {}",
                function.identity, site, call.target
            );
            collect_semantic_field_links(
                &mut semantic_evidence,
                SemanticActionLink {
                    root: &function.identity,
                    operation: call
                        .semantic_operation
                        .as_deref()
                        .expect("filtered semantic call"),
                    target: &call.target,
                    origin: &function.identity,
                    site: call.site,
                    site_path: &site_path,
                    path: &path,
                    scopes: &scopes,
                },
            );
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
    mmio_index
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
        .collect()
}
