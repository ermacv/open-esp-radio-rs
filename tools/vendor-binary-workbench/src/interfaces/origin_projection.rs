//! Fail-closed projection of reviewed archive interface evidence onto an
//! authoritative linked ELF.

use std::collections::{BTreeMap, BTreeSet};

use super::{
    InterfaceCallFact, InterfaceRootSelector, InterfaceWorkspace, ResolvedInterfaceContract,
    ResolvedInterfaceSlot,
};
use crate::artifacts::LinkUnitOriginFact;

/// One reviewed slot binding projected onto an exact call instruction in the
/// authoritative link unit.
#[derive(Clone, Debug)]
pub(crate) struct ProjectedInterfaceCall<'a> {
    pub(crate) member: Option<String>,
    pub(crate) function: String,
    pub(crate) site: u32,
    pub(crate) slot_load_site: Option<u32>,
    pub(crate) tail: bool,
    pub(crate) binding: &'a ResolvedInterfaceSlot,
}

impl InterfaceWorkspace {
    /// Project reviewed archive evidence onto `source` in a linked ELF.
    ///
    /// The projection requires a unique name-and-kind archive origin from the
    /// symbol inventory and an identical decoded indirect-target shape.  Call
    /// addresses are deliberately not compared by relative offset because
    /// RISC-V linker relaxation can change instruction widths and positions.
    /// If two reviewed contracts still match, the call is omitted rather than
    /// guessed.
    pub(crate) fn project_link_unit_calls<'a>(
        &'a self,
        source: &str,
        origins: &[LinkUnitOriginFact],
    ) -> Vec<ProjectedInterfaceCall<'a>> {
        let facts = self.facts();
        let artifacts_by_digest = facts
            .artifacts
            .iter()
            .map(|artifact| (artifact.sha256.as_deref(), artifact.index))
            .collect::<BTreeMap<_, _>>();
        let mut projected = Vec::new();

        for origin in origins.iter().filter(|origin| {
            origin.kind == "text"
                && origin
                    .linked_sources
                    .iter()
                    .any(|candidate| candidate == source)
                && origin
                    .origin_sources
                    .iter()
                    .any(|candidate| candidate == source)
        }) {
            let Some(&linked_artifact) =
                artifacts_by_digest.get(&Some(origin.linked_artifact_sha256.as_str()))
            else {
                continue;
            };
            let Some(&archive_artifact) =
                artifacts_by_digest.get(&Some(origin.origin_artifact_sha256.as_str()))
            else {
                continue;
            };
            let Ok(linked_address) = u32::try_from(origin.linked_address) else {
                continue;
            };
            let Ok(archive_address) = u32::try_from(origin.origin_address) else {
                continue;
            };

            for linked_call in facts.calls.iter().filter(|call| {
                call.artifact == linked_artifact
                    && call.member == origin.linked_member
                    && call.function == origin.symbol
                    && call.function_address == linked_address
            }) {
                let matching_bindings = self
                    .bindings()
                    .iter()
                    .filter(|binding| {
                        let Some(contract) = self
                            .contracts()
                            .iter()
                            .find(|contract| contract.id == binding.contract)
                        else {
                            return false;
                        };
                        facts.calls.iter().any(|archive_call| {
                            archive_call_matches_origin(
                                archive_call,
                                archive_artifact,
                                origin,
                                archive_address,
                            ) && archive_call_matches_binding(
                                archive_call,
                                binding,
                                contract,
                                facts,
                            ) && same_indirect_target_shape(archive_call, linked_call)
                        })
                    })
                    .collect::<Vec<_>>();
                let unique_ids = matching_bindings
                    .iter()
                    .map(|binding| binding.id.as_str())
                    .collect::<BTreeSet<_>>();
                if unique_ids.len() != 1 {
                    continue;
                }
                let binding = matching_bindings[0];
                projected.push(ProjectedInterfaceCall {
                    member: linked_call.member.clone(),
                    function: linked_call.function.clone(),
                    site: linked_call.site,
                    slot_load_site: linked_call.slot_load_site,
                    tail: linked_call.kind == "tail-jump",
                    binding,
                });
            }
        }
        projected.sort_by(|left, right| {
            (&left.member, &left.function, left.site, &left.binding.id).cmp(&(
                &right.member,
                &right.function,
                right.site,
                &right.binding.id,
            ))
        });
        projected.dedup_by(|left, right| {
            left.member == right.member
                && left.function == right.function
                && left.site == right.site
                && left.binding.id == right.binding.id
        });
        projected
    }
}

