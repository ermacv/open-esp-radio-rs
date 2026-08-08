//! Loading immutable JSON emitted by `mmio discover`.

use std::{collections::BTreeSet, fs, path::Path};

use serde_json::{Map, Value};

use crate::{Result, error::WorkbenchError, parse_u32};

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
pub(crate) struct RegisterWritePatternFact {
    pub(crate) occurrences: usize,
    pub(crate) modified_mask: u32,
    pub(crate) preserved_mask: u32,
    pub(crate) inverted_mask: u32,
    pub(crate) forced_zero_mask: u32,
    pub(crate) forced_one_mask: u32,
    pub(crate) read_derived_mask: u32,
    pub(crate) dynamic_mask: u32,
    pub(crate) functions: BTreeSet<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RegisterFact {
    pub(crate) address: u32,
    pub(crate) width: u8,
    pub(crate) catalog_name: String,
    pub(crate) reads: usize,
    pub(crate) writes: usize,
    pub(crate) read_functions: BTreeSet<String>,
    pub(crate) write_functions: BTreeSet<String>,
    pub(crate) write_patterns: Vec<RegisterWritePatternFact>,
    pub(crate) candidate_masks: Vec<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RegisterFacts {
    pub(crate) ranges: Vec<FactRange>,
    pub(crate) registers: Vec<RegisterFact>,
}

impl RegisterFacts {
    #[tracing::instrument(name = "load_register_facts", fields(path = %path.display()))]
    pub(crate) fn load(path: &Path) -> Result<Self> {
        let input = fs::read_to_string(path)?;
        Self::parse(&input).map_err(|error| {
            WorkbenchError::manifest_document("MMIO discovery report", path, &input, error)
        })
    }

    fn parse(input: &str) -> Result<Self> {
        let root: Value = serde_json::from_str(input)?;
        let root = object(&root, "MMIO facts root")?;
        if integer(root, "schema_version", "MMIO facts")? != 2 {
            return Err(crate::Error::invalid("MMIO facts require schema_version 2"));
        }
        if string(root, "command", "MMIO facts")? != "mmio discover" {
            return Err(crate::Error::invalid(
                "register workspace requires an mmio discover JSON report",
            ));
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
                    return Err(crate::Error::invalid(format!(
                        "{context} is empty or reversed"
                    )));
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

    fn validate(&self) -> Result<()> {
        if self.ranges.is_empty() {
            return Err(crate::Error::invalid(
                "MMIO facts contain no discovery ranges",
            ));
        }
        let mut range_names = BTreeSet::new();
        for range in &self.ranges {
            if range.name.is_empty() || !range_names.insert(range.name.as_str()) {
                return Err(crate::Error::invalid(format!(
                    "invalid or duplicate MMIO fact range {:?}",
                    range.name
                )));
            }
        }
        for (index, left) in self.ranges.iter().enumerate() {
            for right in self.ranges.iter().skip(index + 1) {
                if left.end > right.start && right.end > left.start {
                    return Err(crate::Error::invalid(format!(
                        "MMIO fact ranges {:?} and {:?} overlap",
                        left.name, right.name
                    )));
                }
            }
        }
        let mut keys = BTreeSet::new();
        for register in &self.registers {
            if !matches!(register.width, 8 | 16 | 32) {
                return Err(crate::Error::invalid(format!(
                    "MMIO fact at {:#010x} has unsupported width {}",
                    register.address, register.width
                )));
            }
            if !keys.insert((register.address, register.width)) {
                return Err(crate::Error::invalid(format!(
                    "duplicate MMIO fact at {:#010x}/{}",
                    register.address, register.width
                )));
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
                return Err(crate::Error::invalid(format!(
                    "MMIO fact candidate mask at {:#010x}/{} exceeds its width",
                    register.address, register.width
                )));
            }
            let owners = self
                .ranges
                .iter()
                .filter(|range| range.contains(register.address))
                .count();
            if owners != 1 {
                return Err(crate::Error::invalid(format!(
                    "MMIO fact at {:#010x}/{} belongs to {owners} ranges",
                    register.address, register.width
                )));
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
        .map_err(|_| format!("invalid width in {context}"))
        .map_err(crate::Error::invalid)?;
    let mut write_patterns = Vec::new();
    for (pattern_index, pattern) in array(value, "write_patterns", &context)?.iter().enumerate() {
        let pattern_context = format!("{context}.write_patterns[{pattern_index}]");
        let pattern = object(pattern, &pattern_context)?;
        write_patterns.push(RegisterWritePatternFact {
            occurrences: integer(pattern, "occurrences", &pattern_context)?
                .try_into()
                .map_err(|_| format!("invalid occurrence count in {pattern_context}"))
                .map_err(crate::Error::invalid)?,
            modified_mask: address(pattern, "modified_mask", &pattern_context)?,
            preserved_mask: address(pattern, "preserved_mask", &pattern_context)?,
            inverted_mask: address(pattern, "inverted_mask", &pattern_context)?,
            forced_zero_mask: address(pattern, "forced_zero_mask", &pattern_context)?,
            forced_one_mask: address(pattern, "forced_one_mask", &pattern_context)?,
            read_derived_mask: address(pattern, "read_derived_mask", &pattern_context)?,
            dynamic_mask: address(pattern, "dynamic_mask", &pattern_context)?,
            functions: string_set(pattern, "functions", &pattern_context)?,
        });
    }
    let mut candidate_masks = write_patterns
        .iter()
        .map(|pattern| pattern.modified_mask)
        .collect::<Vec<_>>();
    candidate_masks.sort_unstable();
    candidate_masks.dedup();
    Ok(RegisterFact {
        address: address(value, "address", &context)?,
        width,
        catalog_name: string(value, "name", &context)?.to_owned(),
        reads: integer(value, "reads", &context)?
            .try_into()
            .map_err(|_| format!("invalid read count in {context}"))
            .map_err(crate::Error::invalid)?,
        writes: integer(value, "writes", &context)?
            .try_into()
            .map_err(|_| format!("invalid write count in {context}"))
            .map_err(crate::Error::invalid)?,
        read_functions: string_set(value, "read_functions", &context)?,
        write_functions: string_set(value, "write_functions", &context)?,
        write_patterns,
        candidate_masks,
    })
}

fn string_set(object: &Map<String, Value>, key: &str, context: &str) -> Result<BTreeSet<String>> {
    array(object, key, context)?
        .iter()
        .enumerate()
        .map(|(index, value)| {
            value
                .as_str()
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| {
                    crate::Error::invalid(format!(
                        "{context}.{key}[{index}] must be a non-empty string"
                    ))
                })
        })
        .collect()
}

fn object<'a>(value: &'a Value, context: &str) -> Result<&'a Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| crate::Error::invalid(format!("{context} must be an object")))
}

fn array<'a>(object: &'a Map<String, Value>, key: &str, context: &str) -> Result<&'a [Value]> {
    object
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| crate::Error::invalid(format!("{context} requires array {key:?}")))
}

fn string<'a>(object: &'a Map<String, Value>, key: &str, context: &str) -> Result<&'a str> {
    object
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| crate::Error::invalid(format!("{context} requires string {key:?}")))
}

