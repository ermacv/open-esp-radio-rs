//! Strict consumer projection and summary inspection for symbol inventory.

#![allow(
    dead_code,
    reason = "complete stored DTOs enforce every persistent schema field"
)]

use std::{fs::File, io::BufReader, path::Path};

use serde::Deserialize;

use super::super::SYMBOL_INVENTORY;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SymbolInventorySummary {
    pub(crate) artifacts: usize,
    pub(crate) symbol_facts: usize,
    pub(crate) exported_definitions: usize,
    pub(crate) undefined: usize,
    pub(crate) unresolved_or_associated: usize,
    pub(crate) executable_bytes: u64,
    pub(crate) symbol_covered_bytes: u64,
    pub(crate) uncovered_executable_bytes: u64,
    pub(crate) named_zero_sized_code_symbols: usize,
    pub(crate) function_boundary_candidates: usize,
    pub(crate) code_recovery_blockers: usize,
    pub(crate) link_unit_definitions: usize,
    pub(crate) unique_archive_origins: usize,
    pub(crate) ambiguous_archive_origins: usize,
    pub(crate) missing_archive_origins: usize,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct CodeBoundaryInputFact {
    pub(crate) source: String,
    pub(crate) artifact_sha256: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct CodeBoundaryCandidateFact {
    pub(crate) source: String,
    pub(crate) artifact_sha256: String,
    pub(crate) member: Option<String>,
    pub(crate) object_kind: String,
    pub(crate) section: String,
    pub(crate) section_address: u64,
    pub(crate) entry_offset: u64,
    pub(crate) end_limit_offset: u64,
    pub(crate) symbol_names: Vec<String>,
    pub(crate) direct_control_flow: Vec<CodeBoundaryControlFlowFact>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct CodeBoundaryControlFlowFact {
    pub(crate) caller: String,
    pub(crate) site_offset: u64,
    pub(crate) kind: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CodeBoundaryFacts {
    pub(crate) inputs: Vec<CodeBoundaryInputFact>,
    pub(crate) candidates: Vec<CodeBoundaryCandidateFact>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredSymbolInventory {
    schema_version: u32,
    command: String,
    linkage_mode: String,
    linker_resolution_claim: bool,
    pub(crate) artifacts: Vec<StoredSymbolArtifact>,
    code_sections: Vec<StoredCodeSection>,
    pub(crate) symbols: Vec<StoredSymbolFact>,
    summary: StoredSummaryDocument,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredCodeSection {
    artifact: usize,
    member: Option<String>,
    object_kind: String,
    section: String,
    address: String,
    executable_bytes: u64,
    named_sized_symbols: usize,
    named_zero_sized_symbols: usize,
    symbol_covered_bytes: u64,
    uncovered_bytes: u64,
    uncovered_ranges: Vec<StoredCodeRange>,
    function_candidates: Vec<StoredFunctionCandidate>,
    recovery_blockers: Vec<StoredRecoveryBlocker>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredCodeRange {
    start_offset: String,
    end_offset: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredFunctionCandidate {
    entry_offset: String,
    end_limit_offset: String,
    symbol_names: Vec<String>,
    direct_control_flow: Vec<StoredDirectControlFlow>,
    reviewed: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredDirectControlFlow {
    caller: String,
    site_offset: String,
    kind: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredRecoveryBlocker {
    symbol: String,
    message: String,
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
    origin_association: String,
    origin_candidates: Vec<StoredSymbolCandidate>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredSymbolCandidate {
    artifact: usize,
    member: Option<String>,
    address: String,
    kind: String,
}

/// Allocation-light projection used by linked-IR generation. The complete
/// strict DTO above remains the validation/review reader; this projection
/// deliberately ignores code coverage and non-origin symbol fields.
#[derive(Deserialize)]
struct OriginInventoryProjection {
    schema_version: u32,
    command: String,
    linkage_mode: String,
    linker_resolution_claim: bool,
    artifacts: Vec<OriginArtifactProjection>,
    symbols: Vec<OriginSymbolProjection>,
}

#[derive(Deserialize)]
struct OriginArtifactProjection {
    index: usize,
    artifact: OriginArtifactIdentityProjection,
    sources: Vec<String>,
}

#[derive(Deserialize)]
struct OriginArtifactIdentityProjection {
    sha256: String,
}

#[derive(Deserialize)]
struct OriginSymbolProjection {
    artifact: usize,
    member: Option<String>,
    name: String,
    address: String,
    kind: String,
    origin_association: String,
    origin_candidates: Vec<OriginCandidateProjection>,
}

#[derive(Deserialize)]
struct OriginCandidateProjection {
    artifact: usize,
    member: Option<String>,
    address: String,
}

/// Exact, association-only provenance from one authoritative link-unit
/// definition to its sole archive definition candidate.
///
/// This does not claim that Blobray reproduced linker selection. It is
/// only emitted for the inventory's fail-closed `unique-name-and-kind` case.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct LinkUnitOriginFact {
    pub(crate) linked_sources: Vec<String>,
    pub(crate) linked_artifact_sha256: String,
    pub(crate) linked_member: Option<String>,
    pub(crate) symbol: String,
    pub(crate) linked_address: u64,
    pub(crate) kind: String,
    pub(crate) origin_sources: Vec<String>,
    pub(crate) origin_artifact_sha256: String,
    pub(crate) origin_member: Option<String>,
    pub(crate) origin_address: u64,
}

pub(crate) fn parse_symbol_inventory(input: &str) -> crate::Result<StoredSymbolInventory> {
    super::super::expect_identity(input, SYMBOL_INVENTORY)?;
    let document: StoredSymbolInventory = serde_json::from_str(input)?;
    if document.linkage_mode != "association-only" || document.linker_resolution_claim {
        return Err(crate::Error::invalid(
            "symbol inventory makes an unsupported linker-resolution claim",
        ));
    }
    let mut executable_bytes = 0_u64;
    let mut symbol_covered_bytes = 0_u64;
    let mut uncovered_executable_bytes = 0_u64;
    let mut named_zero_sized_code_symbols = 0usize;
    let mut function_boundary_candidates = 0usize;
    let mut code_recovery_blockers = 0usize;
    for section in &document.code_sections {
        if section.symbol_covered_bytes > section.executable_bytes
            || section.uncovered_bytes != section.executable_bytes - section.symbol_covered_bytes
        {
            return Err(crate::Error::invalid(format!(
                "invalid executable-code coverage for section {:?}",
                section.section
            )));
        }
        let mut cursor = 0_u64;
        let mut range_bytes = 0_u64;
        for range in &section.uncovered_ranges {
            let start = hex_u64(&range.start_offset)?;
            let end = hex_u64(&range.end_offset)?;
            if start < cursor || start >= end || end > section.executable_bytes {
                return Err(crate::Error::invalid(format!(
                    "invalid uncovered executable range {:?}..{:?} in section {:?}",
                    range.start_offset, range.end_offset, section.section
                )));
            }
            range_bytes += end - start;
            cursor = end;
        }
        if range_bytes != section.uncovered_bytes {
            return Err(crate::Error::invalid(format!(
                "uncovered ranges do not account for section {:?}",
                section.section
            )));
        }
        let uncovered_contains = |start: u64, end: u64| {
            section.uncovered_ranges.iter().any(|range| {
                let Ok(range_start) = hex_u64(&range.start_offset) else {
                    return false;
                };
                let Ok(range_end) = hex_u64(&range.end_offset) else {
                    return false;
                };
                start >= range_start && start < end && end <= range_end
            })
        };
        let mut previous_entry = None;
        for candidate in &section.function_candidates {
            let entry = hex_u64(&candidate.entry_offset)?;
            let end_limit = hex_u64(&candidate.end_limit_offset)?;
            if candidate.reviewed
                || candidate.symbol_names.is_empty() && candidate.direct_control_flow.is_empty()
                || previous_entry.is_some_and(|previous| entry <= previous)
                || !uncovered_contains(entry, end_limit)
            {
                return Err(crate::Error::invalid(format!(
                    "invalid unreviewed function-boundary candidate at {:?} in section {:?}",
                    candidate.entry_offset, section.section
                )));
            }
            for evidence in &candidate.direct_control_flow {
                if !matches!(evidence.kind.as_str(), "call" | "tail-call") {
                    return Err(crate::Error::invalid(format!(
                        "unsupported direct-control-flow evidence kind {:?}",
                        evidence.kind
                    )));
                }
                let site = hex_u64(&evidence.site_offset)?;
                if site >= section.executable_bytes {
                    return Err(crate::Error::invalid(format!(
                        "direct-control-flow evidence site {:?} is outside section {:?}",
                        evidence.site_offset, section.section
                    )));
                }
            }
            previous_entry = Some(entry);
        }
        executable_bytes += section.executable_bytes;
        symbol_covered_bytes += section.symbol_covered_bytes;
        uncovered_executable_bytes += section.uncovered_bytes;
        named_zero_sized_code_symbols += section.named_zero_sized_symbols;
        function_boundary_candidates += section.function_candidates.len();
        code_recovery_blockers += section.recovery_blockers.len();
    }
    for symbol in &document.symbols {
        let valid_origin = match symbol.origin_association.as_str() {
            "not-applicable" | "missing" => symbol.origin_candidates.is_empty(),
            "unique-name-and-kind" => symbol.origin_candidates.len() == 1,
            "ambiguous-name-and-kind" => symbol.origin_candidates.len() > 1,
            _ => false,
        };
        if !valid_origin {
            return Err(crate::Error::invalid(format!(
                "invalid link-unit origin association {:?} for symbol {:?}",
                symbol.origin_association, symbol.name
            )));
        }
    }
    if document.summary.executable_sections != document.code_sections.len()
        || document.summary.executable_bytes != executable_bytes
        || document.summary.symbol_covered_bytes != symbol_covered_bytes
        || document.summary.uncovered_executable_bytes != uncovered_executable_bytes
        || document.summary.named_zero_sized_code_symbols != named_zero_sized_code_symbols
        || document.summary.function_boundary_candidates != function_boundary_candidates
        || document.summary.code_recovery_blockers != code_recovery_blockers
    {
        return Err(crate::Error::invalid(
            "symbol inventory executable-code summary does not match its sections",
        ));
    }
    Ok(document)
}

fn hex_u64(value: &str) -> crate::Result<u64> {
    let digits = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .ok_or_else(|| {
            crate::Error::invalid(format!("expected hexadecimal value, got {value:?}"))
        })?;
    u64::from_str_radix(digits, 16)
        .map_err(|_| crate::Error::invalid(format!("invalid hexadecimal value {value:?}")))
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
        executable_bytes: document.summary.executable_bytes,
        symbol_covered_bytes: document.summary.symbol_covered_bytes,
        uncovered_executable_bytes: document.summary.uncovered_executable_bytes,
        named_zero_sized_code_symbols: document.summary.named_zero_sized_code_symbols,
        function_boundary_candidates: document.summary.function_boundary_candidates,
        code_recovery_blockers: document.summary.code_recovery_blockers,
        link_unit_definitions: document.summary.link_unit_definitions,
        unique_archive_origins: document.summary.unique_archive_origins,
        ambiguous_archive_origins: document.summary.ambiguous_archive_origins,
        missing_archive_origins: document.summary.missing_archive_origins,
    })
}

pub(crate) fn load_link_unit_origins(path: &Path) -> crate::Result<Vec<LinkUnitOriginFact>> {
    let document: OriginInventoryProjection =
        serde_json::from_reader(BufReader::new(File::open(path)?)).map_err(|error| {
            crate::Error::invalid(format!(
                "unsupported symbol inventory in {}: {error}",
                path.display()
            ))
        })?;
    if document.schema_version != SYMBOL_INVENTORY.version
        || document.command != SYMBOL_INVENTORY.command
        || document.linkage_mode != "association-only"
        || document.linker_resolution_claim
    {
        return Err(crate::Error::invalid(format!(
            "expected association-only schema_version {} and command {:?} in {}",
            SYMBOL_INVENTORY.version,
            SYMBOL_INVENTORY.command,
            path.display()
        )));
    }
    let artifacts = document
        .artifacts
        .iter()
        .map(|artifact| (artifact.index, artifact))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut origins = Vec::new();
    for symbol in document.symbols {
        if symbol.origin_association != "unique-name-and-kind" {
            continue;
        }
        let linked = artifacts.get(&symbol.artifact).ok_or_else(|| {
            crate::Error::invalid(format!(
                "symbol {:?} refers to missing linked artifact {}",
                symbol.name, symbol.artifact
            ))
        })?;
        let origin = symbol
            .origin_candidates
            .first()
            .expect("validated unique origin has one candidate");
        let origin_artifact = artifacts.get(&origin.artifact).ok_or_else(|| {
            crate::Error::invalid(format!(
                "symbol {:?} refers to missing origin artifact {}",
                symbol.name, origin.artifact
            ))
        })?;
        origins.push(LinkUnitOriginFact {
            linked_sources: linked.sources.clone(),
            linked_artifact_sha256: linked.artifact.sha256.clone(),
            linked_member: symbol.member,
            symbol: symbol.name,
            linked_address: hex_u64(&symbol.address)?,
            kind: symbol.kind,
            origin_sources: origin_artifact.sources.clone(),
            origin_artifact_sha256: origin_artifact.artifact.sha256.clone(),
            origin_member: origin.member.clone(),
            origin_address: hex_u64(&origin.address)?,
        });
    }
    origins.sort();
    Ok(origins)
}

pub(crate) fn load_code_boundary_facts(path: &Path) -> crate::Result<CodeBoundaryFacts> {
    let input = std::fs::read_to_string(path)?;
    let document = parse_symbol_inventory(&input).map_err(|error| {
        crate::Error::invalid(format!(
            "unsupported symbol inventory in {}: {error}",
            path.display()
        ))
    })?;
    let mut inputs = Vec::new();
    let mut candidates = Vec::new();
    for artifact in &document.artifacts {
        for source in &artifact.sources {
            inputs.push(CodeBoundaryInputFact {
                source: source.clone(),
                artifact_sha256: artifact.artifact.sha256.clone(),
            });
        }
    }
    inputs.sort();
    inputs.dedup();
    for section in &document.code_sections {
        let artifact = document
            .artifacts
            .iter()
            .find(|artifact| artifact.index == section.artifact)
            .ok_or_else(|| {
                crate::Error::invalid(format!(
                    "code section {:?} refers to missing artifact {}",
                    section.section, section.artifact
                ))
            })?;
        let source = artifact.sources.first().ok_or_else(|| {
            crate::Error::invalid(format!(
                "code section {:?} belongs to an artifact without a logical source",
                section.section
            ))
        })?;
        for candidate in &section.function_candidates {
            candidates.push(CodeBoundaryCandidateFact {
                source: source.clone(),
                artifact_sha256: artifact.artifact.sha256.clone(),
                member: section.member.clone(),
                object_kind: section.object_kind.clone(),
                section: section.section.clone(),
                section_address: hex_u64(&section.address)?,
                entry_offset: hex_u64(&candidate.entry_offset)?,
                end_limit_offset: hex_u64(&candidate.end_limit_offset)?,
                symbol_names: candidate.symbol_names.clone(),
                direct_control_flow: candidate
                    .direct_control_flow
                    .iter()
                    .map(|edge| {
                        Ok(CodeBoundaryControlFlowFact {
                            caller: edge.caller.clone(),
                            site_offset: hex_u64(&edge.site_offset)?,
                            kind: edge.kind.clone(),
                        })
                    })
                    .collect::<crate::Result<Vec<_>>>()?,
            });
        }
    }
    candidates.sort();
    Ok(CodeBoundaryFacts { inputs, candidates })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_inventory_summary_is_strictly_versioned() {
        let path = std::env::temp_dir().join(format!(
            "blobray-symbol-inventory-{}.json",
            std::process::id()
        ));
        std::fs::write(
            &path,
            r#"{"schema_version":5,"command":"symbols inventory","linkage_mode":"association-only","linker_resolution_claim":false,"artifacts":[],"code_sections":[],"symbols":[],"summary":{"artifacts":3,"symbol_facts":40,"emitted":40,"exported_definitions":12,"undefined":7,"unresolved_or_associated":5,"executable_sections":0,"executable_bytes":0,"symbol_covered_bytes":0,"uncovered_executable_bytes":0,"named_zero_sized_code_symbols":0,"function_boundary_candidates":0,"code_recovery_blockers":0,"link_unit_definitions":0,"unique_archive_origins":0,"ambiguous_archive_origins":0,"missing_archive_origins":0}}"#,
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
                .contains("expected schema_version 5")
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn stored_inventory_rejects_unknown_and_missing_fields() {
        let input = r#"{"schema_version":5,"command":"symbols inventory","linkage_mode":"association-only","linker_resolution_claim":false,"artifacts":[],"code_sections":[],"symbols":[],"summary":{"artifacts":0,"symbol_facts":0,"emitted":0,"exported_definitions":0,"undefined":0,"unresolved_or_associated":0,"executable_sections":0,"executable_bytes":0,"symbol_covered_bytes":0,"uncovered_executable_bytes":0,"named_zero_sized_code_symbols":0,"function_boundary_candidates":0,"code_recovery_blockers":0,"link_unit_definitions":0,"unique_archive_origins":0,"ambiguous_archive_origins":0,"missing_archive_origins":0}}"#;
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

    #[test]
    fn generated_inventory_cannot_promote_a_recovery_candidate_to_reviewed() {
        let input = serde_json::json!({
            "schema_version": 5,
            "command": "symbols inventory",
            "linkage_mode": "association-only",
            "linker_resolution_claim": false,
            "artifacts": [],
            "code_sections": [{
                "artifact": 0,
                "member": null,
                "object_kind": "relocatable",
                "section": ".text",
                "address": "0x0",
                "executable_bytes": 4,
                "named_sized_symbols": 0,
                "named_zero_sized_symbols": 1,
                "symbol_covered_bytes": 0,
                "uncovered_bytes": 4,
                "uncovered_ranges": [{"start_offset":"0x0","end_offset":"0x4"}],
                "function_candidates": [{
                    "entry_offset": "0x0",
                    "end_limit_offset": "0x4",
                    "symbol_names": ["candidate"],
                    "direct_control_flow": [],
                    "reviewed": true
                }],
                "recovery_blockers": []
            }],
            "symbols": [],
            "summary": {
                "artifacts": 0,
                "symbol_facts": 0,
                "emitted": 0,
                "exported_definitions": 0,
                "undefined": 0,
                "unresolved_or_associated": 0,
                "executable_sections": 1,
                "executable_bytes": 4,
                "symbol_covered_bytes": 0,
                "uncovered_executable_bytes": 4,
                "named_zero_sized_code_symbols": 1,
                "function_boundary_candidates": 1,
                "code_recovery_blockers": 0,
                "link_unit_definitions": 0,
                "unique_archive_origins": 0,
                "ambiguous_archive_origins": 0,
                "missing_archive_origins": 0
            }
        });
        let error = parse_symbol_inventory(&input.to_string()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("invalid unreviewed function-boundary candidate")
        );
    }
}
