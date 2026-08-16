//! Typed projection for generated interface discovery facts.

use crate::{Result, artifacts};

use super::*;

pub(super) fn parse(input: &str) -> Result<InterfaceFacts> {
    let document = artifacts::parse_interface_facts(input)?;
    let facts = InterfaceFacts {
        artifacts: document
            .artifacts
            .into_iter()
            .map(|artifact| InterfaceFactArtifact {
                index: artifact.index,
                sources: artifact.sources.into_iter().collect(),
                sha256: Some(artifact.sha256),
            })
            .collect(),
        tables: document
            .table_candidates
            .into_iter()
            .map(|table| InterfaceTableFact {
                artifact: table.artifact,
                root: root(table.root),
                container_path: table.container_path.into_iter().map(step).collect(),
                slots: table
                    .slots
                    .into_iter()
                    .map(|slot| InterfaceFactSlot {
                        offset: slot.offset,
                        width: slot.width,
                        selector: slot.selector.map(selector),
                        functions: slot.functions.into_iter().collect(),
                    })
                    .collect(),
                functions: table.functions.into_iter().collect(),
            })
            .collect(),
        calls: document
            .calls
            .into_iter()
            .map(|call| {
                if call.root_linkage.mode != "association-only" {
                    return Err(crate::Error::invalid(format!(
                        "unsupported interface root-linkage mode {:?}",
                        call.root_linkage.mode
                    )));
                }
                let slot_load_site = call.target.loads.last().and_then(|load| load.site);
                let root_linkage = InterfaceRootLinkageFact {
                    symbols: call.root_linkage.symbols,
                    resolutions: call.root_linkage.resolutions,
                    candidates: call
                        .root_linkage
                        .candidates
                        .into_iter()
                        .map(|candidate| {
                            Ok(InterfaceSymbolLocationFact {
                                artifact: candidate.artifact,
                                member: candidate.member,
                                address: crate::parse_u32(&candidate.address).ok_or_else(|| {
                                    crate::Error::invalid(format!(
                                        "invalid interface root-linkage address {:?}",
                                        candidate.address
                                    ))
                                })?,
                                kind: candidate.kind,
                            })
                        })
                        .collect::<Result<Vec<_>>>()?,
                };
                Ok(InterfaceCallFact {
                    artifact: call.artifact,
                    member: call.member,
                    function: call.function,
                    function_address: call.function_address,
                    site: call.site,
                    slot_load_site,
                    kind: call.kind,
                    root: root(call.target.root),
                    loads: call.target.loads.into_iter().map(step).collect(),
                    container_depth: call.target.container_depth,
                    slot_offset: call.target.slot_offset,
                    jalr_offset: call.target.jalr_offset,
                    arguments: call.arguments.into_iter().map(argument).collect(),
                    root_linkage,
                })
            })
            .collect::<Result<Vec<_>>>()?,
        assignments: document
            .assignments
            .into_iter()
            .map(|assignment| InterfaceAssignmentFact {
                artifact: assignment.artifact,
                member: assignment.member,
                function: assignment.function,
                function_address: assignment.function_address,
                site: assignment.site,
                root: root(assignment.root),
                container_path: assignment.container_path.into_iter().map(step).collect(),
                offset: assignment.offset,
                width: assignment.width,
                target: root(assignment.target),
            })
            .collect(),
    };
    super::validate::validate(&facts)?;
    Ok(facts)
}

fn root(root: artifacts::StoredInterfaceRoot) -> InterfaceFactRoot {
    match root {
        artifacts::StoredInterfaceRoot::RelocatedSymbol {
            member,
            symbol,
            addend,
            addressing,
            ..
        } => InterfaceFactRoot::RelocatedSymbol {
            member,
            symbol,
            addend,
            addressing,
        },
        artifacts::StoredInterfaceRoot::FunctionArgument { argument, .. } => {
            InterfaceFactRoot::FunctionArgument { argument }
        }
        artifacts::StoredInterfaceRoot::AbsoluteAddress { address, .. } => {
            InterfaceFactRoot::AbsoluteAddress { address }
        }
    }
}

fn step(step: artifacts::StoredInterfaceStep) -> InterfaceFactStep {
    InterfaceFactStep {
        offset: step.offset,
        width: step.width,
        selector: step.selector.map(selector),
    }
}

fn selector(selector: artifacts::StoredInterfaceSelector) -> InterfaceFactSelector {
    InterfaceFactSelector {
        argument: selector.argument,
        scale: selector.scale,
        addend: selector.addend,
    }
}

fn argument(argument: artifacts::StoredInterfaceArgument) -> InterfaceArgumentFact {
    match argument {
        artifacts::StoredInterfaceArgument::Unknown { index } => InterfaceArgumentFact {
            index,
            kind: "unknown".to_owned(),
            expression: "?".to_owned(),
        },
        artifacts::StoredInterfaceArgument::Constant { index, value } => InterfaceArgumentFact {
            index,
            kind: "constant".to_owned(),
            expression: format!("{value:#010x}"),
        },
        artifacts::StoredInterfaceArgument::PointerProvenance {
            index, canonical, ..
        } => InterfaceArgumentFact {
            index,
            kind: "pointer-provenance".to_owned(),
            expression: canonical,
        },
    }
}
