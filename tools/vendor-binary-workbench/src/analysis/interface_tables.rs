//! Project-level pointer-table and indirect-call discovery facts.

use std::{collections::BTreeSet, path::PathBuf};

use crate::{
    Result, artifact,
    interface_discovery::{
        InterfaceCallCandidate, InterfaceRoot, InterfaceSlotAssignment, discover_interface_calls,
    },
};

use super::{LinkageSymbolLocation, ProjectLinkageInventory, build_project_linkage_inventory};

fn persistent_call_identity(
    discovered: &DiscoveredInterfaceCall,
) -> (usize, InterfaceCallCandidate) {
    let mut call = discovered.call.clone();
    // The persistent facts retain the final slot-load site, but intentionally
    // compare container shape independently of the path-specific instruction
    // that loaded each intermediate pointer. Normalize exactly that omitted
    // provenance before deduplicating so the strict reader never receives two
    // records that deserialize to the same fact.
    let container_len = call.target.loads.len().saturating_sub(1);
    for load in &mut call.target.loads[..container_len] {
        load.site = 0;
    }
    for argument in &mut call.arguments {
        if let crate::interface_discovery::InterfaceArgumentValue::Pointer(pointer) = argument {
            for load in &mut pointer.loads {
                load.site = 0;
            }
        }
    }
    if matches!(
        call.kind,
        crate::interface_discovery::InterfaceCallKind::LinkedJump(_)
    ) {
        // The stable fact vocabulary records this class as `linked-jump`; the
        // exact architectural link register remains presentation evidence.
        call.kind = crate::interface_discovery::InterfaceCallKind::LinkedJump(0);
    }
    (discovered.artifact, call)
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ProjectInterfaceDiscoveryOptions {
    pub(crate) name_prefix: String,
    pub(crate) tables_only: bool,
}

#[derive(Clone)]
pub(crate) struct DiscoveredInterfaceCall {
    pub(crate) artifact: usize,
    pub(crate) call: InterfaceCallCandidate,
}

#[derive(Clone)]
pub(crate) struct DiscoveredInterfaceAssignment {
    pub(crate) artifact: usize,
    pub(crate) assignment: InterfaceSlotAssignment,
}

#[derive(Clone)]
pub(crate) struct InterfaceDecodeFailure {
    pub(crate) artifact: usize,
    pub(crate) member: Option<String>,
    pub(crate) function: String,
    pub(crate) error: String,
}

#[derive(Clone)]
pub(crate) struct InterfaceDecodeBlocker {
    pub(crate) artifact: usize,
    pub(crate) member: Option<String>,
    pub(crate) function: String,
    pub(crate) address: u64,
    pub(crate) width: u8,
    pub(crate) raw: u32,
    pub(crate) class: &'static str,
    pub(crate) linear_control_flow: bool,
}

pub(crate) struct ProjectInterfaceDiscovery {
    pub(crate) linkage: ProjectLinkageInventory,
    pub(crate) functions: Vec<usize>,
    pub(crate) reviewed_boundaries: Vec<usize>,
    pub(crate) calls: Vec<DiscoveredInterfaceCall>,
    pub(crate) assignments: Vec<DiscoveredInterfaceAssignment>,
    pub(crate) decode_blockers: Vec<InterfaceDecodeBlocker>,
    pub(crate) failures: Vec<InterfaceDecodeFailure>,
}

pub(crate) fn discover_project_interfaces(
    inputs: &[(String, PathBuf)],
    options: &ProjectInterfaceDiscoveryOptions,
    effective_code: Option<&super::EffectiveCodeCatalog>,
) -> Result<ProjectInterfaceDiscovery> {
    let linkage = build_project_linkage_inventory(inputs)?;
    let mut functions = Vec::with_capacity(linkage.artifacts.len());
    let mut reviewed_boundaries = Vec::with_capacity(linkage.artifacts.len());
    let mut calls = Vec::new();
    let mut assignments = Vec::new();
    let mut decode_blockers = Vec::new();
    let mut failures = Vec::new();
    for (artifact_index, artifact) in linkage.artifacts.iter().enumerate() {
        let source = artifact.sources.first().ok_or_else(|| {
            crate::Error::invalid(format!(
                "interface artifact {} has no logical source",
                artifact.path.display()
            ))
        })?;
        let (symbols, reviewed_count) = match effective_code {
            Some(catalog) => {
                let loaded = catalog.load_symbols(
                    source,
                    &artifact.path,
                    &options.name_prefix,
                    artifact::CodeSymbolSelection::All,
                )?;
                (loaded.symbols, loaded.reviewed_boundaries)
            }
            None => (
                artifact::load_code_symbols(
                    &artifact.path,
                    &options.name_prefix,
                    artifact::CodeSymbolSelection::All,
                )?,
                0,
            ),
        };
        functions.push(symbols.len());
        reviewed_boundaries.push(reviewed_count);
        for symbol in symbols {
            match discover_interface_calls(&symbol) {
                Ok(discovered) => {
                    decode_blockers.extend(discovered.decode_blockers.into_iter().map(|blocker| {
                        InterfaceDecodeBlocker {
                            artifact: artifact_index,
                            member: symbol.member.clone(),
                            function: symbol.name.clone(),
                            address: blocker.address,
                            width: blocker.width,
                            raw: blocker.raw,
                            class: blocker.class.as_str(),
                            linear_control_flow: blocker.linear_control_flow,
                        }
                    }));
                    calls.extend(
                        discovered
                            .calls
                            .into_iter()
                            .filter(|call| !options.tables_only || !call.target.loads.is_empty())
                            .map(|call| DiscoveredInterfaceCall {
                                artifact: artifact_index,
                                call,
                            }),
                    );
                    assignments.extend(discovered.assignments.into_iter().map(|assignment| {
                        DiscoveredInterfaceAssignment {
                            artifact: artifact_index,
                            assignment,
                        }
                    }));
                }
                Err(error) => failures.push(InterfaceDecodeFailure {
                    artifact: artifact_index,
                    member: symbol.member,
                    function: symbol.name,
                    error: error.to_string(),
                }),
            }
        }
    }
    calls.sort_by(|left, right| (left.artifact, &left.call).cmp(&(right.artifact, &right.call)));
    calls.dedup_by(|left, right| persistent_call_identity(left) == persistent_call_identity(right));
    assignments.sort_by(|left, right| {
        (left.artifact, &left.assignment).cmp(&(right.artifact, &right.assignment))
    });
    Ok(ProjectInterfaceDiscovery {
        linkage,
        functions,
        reviewed_boundaries,
        calls,
        assignments,
        decode_blockers,
        failures,
    })
}

#[derive(Default)]
pub(crate) struct InterfaceRootLinkage {
    pub(crate) symbols: BTreeSet<String>,
    pub(crate) resolutions: BTreeSet<&'static str>,
    pub(crate) candidates: BTreeSet<LinkageSymbolLocation>,
}

pub(crate) fn interface_root_linkage(
    discovery: &ProjectInterfaceDiscovery,
    artifact: usize,
    root: &InterfaceRoot,
) -> InterfaceRootLinkage {
    let mut result = InterfaceRootLinkage::default();
    for symbol in &discovery.linkage.symbols {
        if symbol.artifact != artifact {
            continue;
        }
        let matches = match root {
            InterfaceRoot::RelocatedSymbol {
                member,
                symbol: name,
                ..
            } => &symbol.member == member && &symbol.fact.name == name,
            InterfaceRoot::AbsoluteAddress { address } => {
                symbol.fact.definition.is_definition() && symbol.fact.address == u64::from(*address)
            }
            InterfaceRoot::FunctionArgument { .. } => false,
        };
        if matches {
            result.symbols.insert(symbol.fact.name.clone());
            result.resolutions.insert(symbol.resolution.label());
            result.candidates.extend(symbol.candidates.iter().cloned());
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interface_discovery::{
        InterfaceCallKind, InterfaceLoad, InterfacePointer, InterfaceRoot,
    };

    fn call(container_site: u32, slot_site: u32) -> DiscoveredInterfaceCall {
        DiscoveredInterfaceCall {
            artifact: 0,
            call: InterfaceCallCandidate {
                member: Some("event.o".to_owned()),
                function: "dispatch".to_owned(),
                function_address: 0x1000,
                site: 0x1020,
                kind: InterfaceCallKind::Call,
                target: InterfacePointer {
                    root: InterfaceRoot::FunctionArgument { index: 0 },
                    loads: vec![
                        InterfaceLoad {
                            site: container_site,
                            offset: 4,
                            width: 32,
                            selector: None,
                        },
                        InterfaceLoad {
                            site: slot_site,
                            offset: 8,
                            width: 32,
                            selector: None,
                        },
                    ],
                    post_offset: 0,
                },
                jalr_offset: 0,
                arguments: Vec::new(),
            },
        }
    }

    #[test]
    fn persistent_identity_ignores_only_unstored_container_load_sites() {
        assert_eq!(
            persistent_call_identity(&call(0x1004, 0x101c)),
            persistent_call_identity(&call(0x1008, 0x101c)),
        );
        assert_ne!(
            persistent_call_identity(&call(0x1004, 0x101c)),
            persistent_call_identity(&call(0x1004, 0x1018)),
        );
    }
}
