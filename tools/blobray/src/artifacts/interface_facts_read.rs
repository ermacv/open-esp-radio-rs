//! Typed consumer projection for schema-v11 interface discovery facts.

#![allow(
    dead_code,
    reason = "complete stored DTOs enforce every persistent schema field"
)]

use serde::{Deserialize, Deserializer};

use crate::Result;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredInterfaceFacts {
    pub(crate) limits: crate::interface_discovery::InterfaceDiscoveryLimits,
    pub(crate) gaps: Vec<crate::interfaces::InterfaceGapFact>,
    schema_version: u32,
    command: String,
    analysis_scope: StoredAnalysisScope,
    pub(crate) artifacts: Vec<StoredInterfaceArtifact>,
    pub(crate) calls: Vec<StoredInterfaceCall>,
    pub(crate) assignments: Vec<StoredInterfaceAssignment>,
    pub(crate) table_candidates: Vec<StoredInterfaceTable>,
    pub(crate) decode_blockers: Vec<StoredDecodeBlocker>,
    pub(crate) analysis_failures: Vec<StoredDecodeFailure>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredInterfaceAssignment {
    pub(crate) owner: crate::artifact::CodeIdentity,
    pub(crate) target_loads: Vec<StoredInterfaceStep>,
    pub(crate) target_offset: i32,
    pub(crate) artifact: usize,
    pub(crate) member: Option<String>,
    pub(crate) function: String,
    #[serde(deserialize_with = "hex_u32")]
    pub(crate) function_address: u32,
    #[serde(deserialize_with = "hex_u32")]
    pub(crate) site: u32,
    pub(crate) root: StoredInterfaceRoot,
    pub(crate) container_path: Vec<StoredInterfaceStep>,
    pub(crate) offset: i32,
    pub(crate) width: u8,
    pub(crate) target: StoredInterfaceRoot,
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
    pub(crate) container_path: Vec<StoredInterfaceShape>,
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
    pub(crate) owner: crate::artifact::CodeIdentity,
    pub(crate) artifact: usize,
    pub(crate) member: Option<String>,
    pub(crate) function: String,
    #[serde(deserialize_with = "hex_u32")]
    pub(crate) function_address: u32,
    #[serde(deserialize_with = "hex_u32")]
    pub(crate) site: u32,
    pub(crate) kind: String,
    pub(crate) link_register: u8,
    pub(crate) target: StoredInterfaceTarget,
    pub(crate) root_linkage: StoredRootLinkage,
    pub(crate) arguments: Vec<StoredInterfaceArgument>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredInterfaceTarget {
    pub(crate) post_offset: i32,
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
    #[serde(deserialize_with = "hex_u32")]
    pub(crate) site: u32,
    pub(crate) offset: i32,
    pub(crate) width: u8,
    pub(crate) selector: Option<StoredInterfaceSelector>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredInterfaceShape {
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
        reference: open_radio_vendor_contracts::SymbolReference,
        canonical: String,
        member: Option<String>,
        symbol: String,
        addend: i64,
        addressing: String,
    },
    FunctionArgument {
        owner: crate::artifact::CodeIdentity,
        canonical: String,
        argument: u8,
    },
    AbsoluteAddress {
        data_address: open_radio_vendor_contracts::DataAddressResolution,
        canonical: String,
        #[serde(deserialize_with = "hex_u32")]
        address: u32,
    },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredInterfaceArgument {
    pub(crate) index: usize,
    pub(crate) value: crate::interface_discovery::InterfaceArgumentValue,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredRootLinkage {
    pub(crate) mode: String,
    pub(crate) symbols: Vec<String>,
    pub(crate) resolutions: Vec<String>,
    pub(crate) candidates: Vec<StoredSymbolLocation>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredSymbolLocation {
    pub(crate) location: crate::SymbolLocation,
    pub(crate) artifact: usize,
    pub(crate) member: Option<String>,
    pub(crate) address: String,
    pub(crate) kind: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredDecodeFailure {
    pub(crate) owner: crate::artifact::CodeIdentity,
    pub(crate) artifact: usize,
    pub(crate) member: Option<String>,
    pub(crate) function: String,
    pub(crate) error: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredDecodeBlocker {
    pub(crate) owner: crate::artifact::CodeIdentity,
    pub(crate) artifact: usize,
    pub(crate) member: Option<String>,
    pub(crate) function: String,
    pub(crate) address: String,
    pub(crate) width: u8,
    pub(crate) raw: String,
    pub(crate) class: String,
    pub(crate) linear_control_flow: bool,
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
