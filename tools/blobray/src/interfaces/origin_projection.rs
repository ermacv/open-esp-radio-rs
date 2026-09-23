//! Fail-closed projection of reviewed archive interface evidence onto an
//! authoritative linked ELF.

use std::collections::{BTreeMap, BTreeSet};

use super::{
    InterfaceCallFact, InterfaceRootSelector, InterfaceWorkspace, ResolvedInterfaceContract,
    ResolvedInterfaceSlot,
};
use crate::artifacts::LinkUnitOriginFact;
use crate::{StructuralCallSite, StructuralProjectedRelocation};

type FunctionCallKey = (usize, crate::artifact::CodeIdentity);

fn function_call_key(call: &InterfaceCallFact) -> FunctionCallKey {
    (call.artifact, call.owner.clone())
}

/// One reviewed slot binding projected onto an exact call instruction in the
/// authoritative link unit.
#[derive(Clone, Debug)]
pub(crate) struct ProjectedInterfaceCall<'a> {
    pub(crate) owner: crate::artifact::CodeIdentity,
    pub(crate) site: u32,
    pub(crate) slot_load_site: Option<u32>,
    pub(crate) tail: bool,
    pub(crate) binding: &'a ResolvedInterfaceSlot,
}

impl InterfaceWorkspace {
    /// Project reviewed archive evidence onto `source` in a linked ELF.
    ///
    /// The projection requires a physical archive and linked occurrences from the
    /// symbol inventory and an identical decoded indirect-target shape.  Call
    /// addresses are deliberately not compared by relative offset because
    /// RISC-V linker relaxation can change instruction widths and positions.
    /// If two reviewed contracts still match, the call is omitted rather than
    /// guessed.
    pub(crate) fn project_link_unit_calls<'a>(
        &'a self,
        source: &str,
        origins: &[LinkUnitOriginFact],
        projected_relocations: &BTreeMap<StructuralCallSite, Vec<StructuralProjectedRelocation>>,
    ) -> Vec<ProjectedInterfaceCall<'a>> {
        let facts = self.facts();
        let artifacts_by_digest = facts
            .artifacts
            .iter()
            .map(|artifact| (artifact.sha256.as_deref(), artifact.index))
            .collect::<BTreeMap<_, _>>();
        let contracts_by_id = self
            .contracts()
            .iter()
            .map(|contract| (contract.id.as_str(), contract))
            .collect::<BTreeMap<_, _>>();
        let mut calls_by_function = BTreeMap::<FunctionCallKey, Vec<&InterfaceCallFact>>::new();
        for call in &facts.calls {
            calls_by_function
                .entry(function_call_key(call))
                .or_default()
                .push(call);
        }
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
            let linked_key = (linked_artifact, origin.linked_identity());
            let archive_key = (archive_artifact, origin.origin_identity());
            let Some(linked_calls) = calls_by_function.get(&linked_key) else {
                continue;
            };
            let Some(archive_calls) = calls_by_function.get(&archive_key) else {
                continue;
            };

            for linked_call in linked_calls {
                let matching_bindings = self
                    .bindings()
                    .iter()
                    .filter(|binding| {
                        let Some(contract) =
                            contracts_by_id.get(binding.contract.as_str()).copied()
                        else {
                            return false;
                        };
                        archive_calls.iter().any(|archive_call| {
                            let binding_matches = archive_call_matches_binding(
                                archive_call,
                                binding,
                                contract,
                                facts,
                            );
                            binding_matches
                                && (same_indirect_target_shape(archive_call, linked_call)
                                    || relocated_direct_target_matches(
                                        archive_call,
                                        linked_call,
                                        origin,
                                        projected_relocations,
                                    ))
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
                    owner: origin.linked_identity(),
                    site: linked_call.site,
                    slot_load_site: linked_call.slot_load_site,
                    tail: linked_call.kind == "tail-jump",
                    binding,
                });
            }
        }
        projected.sort_by(|left, right| {
            (&left.owner, left.site, &left.binding.id).cmp(&(
                &right.owner,
                right.site,
                &right.binding.id,
            ))
        });
        projected.dedup_by(|left, right| {
            left.owner == right.owner
                && left.site == right.site
                && left.binding.id == right.binding.id
        });
        projected
    }
}

