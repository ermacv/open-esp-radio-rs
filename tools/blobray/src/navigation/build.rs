//! Deterministic cross-report join construction.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use super::model::{
    ArtifactDocument, IDENTITY_SCHEME, InterfaceCallObservation, InterfaceRootObservation,
    InventoryObservation, IrObservation, NavigationDocument, NavigationIdentity,
    ProjectCallLinkDocument, SCHEMA_VERSION, SummaryDocument, SymbolDocument, SymbolKey, address,
    artifact, input, symbol,
};
use crate::{
    Result,
    artifacts::{LinkedIrStoredDocument, StoredInterfaceFacts, StoredSymbolInventory},
    error::BlobrayError,
    interfaces::{InterfaceFactRoot, InterfaceFacts},
    project::ProjectSpec,
};

pub(crate) fn build(project: &ProjectSpec) -> Result<NavigationDocument> {
    let navigation_output = &project
        .navigation_index
        .as_ref()
        .ok_or("project navigation requires [analysis.navigation]")
        .map_err(crate::Error::invalid)?
        .output;
    let symbols_spec = project
        .symbol_inventory
        .as_ref()
        .ok_or("project navigation requires [analysis.symbols]")
        .map_err(crate::Error::invalid)?;
    let inventory: StoredSymbolInventory = read_artifact(
        &symbols_spec.output,
        "symbol inventory",
        crate::artifacts::parse_symbol_inventory,
    )?;

    let mut inputs = vec![input(
        "symbol-inventory",
        "symbols".to_owned(),
        &symbols_spec.output,
        navigation_output,
    )?];
    let mut artifacts = BTreeMap::<String, ArtifactDocument>::new();
    let mut inventory_artifacts = BTreeMap::new();
    for item in inventory.artifacts {
        let document = artifact(&mut artifacts, &item.artifact.sha256);
        document.paths.insert(item.artifact.path);
        document.sources.extend(item.sources.iter().cloned());
        inventory_artifacts.insert(item.index, (item.artifact.sha256, item.sources));
    }

    let mut symbols = BTreeMap::<SymbolKey, SymbolDocument>::new();
    for fact in inventory.symbols {
        let (sha256, sources) = inventory_artifacts
            .get(&fact.artifact)
            .ok_or_else(|| {
                format!(
                    "symbol refers to unknown inventory artifact {}",
                    fact.artifact
                )
            })
            .map_err(crate::Error::invalid)?;
        let key = SymbolKey {
            occurrence: NavigationIdentity::Symbol {
                artifact_sha256: sha256.clone(),
                location: fact.location,
            },
            artifact_sha256: sha256.clone(),
            member: fact.member,
            name: fact.name,
            object_address: address(&fact.address, "symbol inventory")?,
        };
        let document = symbol(&mut symbols, &key);
        document.sources.extend(sources.iter().cloned());
        document.inventory.insert(InventoryObservation {
            table: fact.table,
            definition: fact.definition,
            kind: fact.kind,
            resolution: fact.resolution,
        });
    }

    let mut project_definitions = BTreeMap::<String, BTreeSet<String>>::new();
    let mut pending_project_calls = BTreeSet::new();
    add_linked_ir(
        project,
        navigation_output,
        &mut inputs,
        &mut artifacts,
        &mut symbols,
        &mut project_definitions,
        &mut pending_project_calls,
    )?;
    let project_calls = project_call_links(&project_definitions, pending_project_calls);
    let (unmatched_interface_roots, interface_observations) = add_interfaces(
        project,
        navigation_output,
        &mut inputs,
        &mut artifacts,
        &mut symbols,
    )?;

    let mut artifacts = artifacts.into_values().collect::<Vec<_>>();
    artifacts.sort_by(|left, right| left.sha256.cmp(&right.sha256));
    let symbols = symbols.into_values().collect::<Vec<_>>();
    let summary = SummaryDocument {
        artifacts: artifacts.len(),
        symbols: symbols.len(),
        inventory_symbols: symbols
            .iter()
            .filter(|symbol| !symbol.inventory.is_empty())
            .count(),
        linked_ir_functions: symbols
            .iter()
            .filter(|symbol| !symbol.linked_ir.is_empty())
            .count(),
        interface_callers: symbols
            .iter()
            .filter(|symbol| !symbol.interface_calls.is_empty())
            .count(),
        interface_roots: symbols
            .iter()
            .filter(|symbol| !symbol.interface_roots.is_empty())
            .count(),
        unmatched_interface_roots,
        project_call_links: project_calls.len(),
        unique_project_calls: project_calls
            .iter()
            .filter(|call| call.status == "unique")
            .count(),
        ambiguous_project_calls: project_calls
            .iter()
            .filter(|call| call.status == "ambiguous")
            .count(),
        unresolved_project_calls: project_calls
            .iter()
            .filter(|call| call.status == "unresolved")
            .count(),
    };
    Ok(NavigationDocument {
        interface_observations,
        schema_version: SCHEMA_VERSION,
        command: "project navigation".to_owned(),
        identity_scheme: IDENTITY_SCHEME.to_owned(),
        semantic_claim: false,
        linker_resolution_claim: false,
        inputs,
        artifacts,
        symbols,
        project_calls,
        summary,
    })
}

