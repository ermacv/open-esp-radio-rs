//! Typed consumer projection for schema-v5 interface discovery facts.

#![allow(
    dead_code,
    reason = "complete stored DTOs enforce every persistent schema field"
)]

use serde::{Deserialize, Deserializer};

use crate::Result;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredInterfaceFacts {
    schema_version: u32,
    command: String,
    analysis_scope: StoredAnalysisScope,
    pub(crate) artifacts: Vec<StoredInterfaceArtifact>,
    pub(crate) calls: Vec<StoredInterfaceCall>,
    pub(crate) table_candidates: Vec<StoredInterfaceTable>,
    decode_blockers: Vec<StoredDecodeBlocker>,
    analysis_failures: Vec<StoredDecodeFailure>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredAnalysisScope {
    architecture: String,
    calling_convention: String,
    evidence: String,
    relocation_evidence: [String; 3],
    semantic_claim: bool,
    table_layout_claim: bool,
    linker_resolution_claim: bool,
    completeness_claim: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredInterfaceArtifact {
    pub(crate) index: usize,
    pub(crate) path: String,
    roles: Vec<String>,
    pub(crate) sources: Vec<String>,
    pub(crate) sha256: String,
    container: String,
    functions: usize,
    reviewed_boundaries: usize,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredInterfaceTable {
    pub(crate) artifact: usize,
    pub(crate) root: StoredInterfaceRoot,
    pub(crate) container_path: Vec<StoredInterfaceStep>,
    pub(crate) slots: Vec<StoredInterfaceSlot>,
    pub(crate) functions: Vec<String>,
    call_sites: usize,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredInterfaceSlot {
    pub(crate) offset: i32,
    pub(crate) width: u8,
    pub(crate) selector: Option<StoredInterfaceSelector>,
    pub(crate) functions: Vec<String>,
    call_sites: usize,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredInterfaceCall {
    pub(crate) artifact: usize,
    pub(crate) member: Option<String>,
    pub(crate) function: String,
    #[serde(deserialize_with = "hex_u32")]
    pub(crate) function_address: u32,
    #[serde(deserialize_with = "hex_u32")]
    pub(crate) site: u32,
    pub(crate) kind: String,
    link_register: u8,
    pub(crate) target: StoredInterfaceTarget,
    root_linkage: StoredRootLinkage,
    pub(crate) arguments: Vec<StoredInterfaceArgument>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredInterfaceTarget {
    canonical: String,
    pub(crate) root: StoredInterfaceRoot,
    pub(crate) loads: Vec<StoredInterfaceStep>,
    pub(crate) container_depth: usize,
    pub(crate) slot_offset: Option<i32>,
    slot_selector: Option<StoredInterfaceSelector>,
    pub(crate) jalr_offset: i32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredInterfaceStep {
    #[serde(default)]
    site: Option<String>,
    pub(crate) offset: i32,
    pub(crate) width: u8,
    pub(crate) selector: Option<StoredInterfaceSelector>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredInterfaceSelector {
    pub(crate) argument: u8,
    pub(crate) scale: u32,
    pub(crate) addend: i32,
    canonical: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) enum StoredInterfaceRoot {
    RelocatedSymbol {
        canonical: String,
        member: Option<String>,
        symbol: String,
        addend: i64,
        addressing: String,
    },
    FunctionArgument {
        canonical: String,
        argument: u8,
    },
    AbsoluteAddress {
        canonical: String,
        #[serde(deserialize_with = "hex_u32")]
        address: u32,
    },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) enum StoredInterfaceArgument {
    Unknown {
        index: usize,
    },
    Constant {
        index: usize,
        #[serde(deserialize_with = "hex_u32")]
        value: u32,
    },
    PointerProvenance {
        index: usize,
        canonical: String,
        root: StoredInterfaceRoot,
        loads: Vec<StoredInterfaceStep>,
        post_offset: i32,
    },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredRootLinkage {
    mode: String,
    symbols: Vec<String>,
    resolutions: Vec<String>,
    candidates: Vec<StoredSymbolLocation>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredSymbolLocation {
    artifact: usize,
    member: Option<String>,
    address: String,
    kind: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredDecodeFailure {
    artifact: usize,
    member: Option<String>,
    function: String,
    error: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredDecodeBlocker {
    artifact: usize,
    member: Option<String>,
    function: String,
    address: String,
    width: u8,
    raw: String,
    class: String,
    linear_control_flow: bool,
}

pub(crate) fn parse_interface_facts(input: &str) -> Result<StoredInterfaceFacts> {
    super::expect_identity(input, super::INTERFACE_FACTS)?;
    let document: StoredInterfaceFacts = serde_json::from_str(input)?;
    if document.analysis_scope.semantic_claim
        || document.analysis_scope.table_layout_claim
        || document.analysis_scope.linker_resolution_claim
        || document.analysis_scope.completeness_claim
    {
        return Err(crate::Error::invalid(
            "interface facts artifact makes an unsupported semantic, layout, linker or completeness claim",
        ));
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
