//! Typed consumer projection for schema-v4 MMIO discovery facts.

#![allow(
    dead_code,
    reason = "complete stored DTOs enforce every persistent schema field"
)]

use serde::{Deserialize, Deserializer};

use crate::Result;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredMmioFacts {
    schema_version: u32,
    command: String,
    analysis_mode: String,
    access_count_mode: String,
    completeness_claim: bool,
    code_selection: StoredCodeSelection,
    pub(crate) ranges: Vec<StoredMmioRange>,
    artifacts: Vec<StoredMmioArtifact>,
    pub(crate) registers: Vec<StoredRegisterFact>,
    diagnostics: Vec<StoredMmioDiagnostic>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredCodeSelection {
    symbols: String,
    symbol_prefix: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredMmioRange {
    pub(crate) name: String,
    #[serde(deserialize_with = "hex_u32")]
    pub(crate) start: u32,
    #[serde(deserialize_with = "hex_u32")]
    pub(crate) end_exclusive: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredMmioArtifact {
    source: String,
    artifact: StoredArtifactIdentity,
    functions: usize,
    reviewed_boundaries: usize,
    functions_with_mmio: usize,
    functions_with_diagnostics: usize,
    explored_states: usize,
    terminal_paths: usize,
    branch_sites: usize,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredArtifactIdentity {
    path: String,
    sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredRegisterFact {
    #[serde(deserialize_with = "hex_u32")]
    pub(crate) address: u32,
    pub(crate) width: u8,
    pub(crate) name: String,
    pub(crate) reads: usize,
    pub(crate) writes: usize,
    pub(crate) read_functions: Vec<String>,
    pub(crate) write_functions: Vec<String>,
    pub(crate) write_patterns: Vec<StoredWritePattern>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredWritePattern {
    pub(crate) occurrences: usize,
    #[serde(deserialize_with = "hex_u32")]
    pub(crate) modified_mask: u32,
    candidate_bit_ranges: String,
    #[serde(deserialize_with = "hex_u32")]
    pub(crate) preserved_mask: u32,
    #[serde(deserialize_with = "hex_u32")]
    pub(crate) inverted_mask: u32,
    #[serde(deserialize_with = "hex_u32")]
    pub(crate) forced_zero_mask: u32,
    #[serde(deserialize_with = "hex_u32")]
    pub(crate) forced_one_mask: u32,
    #[serde(deserialize_with = "hex_u32")]
    pub(crate) read_derived_mask: u32,
    #[serde(deserialize_with = "hex_u32")]
    pub(crate) dynamic_mask: u32,
    pub(crate) functions: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredMmioDiagnostic {
    function: String,
    scope: String,
    message: String,
}

pub(crate) fn parse_mmio_facts(input: &str) -> Result<StoredMmioFacts> {
    super::expect_identity(input, super::MMIO_FACTS)?;
    let document: StoredMmioFacts = serde_json::from_str(input)?;
    if document.analysis_mode != "best-effort"
        || document.access_count_mode != "maximum-per-path"
        || document.completeness_claim
    {
        return Err(crate::Error::invalid(
            "MMIO facts artifact makes an unsupported analysis or completeness claim",
        ));
    }
    if !matches!(document.code_selection.symbols.as_str(), "all" | "exported") {
        return Err(crate::Error::invalid(format!(
            "unsupported MMIO code-symbol selection {:?}",
            document.code_selection.symbols
        )));
    }
    Ok(document)
}

fn hex_u32<'de, D>(deserializer: D) -> std::result::Result<u32, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    crate::parse_u32(&value)
        .ok_or_else(|| serde::de::Error::custom(format!("invalid hexadecimal u32 {value:?}")))
}