type PendingProjectCall = (String, Option<u32>, String);

fn add_linked_ir(
    project: &ProjectSpec,
    navigation_output: &Path,
    inputs: &mut Vec<super::model::InputDocument>,
    artifacts: &mut BTreeMap<String, ArtifactDocument>,
    symbols: &mut BTreeMap<SymbolKey, SymbolDocument>,
    project_definitions: &mut BTreeMap<String, BTreeSet<String>>,
    pending_project_calls: &mut BTreeSet<PendingProjectCall>,
) -> Result<()> {
    for profile in &project.ir_profiles {
        let report: LinkedIrStoredDocument =
            crate::artifacts::load_linked_ir_functions(&profile.output)?;
        inputs.push(input(
            "linked-ir",
            profile.id.clone(),
            &profile.output,
            navigation_output,
        )?);
        let mut source_artifacts = BTreeSet::new();
        for item in report.artifacts {
            let document = artifact(artifacts, &item.artifact.sha256);
            document.paths.insert(item.artifact.path);
            document.sources.insert(item.source.clone());
            source_artifacts.insert((item.source.clone(), item.artifact.sha256));
            for companion in item.companions {
                let document = artifact(artifacts, &companion.sha256);
                document.paths.insert(companion.path);
                document.sources.insert(item.source.clone());
                source_artifacts.insert((item.source.clone(), companion.sha256));
            }
        }
        for function in report.functions {
            if function.is_exported() {
                project_definitions
                    .entry(function.symbol.clone())
                    .or_default()
                    .insert(function.identity.clone());
            }
            for call in &function.calls {
                if let Some(project_symbol) = call.project_symbol() {
                    pending_project_calls.insert((
                        function.identity.clone(),
                        call.site,
                        project_symbol.to_owned(),
                    ));
                }
            }
            if !source_artifacts
                .contains(&(function.source.clone(), function.artifact_sha256.clone()))
            {
                return Err(crate::Error::invalid(
                    "navigation function belongs to an undeclared source artifact",
                ));
            }
            let key = SymbolKey {
                occurrence: NavigationIdentity::from_code(
                    function.code_identity,
                    &function.artifact_sha256,
                ),
                artifact_sha256: function.artifact_sha256,
                member: function.member,
                name: function.symbol,
                object_address: function.object_offset,
            };
            let document = symbol(symbols, &key);
            document.sources.insert(function.source);
            document.linked_ir.insert(IrObservation {
                profile: profile.id.clone(),
                identity: function.identity,
                selection: function.selection,
            });
        }
    }
    Ok(())
}

fn project_call_links(
    definitions: &BTreeMap<String, BTreeSet<String>>,
    calls: BTreeSet<PendingProjectCall>,
) -> Vec<ProjectCallLinkDocument> {
    calls
        .into_iter()
        .map(|(caller, site, symbol)| {
            let candidates = definitions
                .get(&symbol)
                .map(|candidates| candidates.iter().cloned().collect::<Vec<_>>())
                .unwrap_or_default();
            let status = match candidates.len() {
                0 => "unresolved",
                1 => "unique",
                _ => "ambiguous",
            };
            ProjectCallLinkDocument {
                caller,
                site,
                symbol,
                status: status.to_owned(),
                candidates,
                linker_resolution_claim: false,
            }
        })
        .collect()
}

