//! Project-level pointer-table and indirect-call discovery facts.

use std::{collections::BTreeSet, path::PathBuf};

use crate::{
    Result, artifact,
    interface_discovery::{InterfaceCallCandidate, InterfaceRoot, InterfaceSlotAssignment},
    interfaces::InterfaceGapFact,
};

use super::{LinkageSymbolLocation, ProjectLinkageInventory, build_project_linkage_inventory};

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
    pub(crate) owner: crate::artifact::CodeIdentity,
    pub(crate) artifact: usize,
    pub(crate) member: Option<String>,
    pub(crate) function: String,
    pub(crate) error: String,
}

#[derive(Clone)]
pub(crate) struct InterfaceDecodeBlocker {
    pub(crate) owner: crate::artifact::CodeIdentity,
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
    pub(crate) limits: crate::interface_discovery::InterfaceDiscoveryLimits,
    pub(crate) gaps: Vec<InterfaceGapFact>,
    pub(crate) linkage: ProjectLinkageInventory,
    pub(crate) functions: Vec<usize>,
    pub(crate) reviewed_boundaries: Vec<usize>,
    pub(crate) calls: Vec<DiscoveredInterfaceCall>,
    pub(crate) assignments: Vec<DiscoveredInterfaceAssignment>,
    pub(crate) decode_blockers: Vec<InterfaceDecodeBlocker>,
    pub(crate) failures: Vec<InterfaceDecodeFailure>,
}

pub(crate) fn discover_project_interfaces(
    captures: &crate::source_set::CapturedSourceSet,
    inputs: &[(String, PathBuf)],
    options: &ProjectInterfaceDiscoveryOptions,
    effective_code: Option<&super::EffectiveCodeCatalog>,
) -> Result<ProjectInterfaceDiscovery> {
    let linkage = build_project_linkage_inventory(captures, inputs)?;
    let mut functions = Vec::with_capacity(linkage.artifacts.len());
    let mut reviewed_boundaries = Vec::with_capacity(linkage.artifacts.len());
    let mut calls = Vec::new();
    let mut assignments = Vec::new();
    let mut decode_blockers = Vec::new();
    let mut failures = Vec::new();
    let mut gaps = Vec::new();
    for (artifact_index, artifact) in linkage.artifacts.iter().enumerate() {
        let source = artifact.sources.first().ok_or_else(|| {
            crate::Error::invalid(format!(
                "interface artifact {} has no logical source",
                artifact.path.display()
            ))
        })?;
        let capture = captures.artifact(&artifact.path)?;
        let (symbols, reviewed_count) = match effective_code {
            Some(catalog) => {
                let loaded = catalog.load_symbols(
                    source,
                    &artifact.path,
                    capture,
                    &options.name_prefix,
                    artifact::CodeSymbolSelection::All,
                )?;
                (loaded.symbols, loaded.reviewed_boundaries)
            }
            None => (
                capture
                    .code_symbols(&options.name_prefix, artifact::CodeSymbolSelection::All)?
                    .into_iter()
                    .map(|symbol| symbol.definition.clone())
                    .collect(),
                0,
            ),
        };
        let data_symbols = capture.data_symbols()?;
        functions.push(symbols.len());
        reviewed_boundaries.push(reviewed_count);
        for symbol in symbols {
            match crate::interface_discovery::discover_interface_calls_with_data_symbols(
                &symbol,
                data_symbols,
            ) {
                Ok(discovered) => {
                    gaps.extend(
                        discovered
                            .gaps
                            .into_iter()
                            .map(|evidence| InterfaceGapFact {
                                artifact: artifact_index,
                                evidence,
                            }),
                    );
                    decode_blockers.extend(discovered.decode_blockers.into_iter().map(|blocker| {
                        InterfaceDecodeBlocker {
                            owner: symbol.identity.clone(),
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
                    owner: symbol.identity.clone(),
                    artifact: artifact_index,
                    member: symbol.member,
                    function: symbol.name,
                    error: error.to_string(),
                }),
            }
        }
    }
    calls.sort_by(|left, right| (left.artifact, &left.call).cmp(&(right.artifact, &right.call)));
    calls.dedup_by(|left, right| left.artifact == right.artifact && left.call == right.call);
    assignments.sort_by(|left, right| {
        (left.artifact, &left.assignment).cmp(&(right.artifact, &right.assignment))
    });
    Ok(ProjectInterfaceDiscovery {
        limits: Default::default(),
        gaps,
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
            InterfaceRoot::RelocatedSymbol { reference, .. } => match reference {
                open_radio_vendor_contracts::SymbolReference::Captured {
                    artifact_sha256,
                    location,
                    ..
                } => {
                    artifact_sha256 == &discovery.linkage.artifacts[symbol.artifact].sha256
                        && *location == symbol.location
                }
                open_radio_vendor_contracts::SymbolReference::Unknown { .. } => false,
            },
            InterfaceRoot::AbsoluteAddress {
                address,
                data_address,
            } => {
                symbol.fact.definition.is_definition()
                    && ((data_address.candidates().is_empty()
                        && symbol.fact.address == u64::from(*address))
                        || data_address.candidates().iter().any(|candidate| {
                            candidate.identity
                                == crate::artifact::DataIdentity::Symbol {
                                    artifact_sha256: discovery.linkage.artifacts[symbol.artifact]
                                        .sha256
                                        .clone(),
                                    location: symbol.location,
                                }
                        }))
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
