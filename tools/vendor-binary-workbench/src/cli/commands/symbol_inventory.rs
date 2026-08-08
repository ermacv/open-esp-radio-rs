//! Project artifact and symbol facts for manual linkage analysis.

use std::fs;

use serde::Serialize;

use super::super::*;
use crate::run_spec::RunSpec;

type Options = SymbolInventoryArgs;

impl Options {
    fn includes(&self, symbol: &LinkageSymbol) -> bool {
        self.name_prefix
            .as_ref()
            .is_none_or(|prefix| symbol.fact.name.starts_with(prefix))
            && (!self.undefined_only
                || symbol.fact.definition == artifact::ArtifactSymbolDefinitionState::Undefined)
    }
}

fn optional_human(value: Option<&str>) -> &str {
    value.unwrap_or("-")
}

fn print_report(inventory: &ProjectLinkageInventory, options: &Options) {
    for (index, artifact) in inventory.artifacts.iter().enumerate() {
        outputln!(
            "ARTIFACT\tindex={}\tcontainer={}\tobjects={}\tskipped-members={}\troles={}\tsources={}\tpath={}",
            index,
            artifact.container.label(),
            artifact.objects,
            artifact.skipped_members,
            artifact.roles.join(","),
            artifact.sources.join(","),
            artifact.path.display()
        );
    }
    for symbol in inventory
        .symbols
        .iter()
        .filter(|symbol| options.includes(symbol))
    {
        outputln!(
            "SYMBOL\tartifact={}\tmember={}\tobject={}\ttable={}\tname={}\tbinding={}\tvisibility={}\tkind={}\tdefinition={}\tsection={}\taddress={:#x}\tsize={}\tscope={}\tresolution={}\tcandidates={}",
            symbol.artifact,
            optional_human(symbol.member.as_deref()),
            symbol.object_kind.label(),
            symbol.fact.table.label(),
            symbol.fact.name,
            symbol.fact.binding.label(),
            symbol.fact.visibility.label(),
            symbol.fact.kind.label(),
            symbol.fact.definition.label(),
            optional_human(symbol.fact.section.as_deref()),
            symbol.fact.address,
            symbol.fact.size,
            symbol.fact.scope.label(),
            symbol.resolution.label(),
            symbol.candidates.len(),
        );
        for candidate in &symbol.candidates {
            outputln!(
                "CANDIDATE\tname={}\tartifact={}\tmember={}\taddress={:#x}\tkind={}",
                symbol.fact.name,
                candidate.artifact,
                optional_human(candidate.member.as_deref()),
                candidate.address,
                candidate.kind.label(),
            );
        }
    }
    let emitted = inventory
        .symbols
        .iter()
        .filter(|symbol| options.includes(symbol))
        .count();
    let undefined = inventory
        .symbols
        .iter()
        .filter(|symbol| {
            symbol.fact.definition == artifact::ArtifactSymbolDefinitionState::Undefined
        })
        .count();
    let exported = inventory
        .symbols
        .iter()
        .filter(|symbol| symbol.fact.is_exported_definition())
        .count();
    let unresolved = inventory
        .symbols
        .iter()
        .filter(|symbol| symbol.resolution.is_unresolved())
        .count();
    outputln!(
        "SUMMARY\tartifacts={}\tsymbol-facts={}\temitted={}\texported-definitions={}\tundefined={}\tunresolved-or-associated={}",
        inventory.artifacts.len(),
        inventory.symbols.len(),
        emitted,
        exported,
        undefined,
        unresolved,
    );
}

#[derive(Serialize)]
struct ArtifactIdentity {
    path: String,
    sha256: String,
}

#[derive(Serialize)]
struct ArtifactDocument<'a> {
    index: usize,
    artifact: ArtifactIdentity,
    roles: &'a [String],
    sources: &'a [String],
    container: &'static str,
    objects: usize,
    skipped_members: usize,
}

#[derive(Serialize)]
struct CandidateDocument<'a> {
    artifact: usize,
    member: Option<&'a str>,
    address: String,
    kind: &'static str,
}

#[derive(Serialize)]
struct SymbolDocument<'a> {
    artifact: usize,
    member: Option<&'a str>,
    object_kind: &'static str,
    table: &'static str,
    name: &'a str,
    binding: String,
    visibility: String,
    kind: &'static str,
    definition: &'static str,
    section: Option<&'a str>,
    address: String,
    size: u64,
    scope: &'static str,
    resolution: &'static str,
    candidates: Vec<CandidateDocument<'a>>,
}

