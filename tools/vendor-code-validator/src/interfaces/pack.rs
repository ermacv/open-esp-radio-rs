//! Validation and evidence matching for reviewed interface packs.

use std::collections::{BTreeMap, BTreeSet};

use super::{
    InterfaceAnchor, InterfaceFactRoot, InterfaceFacts, InterfaceGuard, InterfacePack,
    InterfaceRootSelector, InterfaceWorkspaceSummary, PackOrigin, ResolvedInterfaceSlot,
    ReviewStatus, SemanticCatalogs, validate_dotted_id,
};
use crate::Result;

impl InterfacePack {
    pub(super) fn validate(
        &self,
        facts: &InterfaceFacts,
        catalogs: &SemanticCatalogs,
        calling_convention: &str,
    ) -> Result<(InterfaceWorkspaceSummary, Vec<ResolvedInterfaceSlot>)> {
        validate_dotted_id(&self.id, "interface pack id")?;
        if self.calling_convention != calling_convention {
            return Err(format!(
                "interface pack calling convention {:?} does not match project target {:?}",
                self.calling_convention, calling_convention
            )
            .into());
        }
        let mut anchor_ids = BTreeSet::new();
        let mut matched_by = vec![None::<&str>; facts.tables.len()];
        let mut matches_by_anchor = Vec::with_capacity(self.anchors.len());
        for anchor in &self.anchors {
            validate_anchor_shape(anchor, catalogs)?;
            if !anchor_ids.insert(anchor.id.as_str()) {
                return Err(format!("duplicate interface anchor id {:?}", anchor.id).into());
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
                return Err(format!(
                    "anchor {:?} requires artifact SHA-256 but matching facts predate digest evidence; regenerate interfaces discover",
                    anchor.id
                )
                .into());
            }
            let matches = base_matches
                .into_iter()
                .filter(|index| anchor_digest_matches(anchor, facts, &facts.tables[*index]))
                .collect::<Vec<_>>();
            match anchor.origin {
                PackOrigin::Observed if matches.is_empty() => {
                    return Err(format!(
                        "observed interface anchor {:?} is stale or has no matching fact",
                        anchor.id
                    )
                    .into());
                }
                PackOrigin::Manual if !matches.is_empty() => {
                    return Err(format!(
                        "manual interface anchor {:?} matches generated facts; use origin = \"observed\"",
                        anchor.id
                    )
                    .into());
                }
                _ => {}
            }
            for index in &matches {
                if let Some(other) = matched_by[*index].replace(&anchor.id) {
                    return Err(format!(
                        "interface fact matches both anchors {other:?} and {:?}; make selectors disjoint",
                        anchor.id
                    )
                    .into());
                }
            }
            validate_anchor_evidence(anchor, facts, &matches)?;
            matches_by_anchor.push(matches);
        }
        let summary = build_summary(self, facts, catalogs, &matched_by, &matches_by_anchor);
        let bindings = build_bindings(self, facts, &matches_by_anchor);
        Ok((summary, bindings))
    }
}

fn validate_anchor_shape(anchor: &InterfaceAnchor, catalogs: &SemanticCatalogs) -> Result<()> {
    super::pack_schema::validate_anchor_shape(anchor, catalogs)
}

fn validate_anchor_evidence(
    anchor: &InterfaceAnchor,
    facts: &InterfaceFacts,
    matches: &[usize],
) -> Result<()> {
    let observed = matches
        .iter()
        .flat_map(|index| facts.tables[*index].slots.iter())
        .map(|slot| (slot.offset, slot.width))
        .collect::<BTreeSet<_>>();
    for slot in &anchor.slots {
        let key = (slot.offset, slot.width);
        match slot.origin {
            PackOrigin::Observed if !observed.contains(&key) => {
                return Err(format!(
                    "observed slot at {:+#x}/{} in anchor {:?} is stale",
                    slot.offset, slot.width, anchor.id
                )
                .into());
            }
            PackOrigin::Manual if observed.contains(&key) => {
                return Err(format!(
                    "manual slot at {:+#x}/{} in anchor {:?} is now observed; change its origin",
                    slot.offset, slot.width, anchor.id
                )
                .into());
            }
            _ => {}
        }
    }
    Ok(())
}

fn anchor_matches_without_digest(
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

fn anchor_digest_matches(
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
        semantic_operations: catalogs.len(),
        ..InterfaceWorkspaceSummary::default()
    };
    for anchor in &pack.anchors {
        match anchor.status {
            ReviewStatus::Reviewed => summary.reviewed_anchors += 1,
            ReviewStatus::Ignored => summary.ignored_anchors += 1,
            ReviewStatus::Unreviewed => summary.unreviewed_anchors += 1,
        }
        if anchor.origin == PackOrigin::Manual {
            summary.manual_anchors += 1;
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
        summary.manual_slots += anchor
            .slots
            .iter()
            .filter(|slot| slot.origin == PackOrigin::Manual)
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

fn build_bindings(
    pack: &InterfacePack,
    facts: &InterfaceFacts,
    matches_by_anchor: &[Vec<usize>],
) -> Vec<ResolvedInterfaceSlot> {
    let mut bindings = Vec::new();
    for (anchor, matches) in pack.anchors.iter().zip(matches_by_anchor) {
        for slot in anchor
            .slots
            .iter()
            .filter(|slot| slot.status == ReviewStatus::Reviewed)
        {
            let functions = matches
                .iter()
                .flat_map(|index| facts.tables[*index].slots.iter())
                .filter(|fact| (fact.offset, fact.width) == (slot.offset, slot.width))
                .flat_map(|fact| fact.functions.iter().cloned())
                .collect();
            bindings.push(ResolvedInterfaceSlot {
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
                functions,
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
