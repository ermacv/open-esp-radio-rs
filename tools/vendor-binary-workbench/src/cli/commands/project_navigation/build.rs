//! Deterministic cross-report join construction.

use std::collections::BTreeMap;

use super::{
    Result,
    model::{
        ArtifactDocument, IDENTITY_SCHEME, InterfaceCallObservation, InterfaceRootObservation,
        InventoryObservation, IrObservation, NavigationDocument, SCHEMA_VERSION, SummaryDocument,
        SymbolDocument, SymbolKey, address, artifact, input, symbol,
    },
    reports::{InterfaceReport, InventoryReport, IrReport, read},
};
use crate::{parse_u32, project::ProjectSpec};

pub(super) fn build(project: &ProjectSpec) -> Result<NavigationDocument> {
    let symbols_spec = project
        .symbol_inventory
        .as_ref()
        .ok_or("project navigation requires [analysis.symbols]")?;
    let inventory: InventoryReport = read(&symbols_spec.output, "symbol inventory")?;
    if inventory.schema_version != 2 || inventory.command != "symbols inventory" {
        return Err("project navigation requires symbols inventory schema_version 2".into());
    }

    let mut inputs = vec![input(
        "symbol-inventory",
        "symbols".to_owned(),
        &symbols_spec.output,
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
        let (sha256, sources) = inventory_artifacts.get(&fact.artifact).ok_or_else(|| {
            format!(
                "symbol refers to unknown inventory artifact {}",
                fact.artifact
            )
        })?;
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

    add_linked_ir(project, &mut inputs, &mut artifacts, &mut symbols)?;
    let unmatched_interface_roots =
        add_interfaces(project, &mut inputs, &mut artifacts, &mut symbols)?;

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
        command: "project navigation",
        identity_scheme: IDENTITY_SCHEME,
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
    inputs: &mut Vec<super::model::InputDocument>,
    artifacts: &mut BTreeMap<String, ArtifactDocument>,
    symbols: &mut BTreeMap<SymbolKey, SymbolDocument>,
) -> Result<()> {
    for profile in &project.ir_profiles {
        let report: IrReport = read(&profile.output, "linked-IR report")?;
        if report.schema_version != 32 || report.command != "ir export" {
            return Err(format!(
                "project navigation requires linked-IR schema_version 32 for profile {:?}",
                profile.id
            )
            .into());
        }
        inputs.push(input("linked-ir", profile.id.clone(), &profile.output)?);
        let mut source_artifacts = BTreeMap::new();
        for item in report.artifacts {
            let document = artifact(artifacts, &item.artifact.sha256);
            document.paths.insert(item.artifact.path);
            document.sources.insert(item.source.clone());
            source_artifacts.insert(item.source, item.artifact.sha256);
        }
        for function in report.functions {
            let sha256 = source_artifacts.get(&function.source).ok_or_else(|| {
                format!(
                    "linked-IR function {:?} refers to unknown source {:?}",
                    function.identity, function.source
                )
            })?;
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
    inputs: &mut Vec<super::model::InputDocument>,
    artifacts: &mut BTreeMap<String, ArtifactDocument>,
    symbols: &mut BTreeMap<SymbolKey, SymbolDocument>,
) -> Result<usize> {
    let Some(paths) = &project.interfaces else {
        return Ok(0);
    };
    let report: InterfaceReport = read(&paths.facts, "interface facts")?;
    if report.schema_version != 2 || report.command != "interfaces discover" {
        return Err("project navigation requires interface facts schema_version 2".into());
    }
    inputs.push(input(
        "interface-facts",
        "interfaces".to_owned(),
        &paths.facts,
    )?);
    let mut interface_artifacts = BTreeMap::new();
    for item in report.artifacts {
        let document = artifact(artifacts, &item.sha256);
        document.paths.insert(item.path);
        document.sources.extend(item.sources.iter().cloned());
        interface_artifacts.insert(item.index, (item.sha256, item.sources));
    }
    let mut unmatched_roots = 0;
    for call in report.calls {
        let (sha256, sources) = interface_artifacts.get(&call.artifact).ok_or_else(|| {
            format!(
                "interface call refers to unknown artifact {}",
                call.artifact
            )
        })?;
        let caller_key = SymbolKey {
            artifact_sha256: sha256.clone(),
            member: call.member.clone(),
            name: call.function.clone(),
            object_address: address(&call.function_address, "interface function")?,
        };
        let caller = symbol(symbols, &caller_key);
        caller.sources.extend(sources.iter().cloned());
        caller.interface_calls.insert(InterfaceCallObservation {
            site: call.site.clone(),
            kind: call.kind.clone(),
        });

        let root_matches = symbols
            .keys()
            .filter(|key| {
                key.artifact_sha256 == *sha256
                    && match call.target.root.kind.as_str() {
                        "relocated-symbol" => {
                            key.member == call.target.root.member
                                && call.target.root.symbol.as_ref() == Some(&key.name)
                        }
                        "absolute-address" => {
                            call.target.root.address.as_deref().and_then(parse_u32)
                                == Some(key.object_address)
                        }
                        _ => false,
                    }
            })
            .cloned()
            .collect::<Vec<_>>();
        if root_matches.is_empty() && call.target.root.kind != "function-argument" {
            unmatched_roots += 1;
        }
        for key in root_matches {
            symbol(symbols, &key)
                .interface_roots
                .insert(InterfaceRootObservation {
                    function: call.function.clone(),
                    site: call.site.clone(),
                    kind: call.target.root.kind.clone(),
                });
        }
    }
    Ok(unmatched_roots)
}
