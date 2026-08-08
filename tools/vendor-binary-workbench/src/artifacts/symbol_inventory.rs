//! Stored symbol-inventory schema, writer and strict summary projection.

#![allow(
    dead_code,
    reason = "complete stored DTOs enforce every persistent schema field"
)]

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::SYMBOL_INVENTORY;
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredSymbolInventory {
    schema_version: u32,
    command: String,
    linkage_mode: String,
    linker_resolution_claim: bool,
    pub(crate) artifacts: Vec<StoredSymbolArtifact>,
    pub(crate) symbols: Vec<StoredSymbolFact>,
    summary: StoredSummaryDocument,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredSymbolArtifact {
    pub(crate) index: usize,
    pub(crate) artifact: StoredSymbolArtifactIdentity,
    roles: Vec<String>,
    pub(crate) sources: Vec<String>,
    container: String,
    objects: usize,
    skipped_members: usize,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredSymbolArtifactIdentity {
    pub(crate) path: String,
    pub(crate) sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredSymbolFact {
    pub(crate) artifact: usize,
    pub(crate) member: Option<String>,
    object_kind: String,
    pub(crate) name: String,
    pub(crate) table: String,
    binding: String,
    visibility: String,
    pub(crate) definition: String,
    pub(crate) kind: String,
    section: Option<String>,
    pub(crate) address: String,
    size: u64,
    scope: String,
    pub(crate) resolution: String,
    candidates: Vec<StoredSymbolCandidate>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredSymbolCandidate {
    artifact: usize,
    member: Option<String>,
    address: String,
    kind: String,
}

pub(crate) fn parse_symbol_inventory(input: &str) -> crate::Result<StoredSymbolInventory> {
    super::expect_identity(input, SYMBOL_INVENTORY)?;
    let document: StoredSymbolInventory = serde_json::from_str(input)?;
    if document.linkage_mode != "association-only" || document.linker_resolution_claim {
        return Err(crate::Error::invalid(
            "symbol inventory makes an unsupported linker-resolution claim",
        ));
    }
    Ok(document)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredSummaryDocument {
    artifacts: usize,
    symbol_facts: usize,
    emitted: usize,
    exported_definitions: usize,
    undefined: usize,
    unresolved_or_associated: usize,
}

pub(crate) fn inspect_symbol_inventory(path: &Path) -> crate::Result<SymbolInventorySummary> {
    let input = std::fs::read_to_string(path)?;
    let document = parse_symbol_inventory(&input).map_err(|error| {
        crate::Error::invalid(format!(
            "unsupported symbol inventory in {}: {error}",
            path.display()
        ))
    })?;
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
            r#"{"schema_version":2,"command":"symbols inventory","linkage_mode":"association-only","linker_resolution_claim":false,"artifacts":[],"symbols":[],"summary":{"artifacts":3,"symbol_facts":40,"emitted":40,"exported_definitions":12,"undefined":7,"unresolved_or_associated":5}}"#,
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

    #[test]
    fn stored_inventory_rejects_unknown_and_missing_fields() {
        let input = r#"{"schema_version":2,"command":"symbols inventory","linkage_mode":"association-only","linker_resolution_claim":false,"artifacts":[],"symbols":[],"summary":{"artifacts":0,"symbol_facts":0,"emitted":0,"exported_definitions":0,"undefined":0,"unresolved_or_associated":0}}"#;
        let mut unknown: serde_json::Value = serde_json::from_str(input).unwrap();
        unknown["summary"]["legacy_field"] = serde_json::json!(true);
        let error = parse_symbol_inventory(&unknown.to_string()).unwrap_err();
        assert!(error.to_string().contains("unknown field `legacy_field`"));

        let mut missing: serde_json::Value = serde_json::from_str(input).unwrap();
        missing["summary"]
            .as_object_mut()
            .unwrap()
            .remove("emitted");
        let error = parse_symbol_inventory(&missing.to_string()).unwrap_err();
        assert!(error.to_string().contains("missing field `emitted`"));
    }
}