/// Match a direct pointer-cell load whose instruction immediate changed when
/// the linker assigned the cell its final address.  The relaxed shape alone
/// is insufficient: the exact origin relocation must identify both the
/// archive function and the pointer symbol used by the reviewed anchor.
fn relocated_direct_target_matches(
    archive: &InterfaceCallFact,
    linked: &InterfaceCallFact,
    origin: &LinkUnitOriginFact,
    projected_relocations: &BTreeMap<StructuralCallSite, Vec<StructuralProjectedRelocation>>,
) -> bool {
    let super::InterfaceFactRoot::RelocatedSymbol {
        member,
        symbol,
        addend,
        ..
    } = &archive.root
    else {
        return false;
    };
    if archive.owner != origin.origin_identity()
        || linked.owner != origin.linked_identity()
        || archive.container_depth != 0
        || linked.container_depth != 0
        || archive.loads.len() != 1
        || linked.loads.len() != 1
        || archive.kind != linked.kind
        || archive.jalr_offset != linked.jalr_offset
        || archive.target_offset != linked.target_offset
        || archive.link_register != linked.link_register
        || archive.loads[0].width != linked.loads[0].width
        || archive.loads[0].selector != linked.loads[0].selector
    {
        return false;
    }
    let Some(load_site) = linked.slot_load_site else {
        return false;
    };
    projected_relocations
        .get(&StructuralCallSite::from_identity(
            origin.linked_identity(),
            load_site,
        ))
        .is_some_and(|relocations| {
            relocations.iter().any(|relocation| {
                relocation.origin_member == origin.origin_member
                    && relocation.origin_symbol == origin.symbol
                    && relocation.symbol == *symbol
                    && relocation.addend == *addend
                    && member == &origin.origin_member
            })
        })
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
    if call.target_offset.wrapping_add(call.jalr_offset) != 0
        || slot.offset != binding.offset
        || slot.width != binding.width
        || slot.selector.is_some()
    {
        return false;
    }
    let call_container = &call.loads[..call.container_depth];
    if call_container.len() != contract.container_path.len()
        || !call_container
            .iter()
            .skip(1)
            .zip(contract.container_path.iter().skip(1))
            .all(|(call, contract)| {
                (call.offset, call.width, call.selector)
                    == (contract.offset, contract.width, contract.selector)
            })
    {
        return false;
    }
    match (&contract.root, &call.root) {
        (
            InterfaceRootSelector::RelocatedSymbol {
                member,
                symbol,
                addend,
                addressing,
            },
            super::InterfaceFactRoot::RelocatedSymbol {
                member: call_member,
                symbol: call_symbol,
                addend: call_addend,
                addressing: call_addressing,
                ..
            },
        ) => {
            crate::interfaces::facts::same_step_shape(call_container, &contract.container_path)
                && member == call_member
                && symbol == call_symbol
                && addend == call_addend
                && addressing == call_addressing
        }
        (
            InterfaceRootSelector::FunctionArgument { argument },
            super::InterfaceFactRoot::FunctionArgument {
                argument: call_argument,
                ..
            },
        ) => {
            crate::interfaces::facts::same_step_shape(call_container, &contract.container_path)
                && argument == call_argument
        }
        (InterfaceRootSelector::AbsoluteAddress { address }, _) => {
            let Some(first_contract_step) = contract.container_path.first() else {
                return false;
            };
            if call_container.first().is_none_or(|step| {
                step.width != first_contract_step.width
                    || step.selector != first_contract_step.selector
            }) {
                return false;
            }
            let Some(pointer_cell) = address.checked_add_signed(first_contract_step.offset) else {
                return false;
            };
            // Project inventory may contain unrelated linked test/oracle
            // definitions of the same pointer symbol. Guards and source
            // ownership select the candidate that can satisfy this anchor.
            call.root_linkage
                .candidates
                .iter()
                .filter(|candidate| {
                    candidate.address == pointer_cell
                        && facts
                            .artifact(candidate.artifact)
                            .is_some_and(|artifact| artifact.sources.contains(&contract.source))
                        && (candidate.kind == "data"
                            || candidate.kind == "unknown"
                                && facts.tables.iter().any(|table| {
                                    table.artifact == candidate.artifact
                                        && matches!(
                                            table.root,
                                            super::InterfaceFactRoot::AbsoluteAddress {
                                                address: table_address, ..
                                            } if table_address == *address
                                        )
                                        && crate::interfaces::facts::same_step_shape(
                                            &table.container_path,
                                            &contract.container_path,
                                        )
                                }))
                })
                .count()
                == 1
        }
        _ => false,
    }
}

