//! Project artifact and symbol facts for manual linkage analysis.

use std::{fs, path::Path};

use serde::{Deserialize, Serialize};

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

fn render_json_report(document: &InventoryDocument<'_>) -> Result<String> {
    let mut output = serde_json::to_string_pretty(&document)?;
    output.push('\n');
    Ok(output)
}

pub(super) fn run(options: SymbolInventoryArgs, run_spec: &RunSpec) -> Result<bool> {
    if options.check && options.json_report.is_none() {
        return Err(
            "symbols inventory --check requires --json-report or project [analysis.symbols]".into(),
        );
    }
    let inputs = run_spec
        .inputs()
        .iter()
        .map(|input| (input.role.to_string(), input.path.clone()))
        .collect::<Vec<_>>();
    let inventory = build_project_linkage_inventory(&inputs)?;
    let document = document(&inventory, &options)?;
    if !crate::cli::output::structured("symbol-inventory", &document) {
        print_report(&inventory, &options);
    }
    if let Some(path) = options.json_report.as_deref() {
        let output = render_json_report(&document)?;
        super::super::generated_output::write_or_check(
            path,
            &output,
            options.check,
            "symbol inventory",
        )?;
        let status = if options.check { "verified" } else { "written" };
        if !crate::cli::output::file("symbol-inventory-file", path, status) {
            outputln!("JSON-REPORT\tstatus={status}\t{}", path.display());
        }
    }
    Ok(true)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct StoredInventorySummary {
    pub(super) artifacts: usize,
    pub(super) symbol_facts: usize,
    pub(super) exported_definitions: usize,
    pub(super) undefined: usize,
    pub(super) unresolved_or_associated: usize,
}

#[derive(Deserialize)]
struct StoredInventoryDocument {
    schema_version: u32,
    command: String,
    summary: StoredSummaryDocument,
}

#[derive(Deserialize)]
struct StoredSummaryDocument {
    artifacts: usize,
    symbol_facts: usize,
    exported_definitions: usize,
    undefined: usize,
    unresolved_or_associated: usize,
}

pub(super) fn inspect_report(path: &Path) -> Result<StoredInventorySummary> {
    let input = fs::read_to_string(path)?;
    let document = serde_json::from_str::<StoredInventoryDocument>(&input)?;
    if document.schema_version != 2 || document.command != "symbols inventory" {
        return Err(format!(
            "unsupported symbol inventory in {}: expected schema_version 2 and command \"symbols inventory\"",
            path.display()
        )
        .into());
    }
    Ok(StoredInventorySummary {
        artifacts: document.summary.artifacts,
        symbol_facts: document.summary.symbol_facts,
        exported_definitions: document.summary.exported_definitions,
        undefined: document.summary.undefined,
        unresolved_or_associated: document.summary.unresolved_or_associated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_inventory_summary_is_strictly_versioned() {
        let path = std::env::temp_dir().join(format!(
            "vendor-workbench-symbol-inventory-{}.json",
            std::process::id()
        ));
        fs::write(
            &path,
            r#"{
  "schema_version": 2,
  "command": "symbols inventory",
  "summary": {
    "artifacts": 3,
    "symbol_facts": 40,
    "emitted": 40,
    "exported_definitions": 12,
    "undefined": 7,
    "unresolved_or_associated": 5
  }
}
"#,
        )
        .unwrap();
        let summary = inspect_report(&path).unwrap();
        assert_eq!(summary.artifacts, 3);
        assert_eq!(summary.symbol_facts, 40);
        assert_eq!(summary.exported_definitions, 12);

        fs::write(
            &path,
            r#"{"schema_version":1,"command":"symbols inventory","summary":{"artifacts":0,"symbol_facts":0,"exported_definitions":0,"undefined":0,"unresolved_or_associated":0}}"#,
        )
        .unwrap();
        assert!(
            inspect_report(&path)
                .unwrap_err()
                .to_string()
                .contains("expected schema_version 2")
        );
        fs::remove_file(path).unwrap();
    }
}
