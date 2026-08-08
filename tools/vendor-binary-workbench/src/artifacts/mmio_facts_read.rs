//! Typed consumer projection for schema-v2 MMIO discovery facts.

use serde::{Deserialize, Deserializer};

use crate::Result;

#[derive(Debug, Deserialize)]
pub(crate) struct StoredMmioFacts {
    schema_version: u32,
    command: String,
    pub(crate) ranges: Vec<StoredMmioRange>,
    pub(crate) registers: Vec<StoredRegisterFact>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct StoredMmioRange {
    pub(crate) name: String,
    #[serde(deserialize_with = "hex_u32")]
    pub(crate) start: u32,
    #[serde(deserialize_with = "hex_u32")]
    pub(crate) end_exclusive: u32,
}

#[derive(Debug, Deserialize)]
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
pub(crate) struct StoredWritePattern {
    pub(crate) occurrences: usize,
    #[serde(deserialize_with = "hex_u32")]
    pub(crate) modified_mask: u32,
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

pub(crate) fn parse_mmio_facts(input: &str) -> Result<StoredMmioFacts> {
    let document: StoredMmioFacts = serde_json::from_str(input)?;
    if document.schema_version != super::MMIO_FACTS.version
        || document.command != super::MMIO_FACTS.command
    {
        return Err(crate::Error::invalid(format!(
            "expected schema-v{} {} artifact",
            super::MMIO_FACTS.version,
            super::MMIO_FACTS.command
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