#[derive(Serialize)]
struct SummaryDocument {
    artifacts: usize,
    symbol_facts: usize,
    emitted: usize,
    exported_definitions: usize,
    undefined: usize,
    unresolved_or_associated: usize,
}

#[derive(Serialize)]
struct InventoryDocument<'a> {
    schema_version: u32,
    command: &'static str,
    linkage_mode: &'static str,
    linker_resolution_claim: bool,
    artifacts: Vec<ArtifactDocument<'a>>,
    symbols: Vec<SymbolDocument<'a>>,
    summary: SummaryDocument,
}

fn document<'a>(
    inventory: &'a ProjectLinkageInventory,
    options: &Options,
) -> Result<InventoryDocument<'a>> {
    let symbols = inventory
        .symbols
        .iter()
        .filter(|symbol| options.includes(symbol))
        .collect::<Vec<_>>();
    let undefined = inventory
        .symbols
        .iter()
        .filter(|symbol| {
            symbol.fact.definition == artifact::ArtifactSymbolDefinitionState::Undefined
        })
        .count();
    let exported = inventory
        .symbols
        .iter()
        .filter(|symbol| symbol.fact.is_exported_definition())
        .count();
    let unresolved = inventory
        .symbols
        .iter()
        .filter(|symbol| symbol.resolution.is_unresolved())
        .count();
    Ok(InventoryDocument {
        schema_version: 2,
        command: "symbols inventory",
        linkage_mode: "association-only",
        linker_resolution_claim: false,
        artifacts: inventory
            .artifacts
            .iter()
            .enumerate()
            .map(|(index, artifact)| {
                Ok(ArtifactDocument {
                    index,
                    artifact: ArtifactIdentity {
                        path: artifact.path.display().to_string(),
                        sha256: artifact_sha256(&artifact.path)?,
                    },
                    roles: &artifact.roles,
                    sources: &artifact.sources,
                    container: artifact.container.label(),
                    objects: artifact.objects,
                    skipped_members: artifact.skipped_members,
                })
            })
            .collect::<Result<Vec<_>>>()?,
        symbols: symbols
            .iter()
            .map(|symbol| SymbolDocument {
                artifact: symbol.artifact,
                member: symbol.member.as_deref(),
                object_kind: symbol.object_kind.label(),
                table: symbol.fact.table.label(),
                name: &symbol.fact.name,
                binding: symbol.fact.binding.label(),
                visibility: symbol.fact.visibility.label(),
                kind: symbol.fact.kind.label(),
                definition: symbol.fact.definition.label(),
                section: symbol.fact.section.as_deref(),
                address: format!("{:#x}", symbol.fact.address),
                size: symbol.fact.size,
                scope: symbol.fact.scope.label(),
                resolution: symbol.resolution.label(),
                candidates: symbol
                    .candidates
                    .iter()
                    .map(|candidate| CandidateDocument {
                        artifact: candidate.artifact,
                        member: candidate.member.as_deref(),
                        address: format!("{:#x}", candidate.address),
                        kind: candidate.kind.label(),
                    })
                    .collect(),
            })
            .collect(),
        summary: SummaryDocument {
            artifacts: inventory.artifacts.len(),
            symbol_facts: inventory.symbols.len(),
            emitted: symbols.len(),
            exported_definitions: exported,
            undefined,
            unresolved_or_associated: unresolved,
        },
    })
}

fn write_json_report(path: &std::path::Path, document: &InventoryDocument<'_>) -> Result<()> {
    let mut output = serde_json::to_string_pretty(&document)?;
    output.push('\n');
    fs::write(path, output)?;
    Ok(())
}

pub(super) fn run(options: SymbolInventoryArgs, run_spec: &RunSpec) -> Result<bool> {
    let inventory = build_project_linkage_inventory(run_spec.inputs())?;
    let document = document(&inventory, &options)?;
    if !crate::cli::output::structured("symbol-inventory", &document) {
        print_report(&inventory, &options);
    }
    if let Some(path) = options.json_report.as_deref() {
        write_json_report(path, &document)?;
    }
    Ok(true)
}