fn archive_call_matches_origin(
    call: &InterfaceCallFact,
    archive_artifact: usize,
    origin: &LinkUnitOriginFact,
    archive_address: u32,
) -> bool {
    call.artifact == archive_artifact
        && call.member == origin.origin_member
        && call.function == origin.symbol
        && call.function_address == archive_address
}

fn archive_call_matches_binding(
    call: &InterfaceCallFact,
    binding: &ResolvedInterfaceSlot,
    contract: &ResolvedInterfaceContract,
    facts: &super::InterfaceFacts,
) -> bool {
    let Some(slot) = call.loads.last() else {
        return false;
    };
    if slot.offset != binding.offset || slot.width != binding.width || slot.selector.is_some() {
        return false;
    }
    let InterfaceRootSelector::AbsoluteAddress { address } = contract.root else {
        return false;
    };
    let Some(first_contract_step) = contract.container_path.first() else {
        return false;
    };
    let call_container = &call.loads[..call.container_depth];
    if call_container.len() != contract.container_path.len()
        || call_container.first().is_none_or(|step| {
            step.width != first_contract_step.width || step.selector != first_contract_step.selector
        })
        || !call_container[1..]
            .iter()
            .zip(&contract.container_path[1..])
            .all(|(call, contract)| call == contract)
    {
        return false;
    }
    let Some(pointer_cell) = address.checked_add_signed(first_contract_step.offset) else {
        return false;
    };
    // Project inventory may contain unrelated linked test/oracle definitions
    // of the same pointer symbol. They make the global association ambiguous,
    // but they do not make this reviewed contract ambiguous: guards and source
    // ownership identify the one candidate that can satisfy this anchor.
    // This is semantic-contract selection, not a claim about linker choice.
    call.root_linkage
        .candidates
        .iter()
        .filter(|candidate| {
            candidate.kind == "data"
                && candidate.address == pointer_cell
                && facts
                    .artifact(candidate.artifact)
                    .is_some_and(|artifact| artifact.sources.contains(&contract.source))
        })
        .count()
        == 1
}

