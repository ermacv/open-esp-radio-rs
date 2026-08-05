//! Loading immutable JSON emitted by `mmio discover`.

use std::{collections::BTreeSet, fs, path::Path};

use serde_json::{Map, Value};

use crate::{Result, parse_u32};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FactRange {
    pub(crate) name: String,
    pub(crate) start: u32,
    pub(crate) end: u32,
}

impl FactRange {
    pub(crate) fn contains(&self, address: u32) -> bool {
        address >= self.start && address < self.end
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RegisterFact {
    pub(crate) address: u32,
    pub(crate) width: u8,
    pub(crate) catalog_name: String,
    pub(crate) reads: usize,
    pub(crate) writes: usize,
    pub(crate) candidate_masks: Vec<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RegisterFacts {
    pub(crate) ranges: Vec<FactRange>,
    pub(crate) registers: Vec<RegisterFact>,
}

impl RegisterFacts {
    pub(crate) fn load(path: &Path) -> Result<Self> {
        let input = fs::read_to_string(path)?;
        let root: Value = serde_json::from_str(&input)?;
        let root = object(&root, "MMIO facts root")?;
        if integer(root, "schema_version", "MMIO facts")? != 1 {
            return Err("MMIO facts require schema_version 1".into());
        }
        if string(root, "command", "MMIO facts")? != "mmio-discover" {
            return Err("register workspace requires a mmio-discover JSON report".into());
        }
        let ranges = array(root, "ranges", "MMIO facts")?
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let context = format!("ranges[{index}]");
                let value = object(value, &context)?;
                let start = address(value, "start", &context)?;
                let end = address(value, "end_exclusive", &context)?;
                if start >= end {
                    return Err(format!("{context} is empty or reversed").into());
                }
                Ok(FactRange {
                    name: string(value, "name", &context)?.to_owned(),
                    start,
                    end,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let registers = array(root, "registers", "MMIO facts")?
            .iter()
            .enumerate()
            .map(|(index, value)| parse_register(value, index))
            .collect::<Result<Vec<_>>>()?;
        let facts = Self { ranges, registers };
        facts.validate()?;
        Ok(facts)
    }

    pub(crate) fn range_for(&self, address: u32) -> Option<&FactRange> {
        self.ranges.iter().find(|range| range.contains(address))
    }

    fn validate(&self) -> Result<()> {
        if self.ranges.is_empty() {
            return Err("MMIO facts contain no discovery ranges".into());
        }
        let mut range_names = BTreeSet::new();
        for range in &self.ranges {
            if range.name.is_empty() || !range_names.insert(range.name.as_str()) {
                return Err(
                    format!("invalid or duplicate MMIO fact range {:?}", range.name).into(),
                );
            }
        }
        for (index, left) in self.ranges.iter().enumerate() {
            for right in self.ranges.iter().skip(index + 1) {
                if left.end > right.start && right.end > left.start {
                    return Err(format!(
                        "MMIO fact ranges {:?} and {:?} overlap",
                        left.name, right.name
                    )
                    .into());
                }
            }
        }
        let mut keys = BTreeSet::new();
        for register in &self.registers {
            if !matches!(register.width, 8 | 16 | 32) {
                return Err(format!(
                    "MMIO fact at {:#010x} has unsupported width {}",
                    register.address, register.width
                )
                .into());
            }
            if !keys.insert((register.address, register.width)) {
                return Err(format!(
                    "duplicate MMIO fact at {:#010x}/{}",
                    register.address, register.width
                )
                .into());
            }
            let width_mask = if register.width == 32 {
                u32::MAX
            } else {
                (1_u32 << register.width) - 1
            };
            if register
                .candidate_masks
                .iter()
                .any(|mask| mask & !width_mask != 0)
            {
                return Err(format!(
                    "MMIO fact candidate mask at {:#010x}/{} exceeds its width",
                    register.address, register.width
                )
                .into());
            }
            let owners = self
                .ranges
                .iter()
                .filter(|range| range.contains(register.address))
                .count();
            if owners != 1 {
                return Err(format!(
                    "MMIO fact at {:#010x}/{} belongs to {owners} ranges",
                    register.address, register.width
                )
                .into());
            }
        }
        Ok(())
    }
}

fn parse_register(value: &Value, index: usize) -> Result<RegisterFact> {
    let context = format!("registers[{index}]");
    let value = object(value, &context)?;
    let width = integer(value, "width", &context)?
        .try_into()
        .map_err(|_| format!("invalid width in {context}"))?;
    let mut candidate_masks = Vec::new();
    for (pattern_index, pattern) in array(value, "write_patterns", &context)?.iter().enumerate() {
        let pattern_context = format!("{context}.write_patterns[{pattern_index}]");
        candidate_masks.push(address(
            object(pattern, &pattern_context)?,
            "modified_mask",
            &pattern_context,
        )?);
    }
    candidate_masks.sort_unstable();
    candidate_masks.dedup();
    Ok(RegisterFact {
        address: address(value, "address", &context)?,
        width,
        catalog_name: string(value, "name", &context)?.to_owned(),
        reads: integer(value, "reads", &context)?
            .try_into()
            .map_err(|_| format!("invalid read count in {context}"))?,
        writes: integer(value, "writes", &context)?
            .try_into()
            .map_err(|_| format!("invalid write count in {context}"))?,
        candidate_masks,
    })
}

fn object<'a>(value: &'a Value, context: &str) -> Result<&'a Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| format!("{context} must be an object").into())
}

fn array<'a>(object: &'a Map<String, Value>, key: &str, context: &str) -> Result<&'a [Value]> {
    object
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| format!("{context} requires array {key:?}").into())
}

fn string<'a>(object: &'a Map<String, Value>, key: &str, context: &str) -> Result<&'a str> {
    object
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{context} requires string {key:?}").into())
}

fn integer(object: &Map<String, Value>, key: &str, context: &str) -> Result<u64> {
    object
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("{context} requires non-negative integer {key:?}").into())
}

fn address(object: &Map<String, Value>, key: &str, context: &str) -> Result<u32> {
    let value = string(object, key, context)?;
    parse_u32(value).ok_or_else(|| format!("invalid address {value:?} in {context}").into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_discovery_identity_and_candidate_masks() {
        let path = std::env::temp_dir().join(format!(
            "vendor-validator-register-facts-{}.json",
            std::process::id()
        ));
        std::fs::write(
            &path,
            r#"{
  "schema_version": 1,
  "command": "mmio-discover",
  "ranges": [{"name":"radio","start":"0x1000","end_exclusive":"0x2000"}],
  "registers": [{"address":"0x1010","width":32,"name":"UNMAPPED","reads":1,"writes":2,"write_patterns":[{"modified_mask":"0x3"}]}]
}"#,
        )
        .unwrap();
        let facts = RegisterFacts::load(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(facts.registers[0].candidate_masks, [3]);
        assert_eq!(facts.range_for(0x1010).unwrap().name, "radio");
    }
}
