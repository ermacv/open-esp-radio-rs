//! Stored symbol-inventory schema, writer and strict summary projection.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{SYMBOL_INVENTORY, read_json};
use crate::{
    analysis::{LinkageSymbol, ProjectLinkageInventory},
    artifact_sha256,
};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct ArtifactIdentity {
    path: String,
    sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct ArtifactDocument {
    index: usize,
    artifact: ArtifactIdentity,
    roles: Vec<String>,
    sources: Vec<String>,
    container: &'static str,
    objects: usize,
    skipped_members: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct CandidateDocument {
    artifact: usize,
    member: Option<String>,
    address: String,
    kind: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct SymbolDocument {
    artifact: usize,
    member: Option<String>,
    object_kind: &'static str,
    table: &'static str,
    name: String,
    binding: String,
    visibility: String,
    kind: &'static str,
    definition: &'static str,
    section: Option<String>,
    address: String,
    size: u64,
    scope: &'static str,
    resolution: &'static str,
    candidates: Vec<CandidateDocument>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct SummaryDocument {
    artifacts: usize,
    symbol_facts: usize,
    emitted: usize,
    exported_definitions: usize,
    undefined: usize,
    unresolved_or_associated: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct SymbolInventoryDocument {
    schema_version: u32,
    command: &'static str,
    linkage_mode: &'static str,
    linker_resolution_claim: bool,
    artifacts: Vec<ArtifactDocument>,
    symbols: Vec<SymbolDocument>,
    summary: SummaryDocument,
}

pub(crate) fn build_symbol_inventory_document(
    inventory: &ProjectLinkageInventory,
    include: impl Fn(&LinkageSymbol) -> bool,
) -> crate::Result<SymbolInventoryDocument> {
    let symbols = inventory
        .symbols
        .iter()
        .filter(|symbol| include(symbol))
        .collect::<Vec<_>>();
    let undefined = inventory
        .symbols
        .iter()
        .filter(|symbol| {
            symbol.fact.definition == crate::artifact::ArtifactSymbolDefinitionState::Undefined
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
    Ok(SymbolInventoryDocument {
        schema_version: SYMBOL_INVENTORY.version,
        command: SYMBOL_INVENTORY.command,
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
                    roles: artifact.roles.clone(),
                    sources: artifact.sources.clone(),
                    container: artifact.container.label(),
                    objects: artifact.objects,
                    skipped_members: artifact.skipped_members,
                })
            })
            .collect::<crate::Result<Vec<_>>>()?,
        symbols: symbols
            .iter()
            .map(|symbol| SymbolDocument {
                artifact: symbol.artifact,
                member: symbol.member.clone(),
                object_kind: symbol.object_kind.label(),
                table: symbol.fact.table.label(),
                name: symbol.fact.name.clone(),
                binding: symbol.fact.binding.label(),
                visibility: symbol.fact.visibility.label(),
                kind: symbol.fact.kind.label(),
                definition: symbol.fact.definition.label(),
                section: symbol.fact.section.clone(),
                address: format!("{:#x}", symbol.fact.address),
                size: symbol.fact.size,
                scope: symbol.fact.scope.label(),
                resolution: symbol.resolution.label(),
                candidates: symbol
                    .candidates
                    .iter()
                    .map(|candidate| CandidateDocument {
                        artifact: candidate.artifact,
                        member: candidate.member.clone(),
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

pub(crate) fn render_symbol_inventory(document: &SymbolInventoryDocument) -> crate::Result<String> {
    let mut output = serde_json::to_string_pretty(document)?;
    output.push('\n');
    Ok(output)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SymbolInventorySummary {
    pub(crate) artifacts: usize,
    pub(crate) symbol_facts: usize,
    pub(crate) exported_definitions: usize,
    pub(crate) undefined: usize,
    pub(crate) unresolved_or_associated: usize,
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

pub(crate) fn inspect_symbol_inventory(path: &Path) -> crate::Result<SymbolInventorySummary> {
    let document = read_json::<StoredInventoryDocument>("symbol inventory", path)?;
    if document.schema_version != SYMBOL_INVENTORY.version
        || document.command != SYMBOL_INVENTORY.command
    {
        return Err(crate::Error::invalid(format!(
            "unsupported symbol inventory in {}: expected schema_version {} and command {:?}",
            path.display(),
            SYMBOL_INVENTORY.version,
            SYMBOL_INVENTORY.command,
        )));
    }
    Ok(SymbolInventorySummary {
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
        std::fs::write(
            &path,
            r#"{"schema_version":2,"command":"symbols inventory","summary":{"artifacts":3,"symbol_facts":40,"emitted":40,"exported_definitions":12,"undefined":7,"unresolved_or_associated":5}}"#,
        )
        .unwrap();
        let summary = inspect_symbol_inventory(&path).unwrap();
        assert_eq!(summary.artifacts, 3);
        assert_eq!(summary.symbol_facts, 40);
        assert_eq!(summary.exported_definitions, 12);

        std::fs::write(
            &path,
            r#"{"schema_version":1,"command":"symbols inventory","summary":{"artifacts":0,"symbol_facts":0,"exported_definitions":0,"undefined":0,"unresolved_or_associated":0}}"#,
        )
        .unwrap();
        assert!(
            inspect_symbol_inventory(&path)
                .unwrap_err()
                .to_string()
                .contains("expected schema_version 2")
        );
        std::fs::remove_file(path).unwrap();
    }
}