fn same_indirect_target_shape(left: &InterfaceCallFact, right: &InterfaceCallFact) -> bool {
    left.kind == right.kind
        && left.jalr_offset == right.jalr_offset
        && left.container_depth == right.container_depth
        && left.loads.len() == right.loads.len()
        && left.loads.iter().zip(&right.loads).all(|(left, right)| {
            left.offset == right.offset
                && left.width == right.width
                && left.selector == right.selector
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::{
        InterfaceFactArtifact, InterfaceFactRoot, InterfaceFactStep, InterfaceFacts,
        InterfaceRootLinkageFact, InterfaceSymbolLocationFact,
    };

    fn call(
        artifact: usize,
        member: Option<&str>,
        function_address: u32,
        site: u32,
    ) -> InterfaceCallFact {
        InterfaceCallFact {
            artifact,
            member: member.map(str::to_owned),
            function: "post_event".to_owned(),
            function_address,
            site,
            slot_load_site: Some(site - 2),
            kind: "call".to_owned(),
            root: InterfaceFactRoot::RelocatedSymbol {
                member: member.map(str::to_owned),
                symbol: "g_services".to_owned(),
                addend: 0,
                addressing: "absolute".to_owned(),
            },
            loads: vec![
                InterfaceFactStep {
                    offset: 0,
                    width: 32,
                    selector: None,
                },
                InterfaceFactStep {
                    offset: 16,
                    width: 32,
                    selector: None,
                },
            ],
            container_depth: 1,
            slot_offset: Some(16),
            jalr_offset: 0,
            arguments: Vec::new(),
            root_linkage: InterfaceRootLinkageFact {
                symbols: vec!["g_services".to_owned()],
                resolutions: vec!["project-associated".to_owned()],
                candidates: vec![InterfaceSymbolLocationFact {
                    artifact: 0,
                    member: None,
                    address: 0x1f00,
                    kind: "data".to_owned(),
                }],
            },
        }
    }

    fn binding() -> ResolvedInterfaceSlot {
        ResolvedInterfaceSlot {
            id: "pack::services@+0x10".to_owned(),
            contract: "pack::services".to_owned(),
            anchor: "services".to_owned(),
            source: "rom".to_owned(),
            layout_version: "v1".to_owned(),
            offset: 16,
            width: 32,
            name: "post_event".to_owned(),
            arguments: Vec::new(),
            return_type: "void".to_owned(),
            variadic: false,
            semantic: None,
            semantic_annotation: None,
            execution_model_set: None,
            execution_model: None,
            assignments: Vec::new(),
            functions: BTreeSet::new(),
            calls: Vec::new(),
        }
    }

    fn contract() -> ResolvedInterfaceContract {
        ResolvedInterfaceContract {
            id: "pack::services".to_owned(),
            pack: "pack".to_owned(),
            anchor: "services".to_owned(),
            source: "rom".to_owned(),
            root: InterfaceRootSelector::AbsoluteAddress { address: 0x2000 },
            container_path: vec![InterfaceFactStep {
                offset: -0x100,
                width: 32,
                selector: None,
            }],
            layout_version: "v1".to_owned(),
            pointer_width: 32,
            layout_size: 32,
            slot_stride: 4,
            guards: Vec::new(),
            execution_contract: None,
            slots: vec!["pack::services@+0x10".to_owned()],
        }
    }

    fn facts() -> InterfaceFacts {
        InterfaceFacts {
            artifacts: vec![InterfaceFactArtifact {
                index: 0,
                sources: BTreeSet::from(["rom".to_owned()]),
                sha256: Some("00".repeat(32)),
            }],
            tables: Vec::new(),
            calls: Vec::new(),
            assignments: Vec::new(),
        }
    }

    #[test]
    fn project_associated_pointer_cell_joins_archive_call_to_absolute_contract() {
        let call = call(1, Some("event.o"), 0, 0x40);
        assert!(archive_call_matches_binding(
            &call,
            &binding(),
            &contract(),
            &facts(),
        ));

        let mut wrong = call.clone();
        wrong.root_linkage.candidates[0].address = 0x1f04;
        assert!(!archive_call_matches_binding(
            &wrong,
            &binding(),
            &contract(),
            &facts(),
        ));
    }

    #[test]
    fn unrelated_project_definition_does_not_erase_unique_reviewed_anchor() {
        let mut call = call(1, Some("event.o"), 0, 0x40);
        call.root_linkage.resolutions = vec!["ambiguous-project".to_owned()];
        call.root_linkage
            .candidates
            .push(InterfaceSymbolLocationFact {
                artifact: 1,
                member: None,
                address: 0x3000,
                kind: "data".to_owned(),
            });
        let mut facts = facts();
        facts.artifacts.push(InterfaceFactArtifact {
            index: 1,
            sources: BTreeSet::from(["test-harness".to_owned()]),
            sha256: Some("11".repeat(32)),
        });

        assert!(archive_call_matches_binding(
            &call,
            &binding(),
            &contract(),
            &facts,
        ));
    }

    #[test]
    fn indirect_shape_ignores_relaxed_sites_but_not_slot_or_call_kind() {
        let archive = call(1, Some("event.o"), 0, 0x40);
        let mut linked = call(2, None, 0x1000, 0x1062);
        assert!(same_indirect_target_shape(&archive, &linked));

        linked.loads[1].offset = 20;
        assert!(!same_indirect_target_shape(&archive, &linked));
        linked.loads[1].offset = 16;
        linked.kind = "tail-jump".to_owned();
        assert!(!same_indirect_target_shape(&archive, &linked));
    }
}