fn add_interfaces(
    project: &ProjectSpec,
    navigation_output: &Path,
    inputs: &mut Vec<super::model::InputDocument>,
    artifacts: &mut BTreeMap<String, ArtifactDocument>,
    symbols: &mut BTreeMap<SymbolKey, SymbolDocument>,
) -> Result<(usize, Option<InterfaceFacts>)> {
    let Some(paths) = &project.interfaces else {
        return Ok((0, None));
    };
    let report: StoredInterfaceFacts = read_artifact(
        &paths.facts,
        "interface facts",
        crate::artifacts::parse_interface_facts,
    )?;
    inputs.push(input(
        "interface-facts",
        "interfaces".to_owned(),
        &paths.facts,
        navigation_output,
    )?);
    let mut interface_artifacts = BTreeMap::new();
    for item in &report.artifacts {
        let document = artifact(artifacts, &item.sha256);
        document.paths.insert(item.path.clone());
        document.sources.extend(item.sources.iter().cloned());
        interface_artifacts.insert(item.index, (item.sha256.clone(), item.sources.clone()));
    }
    let observations = InterfaceFacts::from_document(report)?;
    let mut root_index = InterfaceRootIndex::new(symbols.keys());
    let mut unmatched_roots = 0;
    for call in &observations.calls {
        let (sha256, sources) = interface_artifacts
            .get(&call.artifact)
            .ok_or_else(|| {
                format!(
                    "interface call refers to unknown artifact {}",
                    call.artifact
                )
            })
            .map_err(crate::Error::invalid)?;
        let caller_key = SymbolKey {
            occurrence: NavigationIdentity::from_code(call.owner.clone(), sha256),
            artifact_sha256: sha256.clone(),
            member: call.member.clone(),
            name: call.function.clone(),
            object_address: call.function_address,
        };
        {
            let caller = symbol(symbols, &caller_key);
            caller.sources.extend(sources.iter().cloned());
            caller.interface_calls.insert(InterfaceCallObservation {
                owner: call.owner.clone(),
                site: format!("{:#x}", call.site),
                kind: call.kind.clone(),
            });
        }
        // The caller may be absent from inventory and linked IR. Preserve the
        // old incremental join semantics by making the newly observed symbol
        // available to later interface-root lookups too.
        root_index.insert(&caller_key);

        let root_matches = root_index.matches(sha256, &call.root);
        if root_matches.is_empty()
            && !matches!(call.root, InterfaceFactRoot::FunctionArgument { .. })
        {
            unmatched_roots += 1;
        }
        for key in root_matches {
            symbol(symbols, &key)
                .interface_roots
                .insert(InterfaceRootObservation {
                    owner: call.owner.clone(),
                    data_address: match &call.root {
                        InterfaceFactRoot::AbsoluteAddress { data_address, .. } => {
                            Some(data_address.clone())
                        }
                        _ => None,
                    },
                    function: call.function.clone(),
                    site: format!("{:#x}", call.site),
                    kind: interface_root_kind(&call.root).to_owned(),
                });
        }
    }
    Ok((unmatched_roots, Some(observations)))
}

/// Physical lookup for captured relocation entries and data range candidates.
/// Numeric-address lookups remain explicit associations when no range evidence exists.
struct InterfaceRootIndex {
    physical: BTreeMap<NavigationIdentity, SymbolKey>,
    absolute: BTreeMap<(String, u32), BTreeSet<SymbolKey>>,
}

