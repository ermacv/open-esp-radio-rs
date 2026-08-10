//! Stored symbol-inventory schema and writer.

use serde::Serialize;

use super::SYMBOL_INVENTORY;
use crate::{
    analysis::{LinkageSymbol, ProjectLinkageInventory},
    artifact_sha256,
};

mod read;

pub(crate) use read::{
    CodeBoundaryCandidateFact, CodeBoundaryFacts, StoredSymbolInventory, inspect_symbol_inventory,
    load_code_boundary_facts, parse_symbol_inventory,
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
    origin_association: &'static str,
    origin_candidates: Vec<CandidateDocument>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct CodeRangeDocument {
    start_offset: String,
    end_offset: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct DirectControlFlowDocument {
    caller: String,
    site_offset: String,
    kind: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct FunctionCandidateDocument {
    entry_offset: String,
    end_limit_offset: String,
    symbol_names: Vec<String>,
    direct_control_flow: Vec<DirectControlFlowDocument>,
    reviewed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct RecoveryBlockerDocument {
    symbol: String,
    message: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct CodeSectionDocument {
    artifact: usize,
    member: Option<String>,
    object_kind: &'static str,
    section: String,
    address: String,
    executable_bytes: u64,
    named_sized_symbols: usize,
    named_zero_sized_symbols: usize,
    symbol_covered_bytes: u64,
    uncovered_bytes: u64,
    uncovered_ranges: Vec<CodeRangeDocument>,
    function_candidates: Vec<FunctionCandidateDocument>,
    recovery_blockers: Vec<RecoveryBlockerDocument>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct SummaryDocument {
    artifacts: usize,
    symbol_facts: usize,
    emitted: usize,
    exported_definitions: usize,
    undefined: usize,
    unresolved_or_associated: usize,
    executable_sections: usize,
    executable_bytes: u64,
    symbol_covered_bytes: u64,
    uncovered_executable_bytes: u64,
    named_zero_sized_code_symbols: usize,
    function_boundary_candidates: usize,
    code_recovery_blockers: usize,
    link_unit_definitions: usize,
    unique_archive_origins: usize,
    ambiguous_archive_origins: usize,
    missing_archive_origins: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct SymbolInventoryDocument {
    schema_version: u32,
    command: &'static str,
    linkage_mode: &'static str,
    linker_resolution_claim: bool,
    artifacts: Vec<ArtifactDocument>,
    code_sections: Vec<CodeSectionDocument>,
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
        code_sections: inventory
            .artifacts
            .iter()
            .enumerate()
            .flat_map(|(artifact, input)| {
                input.code_sections.iter().map(move |section| {
                    let coverage = &section.coverage;
                    CodeSectionDocument {
                        artifact,
                        member: section.member.clone(),
                        object_kind: section.object_kind.label(),
                        section: coverage.name.clone(),
                        address: format!("{:#x}", coverage.address),
                        executable_bytes: coverage.size,
                        named_sized_symbols: coverage.named_sized_symbols,
                        named_zero_sized_symbols: coverage.named_zero_sized_symbols,
                        symbol_covered_bytes: coverage.symbol_covered_bytes,
                        uncovered_bytes: coverage.size - coverage.symbol_covered_bytes,
                        uncovered_ranges: coverage
                            .uncovered_ranges
                            .iter()
                            .map(|range| CodeRangeDocument {
                                start_offset: format!("{:#x}", range.start_offset),
                                end_offset: format!("{:#x}", range.end_offset),
                            })
                            .collect(),
                        function_candidates: coverage
                            .function_candidates
                            .iter()
                            .map(|candidate| FunctionCandidateDocument {
                                entry_offset: format!("{:#x}", candidate.entry_offset),
                                end_limit_offset: format!("{:#x}", candidate.end_limit_offset),
                                symbol_names: candidate.symbol_names.clone(),
                                direct_control_flow: candidate
                                    .direct_control_flow
                                    .iter()
                                    .map(|evidence| DirectControlFlowDocument {
                                        caller: evidence.caller.clone(),
                                        site_offset: format!("{:#x}", evidence.site_offset),
                                        kind: evidence.kind.label(),
                                    })
                                    .collect(),
                                reviewed: false,
                            })
                            .collect(),
                        recovery_blockers: coverage
                            .recovery_blockers
                            .iter()
                            .map(|blocker| RecoveryBlockerDocument {
                                symbol: blocker.symbol.clone(),
                                message: blocker.message.clone(),
                            })
                            .collect(),
                    }
                })
            })
            .collect(),
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
                origin_association: symbol.origin_association.label(),
                origin_candidates: symbol
                    .origin_candidates
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
            executable_sections: inventory
                .artifacts
                .iter()
                .map(|artifact| artifact.code_sections.len())
                .sum(),
            executable_bytes: inventory
                .artifacts
                .iter()
                .flat_map(|artifact| &artifact.code_sections)
                .map(|section| section.coverage.size)
                .sum(),
            symbol_covered_bytes: inventory
                .artifacts
                .iter()
                .flat_map(|artifact| &artifact.code_sections)
                .map(|section| section.coverage.symbol_covered_bytes)
                .sum(),
            uncovered_executable_bytes: inventory
                .artifacts
                .iter()
                .flat_map(|artifact| &artifact.code_sections)
                .map(|section| section.coverage.size - section.coverage.symbol_covered_bytes)
                .sum(),
            named_zero_sized_code_symbols: inventory
                .artifacts
                .iter()
                .flat_map(|artifact| &artifact.code_sections)
                .map(|section| section.coverage.named_zero_sized_symbols)
                .sum(),
            function_boundary_candidates: inventory
                .artifacts
                .iter()
                .flat_map(|artifact| &artifact.code_sections)
                .map(|section| section.coverage.function_candidates.len())
                .sum(),
            code_recovery_blockers: inventory
                .artifacts
                .iter()
                .flat_map(|artifact| &artifact.code_sections)
                .map(|section| section.coverage.recovery_blockers.len())
                .sum(),
            link_unit_definitions: inventory
                .symbols
                .iter()
                .filter(|symbol| {
                    symbol.origin_association
                        != crate::analysis::LinkUnitOriginAssociation::NotApplicable
                })
                .count(),
            unique_archive_origins: inventory
                .symbols
                .iter()
                .filter(|symbol| {
                    symbol.origin_association
                        == crate::analysis::LinkUnitOriginAssociation::UniqueNameAndKind
                })
                .count(),
            ambiguous_archive_origins: inventory
                .symbols
                .iter()
                .filter(|symbol| {
                    symbol.origin_association
                        == crate::analysis::LinkUnitOriginAssociation::AmbiguousNameAndKind
                })
                .count(),
            missing_archive_origins: inventory
                .symbols
                .iter()
                .filter(|symbol| {
                    symbol.origin_association == crate::analysis::LinkUnitOriginAssociation::Missing
                })
                .count(),
        },
    })
}

pub(crate) fn render_symbol_inventory(document: &SymbolInventoryDocument) -> crate::Result<String> {
    let mut output = serde_json::to_string(document)?;
    output.push('\n');
    Ok(output)
}
