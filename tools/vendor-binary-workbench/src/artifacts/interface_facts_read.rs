//! Typed consumer projection for schema-v3 interface discovery facts.

use serde::{Deserialize, Deserializer};

use crate::Result;

#[derive(Debug, Deserialize)]
pub(crate) struct StoredInterfaceFacts {
    schema_version: u32,
    command: String,
    pub(crate) artifacts: Vec<StoredInterfaceArtifact>,
    pub(crate) calls: Vec<StoredInterfaceCall>,
    pub(crate) table_candidates: Vec<StoredInterfaceTable>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct StoredInterfaceArtifact {
    pub(crate) index: usize,
    pub(crate) path: String,
    pub(crate) sources: Vec<String>,
    pub(crate) sha256: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct StoredInterfaceTable {
    pub(crate) artifact: usize,
    pub(crate) root: StoredInterfaceRoot,
    pub(crate) container_path: Vec<StoredInterfaceStep>,
    pub(crate) slots: Vec<StoredInterfaceSlot>,
    pub(crate) functions: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct StoredInterfaceSlot {
    pub(crate) offset: i32,
    pub(crate) width: u8,
    pub(crate) selector: Option<StoredInterfaceSelector>,
    pub(crate) functions: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct StoredInterfaceCall {
    pub(crate) artifact: usize,
    pub(crate) member: Option<String>,
    pub(crate) function: String,
    #[serde(deserialize_with = "hex_u32")]
    pub(crate) function_address: u32,
    #[serde(deserialize_with = "hex_u32")]
    pub(crate) site: u32,
    pub(crate) kind: String,
    pub(crate) target: StoredInterfaceTarget,
    pub(crate) arguments: Vec<StoredInterfaceArgument>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct StoredInterfaceTarget {
    pub(crate) root: StoredInterfaceRoot,
    pub(crate) loads: Vec<StoredInterfaceStep>,
    pub(crate) container_depth: usize,
    pub(crate) slot_offset: Option<i32>,
    pub(crate) jalr_offset: i32,
}

#[derive(Debug, Deserialize)]
pub(crate) struct StoredInterfaceStep {
    pub(crate) offset: i32,
    pub(crate) width: u8,
    pub(crate) selector: Option<StoredInterfaceSelector>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub(crate) struct StoredInterfaceSelector {
    pub(crate) argument: u8,
    pub(crate) scale: u32,
    pub(crate) addend: i32,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum StoredInterfaceRoot {
    RelocatedSymbol {
        member: Option<String>,
        symbol: String,
        addend: i64,
        addressing: String,
    },
    FunctionArgument {
        argument: u8,
    },
    AbsoluteAddress {
        #[serde(deserialize_with = "hex_u32")]
        address: u32,
    },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
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
    },
}

pub(crate) fn parse_interface_facts(input: &str) -> Result<StoredInterfaceFacts> {
    let document: StoredInterfaceFacts = serde_json::from_str(input)?;
    if document.schema_version != super::INTERFACE_FACTS.version
        || document.command != super::INTERFACE_FACTS.command
    {
        return Err(crate::Error::invalid(format!(
            "expected schema-v{} {} artifact",
            super::INTERFACE_FACTS.version,
            super::INTERFACE_FACTS.command
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