fn same_indirect_target_shape(left: &InterfaceCallFact, right: &InterfaceCallFact) -> bool {
    left.kind == right.kind
        && left.target_offset == right.target_offset
        && left.link_register == right.link_register
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
        InterfaceRootLinkageFact, InterfaceSymbolLocationFact, InterfaceTableFact,
    };

    fn call(
        artifact: usize,
        member: Option<&str>,
        function_address: u32,
        site: u32,
    ) -> InterfaceCallFact {
        InterfaceCallFact {
            owner: crate::artifact::CodeIdentity::Symbol {
                artifact_sha256: if member.is_some() { "11" } else { "22" }.repeat(32),
                location: crate::SymbolLocation {
                    object: if member.is_some() {
                        crate::ObjectLocation::ArchiveMember { ordinal: 0 }
                    } else {
                        crate::ObjectLocation::Standalone
                    },
                    table: crate::ArtifactSymbolTable::Static,
                    index: 1,
                },
            },
            link_register: 1,
            target_offset: 0,
            artifact,
            member: member.map(str::to_owned),
            function: "post_event".to_owned(),
            function_address,
            site,
            slot_load_site: Some(site - 2),
            kind: "call".to_owned(),
            root: InterfaceFactRoot::RelocatedSymbol {
                reference: open_radio_vendor_contracts::SymbolReference::Unknown {
                    reason: "fixture lacks captured relocation".into(),
                },
                member: member.map(str::to_owned),
                symbol: "g_services".to_owned(),
                addend: 0,
                addressing: "absolute".to_owned(),
            },
            loads: vec![
                InterfaceFactStep {
                    site: None,
                    offset: 0,
                    width: 32,
                    selector: None,
                },
                InterfaceFactStep {
                    site: None,
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
                    location: crate::SymbolLocation {
                        object: crate::ObjectLocation::Standalone,
                        table: crate::ArtifactSymbolTable::Static,
                        index: 1,
                    },
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
            template: None,
            template_overrides: Vec::new(),
            source: "rom".to_owned(),
            root: InterfaceRootSelector::AbsoluteAddress { address: 0x2000 },
            container_path: vec![InterfaceFactStep {
                site: None,
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
            decode_blockers: Vec::new(),
            analysis_failures: Vec::new(),
            limits: Default::default(),
            gaps: Vec::new(),
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
    fn absolute_linker_symbol_is_accepted_only_with_an_observed_table_fact() {
        let mut call = call(1, Some("event.o"), 0, 0x40);
        call.root_linkage.candidates[0].kind = "unknown".to_owned();
        let mut facts = facts();

        assert!(!archive_call_matches_binding(
            &call,
            &binding(),
            &contract(),
            &facts,
        ));

        facts.tables.push(InterfaceTableFact {
            artifact: 0,
            root: InterfaceFactRoot::AbsoluteAddress {
                address: 0x2000,
                data_address: open_radio_vendor_contracts::DataAddressResolution::Unknown {
                    reason: open_radio_vendor_contracts::DataAddressGap::NotAnalyzed,
                },
            },
            container_path: vec![InterfaceFactStep {
                site: None,
                offset: -0x100,
                width: 32,
                selector: None,
            }],
            slots: Vec::new(),
            functions: BTreeSet::new(),
        });
        assert!(archive_call_matches_binding(
            &call,
            &binding(),
            &contract(),
            &facts,
        ));
    }

    #[test]
    fn exact_relocated_callback_cell_matches_without_an_absolute_fixture() {
        let call = call(1, Some("event.o"), 0, 0x40);
        let mut contract = contract();
        contract.root = InterfaceRootSelector::RelocatedSymbol {
            member: Some("event.o".to_owned()),
            symbol: "g_services".to_owned(),
            addend: 0,
            addressing: "absolute".to_owned(),
        };
        contract.container_path = vec![InterfaceFactStep {
            site: None,
            offset: 0,
            width: 32,
            selector: None,
        }];

        assert!(archive_call_matches_binding(
            &call,
            &binding(),
            &contract,
            &facts(),
        ));

        contract.root = InterfaceRootSelector::RelocatedSymbol {
            member: Some("other.o".to_owned()),
            symbol: "g_services".to_owned(),
            addend: 0,
            addressing: "absolute".to_owned(),
        };
        assert!(!archive_call_matches_binding(
            &call,
            &binding(),
            &contract,
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
                location: crate::SymbolLocation {
                    object: crate::ObjectLocation::Standalone,
                    table: crate::ArtifactSymbolTable::Static,
                    index: 1,
                },
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
    fn reviewed_nested_shape_keeps_sites_separate_and_rejects_shifted_target() {
        let mut observed = call(1, Some("event.o"), 0, 0x40);
        let mut reviewed = contract();
        reviewed.root = InterfaceRootSelector::RelocatedSymbol {
            member: Some("event.o".into()),
            symbol: "g_services".into(),
            addend: 0,
            addressing: "absolute".into(),
        };
        let intermediate = InterfaceFactStep {
            site: Some(0x20),
            offset: 4,
            width: 32,
            selector: None,
        };
        observed.loads.insert(1, intermediate);
        observed.container_depth = 2;
        reviewed.container_path[0].offset = 0;
        reviewed.container_path.push(InterfaceFactStep {
            site: None,
            ..intermediate
        });
        assert!(archive_call_matches_binding(
            &observed,
            &binding(),
            &reviewed,
            &facts()
        ));
        observed.target_offset = 4;
        assert!(!archive_call_matches_binding(
            &observed,
            &binding(),
            &reviewed,
            &facts()
        ));
        observed.jalr_offset = -4;
        assert!(archive_call_matches_binding(
            &observed,
            &binding(),
            &reviewed,
            &facts()
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

    #[test]
    fn exact_origin_relocation_allows_a_linker_changed_pointer_cell_immediate() {
        let mut archive = call(1, Some("event.o"), 0, 0x40);
        archive.loads.truncate(1);
        archive.container_depth = 0;
        archive.slot_offset = Some(0);
        archive.slot_load_site = Some(0x3c);
        let mut linked = call(2, None, 0x1000, 0x1062);
        linked.loads.truncate(1);
        linked.loads[0].offset = 0x160;
        linked.container_depth = 0;
        linked.slot_offset = Some(0x160);
        linked.slot_load_site = Some(0x105c);
        linked.root = InterfaceFactRoot::AbsoluteAddress {
            data_address: open_radio_vendor_contracts::DataAddressResolution::Unknown {
                reason: open_radio_vendor_contracts::DataAddressGap::NotAnalyzed,
            },
            address: 0x1008_8000,
        };
        let origin = LinkUnitOriginFact {
            linked_location: crate::SymbolLocation {
                object: crate::ObjectLocation::Standalone,
                table: crate::ArtifactSymbolTable::Static,
                index: 1,
            },
            origin_location: crate::SymbolLocation {
                object: crate::ObjectLocation::ArchiveMember { ordinal: 0 },
                table: crate::ArtifactSymbolTable::Static,
                index: 1,
            },
            linked_sources: vec!["rom".to_owned()],
            linked_artifact_sha256: "22".repeat(32),
            linked_member: None,
            symbol: "post_event".to_owned(),
            linked_address: 0x1000,
            kind: "text".to_owned(),
            origin_sources: vec!["rom".to_owned()],
            origin_artifact_sha256: "11".repeat(32),
            origin_member: Some("event.o".to_owned()),
            origin_address: 0,
        };
        let relocations = BTreeMap::from([(
            StructuralCallSite::from_identity(origin.linked_identity(), 0x105c),
            vec![StructuralProjectedRelocation {
                reference: open_radio_vendor_contracts::SymbolReference::Unknown {
                    reason: "synthetic fixture".to_owned(),
                },
                origin_member: Some("event.o".to_owned()),
                origin_symbol: "post_event".to_owned(),
                origin_offsets: vec![0],
                kind: crate::artifact::RelocationKind::Lo12I,
                symbol: "g_services".to_owned(),
                addend: 0,
                correspondence: "same-shape",
            }],
        )]);

        assert!(!same_indirect_target_shape(&archive, &linked));
        assert!(relocated_direct_target_matches(
            &archive,
            &linked,
            &origin,
            &relocations,
        ));

        let mut wrong = relocations;
        wrong.values_mut().next().unwrap()[0].symbol = "other_cell".to_owned();
        assert!(!relocated_direct_target_matches(
            &archive, &linked, &origin, &wrong,
        ));
    }
}