fn integer(object: &Map<String, Value>, key: &str, context: &str) -> Result<u64> {
    object.get(key).and_then(Value::as_u64).ok_or_else(|| {
        crate::Error::invalid(format!("{context} requires non-negative integer {key:?}"))
    })
}

fn address(object: &Map<String, Value>, key: &str, context: &str) -> Result<u32> {
    let value = string(object, key, context)?;
    parse_u32(value)
        .ok_or_else(|| crate::Error::invalid(format!("invalid address {value:?} in {context}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_discovery_identity_and_candidate_masks() {
        let path = std::env::temp_dir().join(format!(
            "vendor-workbench-register-facts-{}.json",
            std::process::id()
        ));
        std::fs::write(
            &path,
            r#"{
  "schema_version": 2,
  "command": "mmio discover",
  "ranges": [{"name":"radio","start":"0x1000","end_exclusive":"0x2000"}],
  "registers": [{"address":"0x1010","width":32,"name":"UNMAPPED","reads":1,"writes":2,"read_functions":["rom:read"],"write_functions":["lib:member.o:write"],"write_patterns":[{"occurrences":2,"modified_mask":"0x3","candidate_bit_ranges":"0-1","preserved_mask":"0xfffffffc","inverted_mask":"0x0","forced_zero_mask":"0x0","forced_one_mask":"0x1","read_derived_mask":"0x0","dynamic_mask":"0x2","functions":["lib:member.o:write"]}]}]
}"#,
        )
        .unwrap();
        let facts = RegisterFacts::load(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(facts.registers[0].candidate_masks, [3]);
        assert_eq!(
            facts.registers[0].read_functions,
            ["rom:read".to_owned()].into()
        );
        assert_eq!(facts.registers[0].write_patterns[0].occurrences, 2);
        assert_eq!(facts.registers[0].write_patterns[0].dynamic_mask, 2);
        assert_eq!(facts.ranges[0].name, "radio");
        assert!(facts.ranges[0].start <= 0x1010 && 0x1010 < facts.ranges[0].end);
    }
}
