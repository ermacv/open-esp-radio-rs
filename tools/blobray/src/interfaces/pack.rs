//! Validation and evidence matching for reviewed interface packs.

use std::collections::{BTreeMap, BTreeSet};

use super::validation::{ValidationError, ValidationResult};
use super::{
    InterfaceAnchor, InterfaceFactRoot, InterfaceFacts, InterfaceGuard, InterfacePack,
    InterfaceRootSelector, InterfaceWorkspaceSummary, PackOrigin,
    ResolvedExternalCallExecutionModel, ResolvedInterfaceContract,
    ResolvedInterfaceExecutionContract, ResolvedInterfaceSlot, ResolvedSemanticAnnotation,
    ReviewStatus, SemanticCatalogs, UnreviewedInterfaceObservation, validate_dotted_id,
};
use crate::{ExternalCallModelSetRef, KnowledgeContractSpec};

type ValidatedInterfacePack = (
    InterfaceWorkspaceSummary,
    Vec<ResolvedInterfaceContract>,
    Vec<ResolvedInterfaceSlot>,
    Vec<UnreviewedInterfaceObservation>,
);

impl InterfacePack {
    pub(super) fn validate(
        &self,
        facts: &InterfaceFacts,
        catalogs: &SemanticCatalogs,
        calling_convention: &str,
        execution_contracts: Option<&KnowledgeContractSpec>,
    ) -> ValidationResult<ValidatedInterfacePack> {
        validate_dotted_id(&self.id, "interface pack id")
            .map_err(|error| ValidationError::pack("id", error.to_string()))?;
        if self.calling_convention != calling_convention {
            return Err(ValidationError::pack(
                "calling-convention",
                format!(
                    "interface pack calling convention {:?} does not match project target {:?}",
                    self.calling_convention, calling_convention
                ),
            ));
        }
        let mut anchor_ids = BTreeSet::new();
        let mut matched_by = vec![None::<&str>; facts.tables.len()];
        let mut matches_by_anchor = Vec::with_capacity(self.anchors.len());
        for anchor in &self.anchors {
            validate_anchor_shape(anchor, catalogs)?;
            if !anchor_ids.insert(anchor.id.as_str()) {
                return Err(ValidationError::anchor(
                    anchor,
                    "id",
                    format!("duplicate interface anchor id {:?}", anchor.id),
                ));
            }
            let base_matches = facts
                .tables
                .iter()
                .enumerate()
                .filter(|(_, fact)| anchor_matches_without_digest(anchor, facts, fact))
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            if anchor
                .guards
                .iter()
                .any(|guard| matches!(guard, InterfaceGuard::ArtifactSha256 { .. }))
                && base_matches.iter().any(|index| {
                    facts
                        .artifact(facts.tables[*index].artifact)
                        .is_some_and(|artifact| artifact.sha256.is_none())
                })
            {
                return Err(ValidationError::anchor(
                    anchor,
                    "guards",
                    format!(
                        "anchor {:?} requires artifact SHA-256 but matching facts predate digest evidence; regenerate interfaces discover",
                        anchor.id
                    ),
                ));
            }
            let matches = base_matches
                .into_iter()
                .filter(|index| anchor_digest_matches(anchor, facts, &facts.tables[*index]))
                .collect::<Vec<_>>();
            match anchor.origin {
                PackOrigin::Observed if matches.is_empty() => {
                    return Err(ValidationError::anchor(
                        anchor,
                        "origin",
                        format!(
                            "observed interface anchor {:?} is stale or has no matching fact",
                            anchor.id
                        ),
                    ));
                }
                PackOrigin::Reviewed if !matches.is_empty() => {
                    return Err(ValidationError::anchor(
                        anchor,
                        "origin",
                        format!(
                            "reviewed-only interface anchor {:?} matches generated facts; use origin = \"observed\"",
                            anchor.id
                        ),
                    ));
                }
                _ => {}
            }
            for index in &matches {
                if let Some(other) = matched_by[*index].replace(&anchor.id) {
                    return Err(ValidationError::anchor(
                        anchor,
                        "root-kind",
                        format!(
                            "interface fact matches both anchors {other:?} and {:?}; make selectors disjoint",
                            anchor.id
                        ),
                    ));
                }
            }
            validate_anchor_evidence(anchor, facts, &matches)?;
            matches_by_anchor.push(matches);
        }
        let execution_model_sets = super::execution_models::resolve(self, execution_contracts)?;
        let bindings = build_bindings(
            self,
            facts,
            catalogs,
            &matches_by_anchor,
            &execution_model_sets,
        );
        let unreviewed_observations = build_unreviewed_observations(self, facts, &matched_by);
        let contracts = build_contracts(self, &bindings, &execution_model_sets);
        let mut summary = build_summary(self, facts, catalogs, &matched_by, &matches_by_anchor);
        summary.resolved_calls = bindings.iter().map(|binding| binding.calls.len()).sum();
        summary.execution_contracts = contracts
            .iter()
            .filter(|contract| contract.execution_contract.is_some())
            .count();
        summary.execution_models = bindings
            .iter()
            .filter(|binding| binding.execution_model.is_some())
            .count();
        Ok((summary, contracts, bindings, unreviewed_observations))
    }
}