impl InterfaceRootIndex {
    fn new<'a>(symbols: impl Iterator<Item = &'a SymbolKey>) -> Self {
        let mut index = Self {
            physical: BTreeMap::new(),
            absolute: BTreeMap::new(),
        };
        for symbol in symbols {
            index.insert(symbol);
        }
        index
    }

    fn insert(&mut self, symbol: &SymbolKey) {
        self.physical
            .insert(symbol.occurrence.clone(), symbol.clone());
        self.absolute
            .entry((symbol.artifact_sha256.clone(), symbol.object_address))
            .or_default()
            .insert(symbol.clone());
    }

    fn matches(&self, artifact_sha256: &str, root: &InterfaceFactRoot) -> Vec<SymbolKey> {
        let mut matches = BTreeSet::new();
        match root {
            InterfaceFactRoot::RelocatedSymbol { reference, .. } => {
                if let open_radio_vendor_contracts::SymbolReference::Captured {
                    artifact_sha256,
                    location,
                    ..
                } = reference
                    && let Some(found) = self.physical.get(&NavigationIdentity::Symbol {
                        artifact_sha256: artifact_sha256.clone(),
                        location: *location,
                    })
                {
                    matches.insert(found.clone());
                }
            }
            InterfaceFactRoot::AbsoluteAddress {
                address,
                data_address,
            } => {
                for candidate in data_address.candidates() {
                    if let open_radio_vendor_contracts::DataIdentity::Symbol {
                        artifact_sha256,
                        location,
                    } = &candidate.identity
                        && let Some(found) = self.physical.get(&NavigationIdentity::Symbol {
                            artifact_sha256: artifact_sha256.clone(),
                            location: *location,
                        })
                    {
                        matches.insert(found.clone());
                    }
                }
                if data_address.candidates().is_empty()
                    && *address != 0
                    && let Some(found) = self.absolute.get(&(artifact_sha256.to_owned(), *address))
                {
                    matches.extend(found.iter().cloned());
                }
            }
            InterfaceFactRoot::FunctionArgument { .. } => {}
        }
        matches.into_iter().collect()
    }
}

fn interface_root_kind(root: &InterfaceFactRoot) -> &'static str {
    match root {
        InterfaceFactRoot::RelocatedSymbol { .. } => "relocated-symbol",
        InterfaceFactRoot::FunctionArgument { .. } => "function-argument",
        InterfaceFactRoot::AbsoluteAddress { .. } => "absolute-address",
    }
}

fn read_artifact<T>(
    path: &Path,
    kind: &'static str,
    parse: impl FnOnce(&str) -> Result<T>,
) -> Result<T> {
    let input = std::fs::read_to_string(path)?;
    parse(&input).map_err(|error| BlobrayError::manifest_document(kind, path, &input, error))
}

#[cfg(test)]
mod index_tests {
    use super::*;

    #[test]
    fn project_calls_preserve_unique_ambiguous_and_unresolved_candidates() {
        let definitions = BTreeMap::from([
            ("one".to_owned(), BTreeSet::from(["rom::one".to_owned()])),
            (
                "many".to_owned(),
                BTreeSet::from(["a::many".to_owned(), "b::many".to_owned()]),
            ),
        ]);
        let calls = BTreeSet::from([
            ("archive::root".to_owned(), Some(0x10), "one".to_owned()),
            ("archive::root".to_owned(), Some(0x20), "many".to_owned()),
            ("archive::root".to_owned(), Some(0x30), "missing".to_owned()),
        ]);

        let links = project_call_links(&definitions, calls);
        assert_eq!(links[0].status, "unique");
        assert_eq!(links[0].candidates, ["rom::one"]);
        assert_eq!(links[1].status, "ambiguous");
        assert_eq!(links[1].candidates, ["a::many", "b::many"]);
        assert_eq!(links[2].status, "unresolved");
        assert!(links.iter().all(|link| !link.linker_resolution_claim));
    }

    #[test]
    fn unknown_zero_absolute_root_never_joins_zero_offset_symbols() {
        let key = SymbolKey {
            occurrence: NavigationIdentity::Symbol {
                artifact_sha256: "11".repeat(32),
                location: crate::SymbolLocation {
                    object: crate::ObjectLocation::Standalone,
                    table: crate::ArtifactSymbolTable::Static,
                    index: 1,
                },
            },
            artifact_sha256: "11".repeat(32),
            member: None,
            name: "undefined".to_owned(),
            object_address: 0,
        };
        let index = InterfaceRootIndex::new(std::iter::once(&key));

        assert!(
            index
                .matches(
                    &key.artifact_sha256,
                    &InterfaceFactRoot::AbsoluteAddress {
                        data_address: open_radio_vendor_contracts::DataAddressResolution::Unknown {
                            reason:
                                open_radio_vendor_contracts::DataAddressGap::NoContainingDefinition
                        },
                        address: 0,
                    },
                )
                .is_empty()
        );
    }
}
