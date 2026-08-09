//! Project-level pointer-table and indirect-call discovery facts.

use std::{collections::BTreeSet, path::PathBuf};

use crate::{
    Result, artifact,
    interface_discovery::{InterfaceCallCandidate, InterfaceRoot, discover_interface_calls},
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
pub(crate) struct InterfaceDecodeFailure {
    pub(crate) artifact: usize,
    pub(crate) member: Option<String>,
    pub(crate) function: String,
    pub(crate) error: String,
}

pub(crate) struct ProjectInterfaceDiscovery {
    pub(crate) linkage: ProjectLinkageInventory,
    pub(crate) functions: Vec<usize>,
    pub(crate) calls: Vec<DiscoveredInterfaceCall>,
    pub(crate) failures: Vec<InterfaceDecodeFailure>,
}

pub(crate) fn discover_project_interfaces(
    inputs: &[(String, PathBuf)],
    options: &ProjectInterfaceDiscoveryOptions,
) -> Result<ProjectInterfaceDiscovery> {
    let linkage = build_project_linkage_inventory(inputs)?;
    let mut functions = Vec::with_capacity(linkage.artifacts.len());
    let mut calls = Vec::new();
    let mut failures = Vec::new();
    for (artifact_index, artifact) in linkage.artifacts.iter().enumerate() {
        let symbols = artifact::load_code_symbols(
            &artifact.path,
            &options.name_prefix,
            artifact::CodeSymbolSelection::All,
        )?;
        functions.push(symbols.len());
        for symbol in symbols {
            match discover_interface_calls(&symbol) {
                Ok(discovered) => calls.extend(
                    discovered
                        .into_iter()
                        .filter(|call| !options.tables_only || !call.target.loads.is_empty())
                        .map(|call| DiscoveredInterfaceCall {
                            artifact: artifact_index,
                            call,
                        }),
                ),
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
    Ok(ProjectInterfaceDiscovery {
        linkage,
        functions,
        calls,
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