fn validate_anchor_shape(
    anchor: &InterfaceAnchor,
    catalogs: &SemanticCatalogs,
) -> ValidationResult<()> {
    super::pack_schema::validate_anchor_shape(anchor, catalogs)
}

fn validate_anchor_evidence(
    anchor: &InterfaceAnchor,
    facts: &InterfaceFacts,
    matches: &[usize],
) -> ValidationResult<()> {
    let observed = matches
        .iter()
        .flat_map(|index| facts.tables[*index].slots.iter())
        .filter(|slot| slot.selector.is_none())
        .map(|slot| (slot.offset, slot.width))
        .collect::<BTreeSet<_>>();
    for slot in &anchor.slots {
        let key = (slot.offset, slot.width);
        match slot.origin {
            PackOrigin::Observed if !observed.contains(&key) => {
                return Err(ValidationError::slot(
                    anchor,
                    slot,
                    "origin",
                    format!(
                        "observed slot at {:+#x}/{} in anchor {:?} is stale",
                        slot.offset, slot.width, anchor.id
                    ),
                ));
            }
            PackOrigin::Reviewed if observed.contains(&key) => {
                return Err(ValidationError::slot(
                    anchor,
                    slot,
                    "origin",
                    format!(
                        "manual slot at {:+#x}/{} in anchor {:?} is now observed; change its origin",
                        slot.offset, slot.width, anchor.id
                    ),
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

pub(super) fn anchor_matches_without_digest(
    anchor: &InterfaceAnchor,
    facts: &InterfaceFacts,
    fact: &super::InterfaceTableFact,
) -> bool {
    facts
        .artifact(fact.artifact)
        .is_some_and(|artifact| artifact.sources.contains(&anchor.source))
        && selector_matches(&anchor.root, &fact.root)
        && anchor.container_path == fact.container_path
}

pub(super) fn anchor_digest_matches(
    anchor: &InterfaceAnchor,
    facts: &InterfaceFacts,
    fact: &super::InterfaceTableFact,
) -> bool {
    let expected = anchor.guards.iter().find_map(|guard| match guard {
        InterfaceGuard::ArtifactSha256 { sha256 } => Some(sha256.as_str()),
        InterfaceGuard::RuntimeValue { .. } => None,
    });
    expected.is_none_or(|expected| {
        facts
            .artifact(fact.artifact)
            .and_then(|artifact| artifact.sha256.as_deref())
            == Some(expected)
    })
}

fn selector_matches(selector: &InterfaceRootSelector, root: &InterfaceFactRoot) -> bool {
    match (selector, root) {
        (
            InterfaceRootSelector::RelocatedSymbol {
                member,
                symbol,
                addend,
                addressing,
            },
            InterfaceFactRoot::RelocatedSymbol {
                member: fact_member,
                symbol: fact_symbol,
                addend: fact_addend,
                addressing: fact_addressing,
            },
        ) => {
            member
                .as_ref()
                .is_none_or(|member| Some(member) == fact_member.as_ref())
                && symbol == fact_symbol
                && addend == fact_addend
                && addressing == fact_addressing
        }
        (
            InterfaceRootSelector::FunctionArgument { argument },
            InterfaceFactRoot::FunctionArgument {
                argument: fact_argument,
            },
        ) => argument == fact_argument,
        (
            InterfaceRootSelector::AbsoluteAddress { address },
            InterfaceFactRoot::AbsoluteAddress {
                address: fact_address,
            },
        ) => address == fact_address,
        (
            InterfaceRootSelector::AbsoluteAddress { address },
            InterfaceFactRoot::BoundedDataAddress {
                address: fact_address,
                ..
            },
        ) => address == fact_address,
        _ => false,
    }
}

fn build_summary(
    pack: &InterfacePack,
    facts: &InterfaceFacts,
    catalogs: &SemanticCatalogs,
    matched_by: &[Option<&str>],
    matches_by_anchor: &[Vec<usize>],
) -> InterfaceWorkspaceSummary {
    let anchors_by_id = pack
        .anchors
        .iter()
        .map(|anchor| (anchor.id.as_str(), anchor))
        .collect::<BTreeMap<_, _>>();
    let mut summary = InterfaceWorkspaceSummary {
        fact_tables: facts.tables.len(),
        observed_slots: facts.observed_slots(),
        observed_calls: facts.observed_calls(),
        unreviewed_anchors: matched_by.iter().filter(|anchor| anchor.is_none()).count(),
        semantic_operations: catalogs.len(),
        ..InterfaceWorkspaceSummary::default()
    };
    for anchor in &pack.anchors {
        match anchor.status {
            ReviewStatus::Reviewed => summary.reviewed_anchors += 1,
            ReviewStatus::Ignored => summary.ignored_anchors += 1,
            ReviewStatus::Unreviewed => summary.unreviewed_anchors += 1,
        }
        if anchor.origin == PackOrigin::Reviewed {
            summary.asserted_anchors += 1;
        }
        summary.artifact_guards += anchor
            .guards
            .iter()
            .filter(|guard| matches!(guard, InterfaceGuard::ArtifactSha256 { .. }))
            .count();
        summary.runtime_guards += anchor
            .guards
            .iter()
            .filter(|guard| matches!(guard, InterfaceGuard::RuntimeValue { .. }))
            .count();
        summary.asserted_slots += anchor
            .slots
            .iter()
            .filter(|slot| slot.origin == PackOrigin::Reviewed)
            .count();
        summary.semantic_links += anchor
            .slots
            .iter()
            .filter(|slot| slot.semantic.is_some())
            .count();
    }
    for (fact_index, fact) in facts.tables.iter().enumerate() {
        let Some(anchor) = matched_by[fact_index].and_then(|id| anchors_by_id.get(id).copied())
        else {
            summary.unreviewed_slots += fact.slots.len();
            continue;
        };
        if anchor.status == ReviewStatus::Ignored {
            summary.ignored_slots += fact.slots.len();
            continue;
        }
        for observed in &fact.slots {
            if let Some(selector) = observed.selector {
                let candidates =
                    indexed_candidate_offsets(anchor, observed.offset, observed.width, selector);
                let classified = !candidates.is_empty()
                    && candidates.into_iter().all(|offset| {
                        anchor.slots.iter().any(|slot| {
                            slot.offset == offset
                                && slot.width == observed.width
                                && slot.status != ReviewStatus::Unreviewed
                        })
                    });
                if classified {
                    summary.reviewed_slots += 1;
                } else {
                    summary.unreviewed_slots += 1;
                }
                continue;
            }
            let status = anchor
                .slots
                .iter()
                .find(|slot| {
                    slot.origin == PackOrigin::Observed
                        && (slot.offset, slot.width) == (observed.offset, observed.width)
                })
                .map_or(ReviewStatus::Unreviewed, |slot| slot.status);
            match status {
                ReviewStatus::Reviewed => summary.reviewed_slots += 1,
                ReviewStatus::Ignored => summary.ignored_slots += 1,
                ReviewStatus::Unreviewed => summary.unreviewed_slots += 1,
            }
        }
    }
    debug_assert_eq!(pack.anchors.len(), matches_by_anchor.len());
    summary
}

fn build_unreviewed_observations(
    pack: &InterfacePack,
    facts: &InterfaceFacts,
    matched_by: &[Option<&str>],
) -> Vec<UnreviewedInterfaceObservation> {
    let anchors_by_id = pack
        .anchors
        .iter()
        .map(|anchor| (anchor.id.as_str(), anchor))
        .collect::<BTreeMap<_, _>>();
    let mut observations = Vec::new();
    for (fact_index, fact) in facts.tables.iter().enumerate() {
        let anchor = matched_by[fact_index].and_then(|id| anchors_by_id.get(id).copied());
        if anchor.is_some_and(|anchor| anchor.status == ReviewStatus::Ignored) {
            continue;
        }
        let artifact = facts
            .artifact(fact.artifact)
            .expect("validated interface facts reference an artifact");
        let source = artifact
            .sources
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join(",");
        let contract = anchor.map_or_else(
            || format!("unmatched:{source}:{}", fact.root.kind()),
            |anchor| format!("{}::{}", pack.id, anchor.id),
        );
        for observed in &fact.slots {
            let classified = anchor.is_some_and(|anchor| {
                observed.selector.map_or_else(
                    || {
                        anchor.slots.iter().any(|slot| {
                            slot.origin == PackOrigin::Observed
                                && slot.offset == observed.offset
                                && slot.width == observed.width
                                && slot.status != ReviewStatus::Unreviewed
                        })
                    },
                    |selector| {
                        let candidates = indexed_candidate_offsets(
                            anchor,
                            observed.offset,
                            observed.width,
                            selector,
                        );
                        !candidates.is_empty()
                            && candidates.into_iter().all(|offset| {
                                anchor.slots.iter().any(|slot| {
                                    slot.offset == offset
                                        && slot.width == observed.width
                                        && slot.status != ReviewStatus::Unreviewed
                                })
                            })
                    },
                )
            });
            if classified {
                continue;
            }
            let call_sites = facts
                .calls
                .iter()
                .filter(|call| {
                    let Some((call_slot, container)) = call.loads.split_last() else {
                        return false;
                    };
                    call.artifact == fact.artifact
                        && call.root == fact.root
                        && container == fact.container_path
                        && call_slot.offset == observed.offset
                        && call_slot.width == observed.width
                        && call_slot.selector == observed.selector
                })
                .map(|call| call.site)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            let selector = observed
                .selector
                .map(super::InterfaceFactSelector::canonical);
            observations.push(UnreviewedInterfaceObservation {
                id: format!(
                    "{contract}::fact-{fact_index}@{:+#x}/{}{}",
                    observed.offset,
                    observed.width,
                    selector
                        .as_deref()
                        .map_or_else(String::new, |selector| format!("[{selector}]")),
                ),
                contract: contract.clone(),
                source: source.clone(),
                offset: observed.offset,
                width: observed.width,
                selector,
                functions: observed.functions.iter().cloned().collect(),
                call_sites,
            });
        }
    }
    observations.sort_by(|left, right| {
        (&left.source, &left.contract, left.offset, &left.selector).cmp(&(
            &right.source,
            &right.contract,
            right.offset,
            &right.selector,
        ))
    });
    observations
}

fn build_bindings(
    pack: &InterfacePack,
    facts: &InterfaceFacts,
    catalogs: &SemanticCatalogs,
    matches_by_anchor: &[Vec<usize>],
    execution_model_sets: &[Option<ExternalCallModelSetRef>],
) -> Vec<ResolvedInterfaceSlot> {
    let mut bindings = Vec::new();
    for ((anchor, matches), execution_model_set) in pack
        .anchors
        .iter()
        .zip(matches_by_anchor)
        .zip(execution_model_sets)
    {
        for slot in anchor
            .slots
            .iter()
            .filter(|slot| slot.status == ReviewStatus::Reviewed)
        {
            let assignments = facts
                .assignments
                .iter()
                .filter(|assignment| {
                    assignment.width == slot.width
                        && assignment.offset == slot.offset
                        && assignment.container_path == anchor.container_path
                        && selector_matches(&anchor.root, &assignment.root)
                        && facts
                            .artifact(assignment.artifact)
                            .is_some_and(|artifact| artifact.sources.contains(&anchor.source))
                })
                .filter_map(|assignment| {
                    let super::InterfaceFactRoot::RelocatedSymbol {
                        member,
                        symbol,
                        addend,
                        ..
                    } = &assignment.target
                    else {
                        return None;
                    };
                    Some(super::ResolvedInterfaceAssignment {
                        member: assignment.member.clone(),
                        producer: assignment.function.clone(),
                        site: assignment.site,
                        target_member: member.clone(),
                        target_symbol: symbol.clone(),
                        target_addend: *addend,
                    })
                })
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let functions = matches
                .iter()
                .flat_map(|index| facts.tables[*index].slots.iter())
                .filter(|fact| {
                    fact.width == slot.width
                        && fact.selector.map_or_else(
                            || fact.offset == slot.offset,
                            |selector| {
                                indexed_slot_index(anchor, fact.offset, slot.offset, selector)
                                    .is_some()
                            },
                        )
                })
                .flat_map(|fact| fact.functions.iter().cloned())
                .collect();
            let calls = matches
                .iter()
                .flat_map(|index| {
                    let table = &facts.tables[*index];
                    facts.calls.iter().filter(move |call| {
                        let Some((call_slot, container)) = call.loads.split_last() else {
                            return false;
                        };
                        call.artifact == table.artifact
                            && call.root == table.root
                            && container == table.container_path
                            && call_slot.width == slot.width
                            && call_slot.selector.map_or_else(
                                || call_slot.offset == slot.offset,
                                |selector| {
                                    indexed_slot_index(
                                        anchor,
                                        call_slot.offset,
                                        slot.offset,
                                        selector,
                                    )
                                    .is_some()
                                },
                            )
                    })
                })
                .map(|call| {
                    let selector = call.loads.last().and_then(|load| load.selector);
                    let slot_index = selector.and_then(|selector| {
                        indexed_slot_index(
                            anchor,
                            call.loads
                                .last()
                                .expect("matched call has a slot load")
                                .offset,
                            slot.offset,
                            selector,
                        )
                    });
                    let slot_index_domain = selector
                        .and_then(|selector| index_domain(anchor, selector.argument))
                        .map(|domain| super::ResolvedInterfaceIndexDomain {
                            argument: domain.argument,
                            min: domain.min,
                            max: domain.max,
                            evidence: domain.evidence.clone(),
                        });
                    super::ResolvedInterfaceCall {
                        artifact: call.artifact,
                        member: call.member.clone(),
                        function: call.function.clone(),
                        function_address: call.function_address,
                        site: call.site,
                        slot_load_site: call.slot_load_site,
                        kind: call.kind.clone(),
                        jalr_offset: call.jalr_offset,
                        slot_selector: selector.map(super::InterfaceFactSelector::canonical),
                        slot_index,
                        slot_index_domain,
                        arguments: call
                            .arguments
                            .iter()
                            .map(|argument| super::ResolvedInterfaceArgument {
                                index: argument.index,
                                kind: argument.kind.clone(),
                                expression: argument.expression.clone(),
                            })
                            .collect(),
                    }
                })
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            bindings.push(ResolvedInterfaceSlot {
                id: format!("{}::{}@{:+#x}", pack.id, anchor.id, slot.offset),
                contract: format!("{}::{}", pack.id, anchor.id),
                anchor: anchor.id.clone(),
                source: anchor.source.clone(),
                layout_version: anchor
                    .layout_version
                    .clone()
                    .expect("reviewed slot requires reviewed layout"),
                offset: slot.offset,
                width: slot.width,
                name: slot.name.clone().expect("reviewed slot requires a name"),
                arguments: slot
                    .arguments
                    .clone()
                    .expect("reviewed slot requires arguments"),
                return_type: slot
                    .return_type
                    .clone()
                    .expect("reviewed slot requires return type"),
                variadic: slot.variadic,
                semantic: slot.semantic.clone(),
                semantic_annotation: slot.semantic.as_deref().map(|id| {
                    let operation = catalogs
                        .get(id)
                        .expect("reviewed semantic was validated against the catalog");
                    ResolvedSemanticAnnotation {
                        operation: operation.id.clone(),
                        domain: operation.domain.clone(),
                        summary: operation.summary.clone(),
                        argument_roles: operation.argument_roles.clone(),
                        return_role: operation.return_role.clone(),
                        effects: operation.effects.clone(),
                        replacement: operation.replacement.clone(),
                    }
                }),
                execution_model_set: execution_model_set
                    .map(|model_set| model_set.spec().id.to_owned()),
                execution_model: slot.execution_model.as_deref().map(|model_id| {
                    let model_set = execution_model_set
                        .expect("an execution model requires a resolved execution contract");
                    let model = model_set
                        .model(model_id)
                        .expect("execution model was validated");
                    let model = model.spec();
                    ResolvedExternalCallExecutionModel {
                        id: format!("{}.{}", model_set.spec().id, model.id),
                        set: model_set.spec().id.to_owned(),
                        model: model.id.to_owned(),
                        return_model: model.return_model,
                        outputs: model.outputs.to_vec(),
                    }
                }),
                assignments,
                functions,
                calls,
            });
        }
    }
    bindings.sort_by(|left, right| {
        (&left.source, &left.anchor, left.offset, &left.name).cmp(&(
            &right.source,
            &right.anchor,
            right.offset,
            &right.name,
        ))
    });
    bindings
}

fn indexed_candidate_offsets(
    anchor: &InterfaceAnchor,
    fixed_offset: i32,
    width: u8,
    selector: super::InterfaceFactSelector,
) -> Vec<i32> {
    let Some(layout_size) = anchor.layout_size else {
        return Vec::new();
    };
    let stride = i32::from(anchor.slot_stride.unwrap_or(1));
    if stride <= 0 {
        return Vec::new();
    }
    let width_bytes = u32::from(width) / 8;
    let Some(last_offset) = layout_size.checked_sub(width_bytes) else {
        return Vec::new();
    };
    let Some(domain) = index_domain(anchor, selector.argument) else {
        return Vec::new();
    };
    (domain.min..=domain.max)
        .filter_map(|index| {
            let base = i64::from(fixed_offset).checked_add(i64::from(selector.addend))?;
            let scaled = i64::from(index).checked_mul(i64::from(selector.scale))?;
            base.checked_add(scaled)
                .and_then(|offset| i32::try_from(offset).ok())
        })
        .filter(|offset| {
            *offset >= 0
                && u32::try_from(*offset).is_ok_and(|offset| offset <= last_offset)
                && *offset % stride == 0
        })
        .collect()
}

fn index_domain(anchor: &InterfaceAnchor, argument: u8) -> Option<&super::InterfaceIndexDomain> {
    anchor
        .index_domains
        .iter()
        .find(|domain| domain.argument == argument)
}

fn indexed_slot_index(
    anchor: &InterfaceAnchor,
    fixed_offset: i32,
    slot_offset: i32,
    selector: super::InterfaceFactSelector,
) -> Option<u32> {
    let index = selector.index_for_offset(slot_offset.wrapping_sub(fixed_offset))?;
    let domain = index_domain(anchor, selector.argument)?;
    (domain.min..=domain.max).contains(&index).then_some(index)
}

fn build_contracts(
    pack: &InterfacePack,
    bindings: &[ResolvedInterfaceSlot],
    execution_model_sets: &[Option<ExternalCallModelSetRef>],
) -> Vec<ResolvedInterfaceContract> {
    pack.anchors
        .iter()
        .zip(execution_model_sets)
        .filter(|(anchor, _)| anchor.status == ReviewStatus::Reviewed)
        .map(|(anchor, execution_model_set)| ResolvedInterfaceContract {
            id: format!("{}::{}", pack.id, anchor.id),
            pack: pack.id.clone(),
            anchor: anchor.id.clone(),
            template: anchor.template.clone(),
            template_overrides: anchor.template_overrides.clone(),
            source: anchor.source.clone(),
            root: anchor.root.clone(),
            container_path: anchor.container_path.clone(),
            layout_version: anchor
                .layout_version
                .clone()
                .expect("reviewed anchor requires layout version"),
            pointer_width: anchor
                .pointer_width
                .expect("reviewed anchor requires pointer width"),
            layout_size: anchor
                .layout_size
                .expect("reviewed anchor requires layout size"),
            slot_stride: anchor
                .slot_stride
                .expect("reviewed anchor requires slot stride"),
            guards: anchor.guards.clone(),
            execution_contract: execution_model_set.map(|model_set| {
                ResolvedInterfaceExecutionContract {
                    id: model_set.spec().id.to_owned(),
                }
            }),
            slots: bindings
                .iter()
                .filter(|binding| binding.anchor == anchor.id)
                .map(|binding| binding.id.clone())
                .collect(),
        })
        .collect()
}
