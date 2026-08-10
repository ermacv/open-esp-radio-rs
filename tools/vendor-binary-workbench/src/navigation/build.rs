//! Deterministic cross-report join construction.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use super::model::{
    ArtifactDocument, IDENTITY_SCHEME, InterfaceCallObservation, InterfaceRootObservation,
    InventoryObservation, IrObservation, NavigationDocument, SCHEMA_VERSION, SummaryDocument,
    SymbolDocument, SymbolKey, address, artifact, input, symbol,
};
use crate::{
    Result,
    artifacts::{
        LinkedIrStoredDocument, StoredInterfaceFacts, StoredInterfaceRoot, StoredSymbolInventory,
    },
    error::WorkbenchError,
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

    add_linked_ir(
        project,
        navigation_output,
        &mut inputs,
        &mut artifacts,
        &mut symbols,
    )?;
    let unmatched_interface_roots = add_interfaces(
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
    };
    Ok(NavigationDocument {
        schema_version: SCHEMA_VERSION,
        command: "project navigation".to_owned(),
        identity_scheme: IDENTITY_SCHEME.to_owned(),
        semantic_claim: false,
        linker_resolution_claim: false,
        inputs,
        artifacts,
        symbols,
        summary,
    })
}

fn add_linked_ir(
    project: &ProjectSpec,
    navigation_output: &Path,
    inputs: &mut Vec<super::model::InputDocument>,
    artifacts: &mut BTreeMap<String, ArtifactDocument>,
    symbols: &mut BTreeMap<SymbolKey, SymbolDocument>,
) -> Result<()> {
    for profile in &project.ir_profiles {
        let report: LinkedIrStoredDocument = read_artifact(
            &profile.output,
            "linked-IR report",
            crate::artifacts::parse_linked_ir,
        )?;
        inputs.push(input(
            "linked-ir",
            profile.id.clone(),
            &profile.output,
            navigation_output,
        )?);
        let mut source_artifacts = BTreeMap::new();
        for item in report.artifacts {
            let document = artifact(artifacts, &item.artifact.sha256);
            document.paths.insert(item.artifact.path);
            document.sources.insert(item.source.clone());
            source_artifacts.insert(item.source, item.artifact.sha256);
        }
        for function in report.functions {
            let sha256 = source_artifacts
                .get(&function.source)
                .ok_or_else(|| {
                    format!(
                        "linked-IR function {:?} refers to unknown source {:?}",
                        function.identity, function.source
                    )
                })
                .map_err(crate::Error::invalid)?;
            let key = SymbolKey {
                artifact_sha256: sha256.clone(),
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

fn add_interfaces(
    project: &ProjectSpec,
    navigation_output: &Path,
    inputs: &mut Vec<super::model::InputDocument>,
    artifacts: &mut BTreeMap<String, ArtifactDocument>,
    symbols: &mut BTreeMap<SymbolKey, SymbolDocument>,
) -> Result<usize> {
    let Some(paths) = &project.interfaces else {
        return Ok(0);
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
    for item in report.artifacts {
        let document = artifact(artifacts, &item.sha256);
        document.paths.insert(item.path);
        document.sources.extend(item.sources.iter().cloned());
        interface_artifacts.insert(item.index, (item.sha256, item.sources));
    }
    let mut root_index = InterfaceRootIndex::new(symbols.keys());
    let mut unmatched_roots = 0;
    for call in report.calls {
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
            artifact_sha256: sha256.clone(),
            member: call.member.clone(),
            name: call.function.clone(),
            object_address: call.function_address,
        };
        {
            let caller = symbol(symbols, &caller_key);
            caller.sources.extend(sources.iter().cloned());
            caller.interface_calls.insert(InterfaceCallObservation {
                site: format!("{:#x}", call.site),
                kind: call.kind.clone(),
            });
        }
        // The caller may be absent from inventory and linked IR. Preserve the
        // old incremental join semantics by making the newly observed symbol
        // available to later interface-root lookups too.
        root_index.insert(&caller_key);

        let root_matches = root_index.matches(sha256, &call.target.root);
        if root_matches.is_empty()
            && !matches!(
                call.target.root,
                StoredInterfaceRoot::FunctionArgument { .. }
            )
        {
            unmatched_roots += 1;
        }
        for key in root_matches {
            symbol(symbols, &key)
                .interface_roots
                .insert(InterfaceRootObservation {
                    function: call.function.clone(),
                    site: format!("{:#x}", call.site),
                    kind: interface_root_kind(&call.target.root).to_owned(),
                });
        }
    }
    Ok(unmatched_roots)
}

/// Indexed view of the two exact root identities used by interface facts.
///
/// Navigation used to scan every accumulated symbol for every interface call.
/// Real projects have tens of thousands of symbols and thousands of calls,
/// making that join quadratic. These indexes retain the same `BTreeSet`
/// ordering while reducing each lookup to logarithmic map access.
struct InterfaceRootIndex {
    relocated: BTreeMap<(String, Option<String>, String), BTreeSet<SymbolKey>>,
    absolute: BTreeMap<(String, u32), BTreeSet<SymbolKey>>,
}

impl InterfaceRootIndex {
    fn new<'a>(symbols: impl Iterator<Item = &'a SymbolKey>) -> Self {
        let mut index = Self {
            relocated: BTreeMap::new(),
            absolute: BTreeMap::new(),
        };
        for symbol in symbols {
            index.insert(symbol);
        }
        index
    }

    fn insert(&mut self, symbol: &SymbolKey) {
        self.relocated
            .entry((
                symbol.artifact_sha256.clone(),
                symbol.member.clone(),
                symbol.name.clone(),
            ))
            .or_default()
            .insert(symbol.clone());
        self.absolute
            .entry((symbol.artifact_sha256.clone(), symbol.object_address))
            .or_default()
            .insert(symbol.clone());
    }

    fn matches(&self, artifact_sha256: &str, root: &StoredInterfaceRoot) -> Vec<SymbolKey> {
        match root {
            StoredInterfaceRoot::RelocatedSymbol { member, symbol, .. } => {
                self.relocated
                    .get(&(artifact_sha256.to_owned(), member.clone(), symbol.clone()))
            }
            StoredInterfaceRoot::AbsoluteAddress { address, .. } => {
                self.absolute.get(&(artifact_sha256.to_owned(), *address))
            }
            StoredInterfaceRoot::FunctionArgument { .. } => None,
        }
        .into_iter()
        .flat_map(|matches| matches.iter().cloned())
        .collect()
    }
}

fn interface_root_kind(root: &StoredInterfaceRoot) -> &'static str {
    match root {
        StoredInterfaceRoot::RelocatedSymbol { .. } => "relocated-symbol",
        StoredInterfaceRoot::FunctionArgument { .. } => "function-argument",
        StoredInterfaceRoot::AbsoluteAddress { .. } => "absolute-address",
    }
}

fn read_artifact<T>(
    path: &Path,
    kind: &'static str,
    parse: impl FnOnce(&str) -> Result<T>,
) -> Result<T> {
    let input = std::fs::read_to_string(path)?;
    parse(&input).map_err(|error| WorkbenchError::manifest_document(kind, path, &input, error))
}
