//! Loading immutable JSON emitted by `interfaces discover`.

use std::{collections::BTreeSet, fs, path::Path};

use serde_json::{Map, Value};

use crate::{Result, parse_u32};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum InterfaceFactRoot {
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
        address: u32,
    },
}

impl InterfaceFactRoot {
    pub(crate) const fn kind(&self) -> &'static str {
        match self {
            Self::RelocatedSymbol { .. } => "relocated-symbol",
            Self::FunctionArgument { .. } => "function-argument",
            Self::AbsoluteAddress { .. } => "absolute-address",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct InterfaceFactStep {
    pub(crate) offset: i32,
    pub(crate) width: u8,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct InterfaceFactSlot {
    pub(crate) offset: i32,
    pub(crate) width: u8,
    pub(crate) functions: BTreeSet<String>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct InterfaceTableFact {
    pub(crate) artifact: usize,
    pub(crate) root: InterfaceFactRoot,
    pub(crate) container_path: Vec<InterfaceFactStep>,
    pub(crate) slots: Vec<InterfaceFactSlot>,
    pub(crate) functions: BTreeSet<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InterfaceFactArtifact {
    pub(crate) index: usize,
    pub(crate) sources: BTreeSet<String>,
    pub(crate) sha256: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InterfaceFacts {
    pub(crate) artifacts: Vec<InterfaceFactArtifact>,
    pub(crate) tables: Vec<InterfaceTableFact>,
}

impl InterfaceFacts {
    pub(crate) fn load(path: &Path) -> Result<Self> {
        let input = fs::read_to_string(path)?;
        let root: Value = serde_json::from_str(&input)?;
        let root = object(&root, "interface facts root")?;
        if integer(root, "schema_version", "interface facts")? != 1 {
            return Err("interface facts require schema_version 1".into());
        }
        if string(root, "command", "interface facts")? != "interfaces-discover" {
            return Err("interface workspace requires an interfaces-discover JSON report".into());
        }
        let artifacts = array(root, "artifacts", "interface facts")?
            .iter()
            .enumerate()
            .map(|(index, value)| parse_artifact(value, index))
            .collect::<Result<Vec<_>>>()?;
        let tables = array(root, "table_candidates", "interface facts")?
            .iter()
            .enumerate()
            .map(|(index, value)| parse_table(value, index))
            .collect::<Result<Vec<_>>>()?;
        let facts = Self { artifacts, tables };
        facts.validate()?;
        Ok(facts)
    }

    pub(crate) fn artifact(&self, index: usize) -> Option<&InterfaceFactArtifact> {
        self.artifacts
            .iter()
            .find(|artifact| artifact.index == index)
    }

    pub(crate) fn observed_slots(&self) -> usize {
        self.tables.iter().map(|table| table.slots.len()).sum()
    }

    fn validate(&self) -> Result<()> {
        let mut artifact_indices = BTreeSet::new();
        for artifact in &self.artifacts {
            if !artifact_indices.insert(artifact.index) {
                return Err(
                    format!("duplicate interface artifact index {}", artifact.index).into(),
                );
            }
            if artifact.sources.is_empty() {
                return Err(format!(
                    "interface artifact {} has no logical source identity",
                    artifact.index
                )
                .into());
            }
            if let Some(digest) = &artifact.sha256 {
                validate_sha256(digest, "interface artifact")?;
            }
        }
        let mut table_keys = BTreeSet::new();
        for table in &self.tables {
            if self.artifact(table.artifact).is_none() {
                return Err(format!(
                    "interface table refers to unknown artifact {}",
                    table.artifact
                )
                .into());
            }
            let key = (table.artifact, &table.root, table.container_path.as_slice());
            if !table_keys.insert(key) {
                return Err("duplicate interface table candidate".into());
            }
            validate_steps(&table.container_path, "interface container path")?;
            validate_slots(&table.slots, "interface slots")?;
            if table.slots.is_empty() {
                return Err("interface table candidate has no observed slots".into());
            }
            if table.functions.is_empty() {
                return Err("interface table candidate has no calling functions".into());
            }
        }
        Ok(())
    }
}

pub(crate) fn validate_sha256(value: &str, context: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(format!("{context} has invalid lowercase SHA-256 {value:?}").into());
    }
    Ok(())
}

fn validate_steps(steps: &[InterfaceFactStep], context: &str) -> Result<()> {
    let mut keys = BTreeSet::new();
    for step in steps {
        if !matches!(step.width, 8 | 16 | 32 | 64) {
            return Err(format!("{context} has unsupported width {}", step.width).into());
        }
        if !keys.insert((step.offset, step.width)) {
            return Err(format!("{context} contains a duplicate step").into());
        }
    }
    Ok(())
}

fn validate_slots(slots: &[InterfaceFactSlot], context: &str) -> Result<()> {
    let mut keys = BTreeSet::new();
    for slot in slots {
        if !matches!(slot.width, 8 | 16 | 32 | 64) {
            return Err(format!("{context} has unsupported width {}", slot.width).into());
        }
        if !keys.insert((slot.offset, slot.width)) {
            return Err(format!("{context} contains a duplicate slot").into());
        }
        if slot.functions.is_empty() {
            return Err(format!("{context} contains a slot without calling functions").into());
        }
    }
    Ok(())
}

fn parse_artifact(value: &Value, index: usize) -> Result<InterfaceFactArtifact> {
    let context = format!("artifacts[{index}]");
    let value = object(value, &context)?;
    Ok(InterfaceFactArtifact {
        index: usize::try_from(integer(value, "index", &context)?)
            .map_err(|_| format!("invalid artifact index in {context}"))?,
        sources: array(value, "sources", &context)?
            .iter()
            .enumerate()
            .map(|(source_index, value)| {
                value
                    .as_str()
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
                    .ok_or_else(|| {
                        format!("{context}.sources[{source_index}] must be a non-empty string")
                            .into()
                    })
            })
            .collect::<Result<_>>()?,
        sha256: value
            .get("sha256")
            .map(|value| -> Result<String> {
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| format!("{context}.sha256 must be a string").into())
            })
            .transpose()?,
    })
}

fn parse_table(value: &Value, index: usize) -> Result<InterfaceTableFact> {
    let context = format!("table_candidates[{index}]");
    let value = object(value, &context)?;
    let functions = array(value, "functions", &context)?
        .iter()
        .enumerate()
        .map(|(function_index, value)| {
            value
                .as_str()
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| {
                    format!("{context}.functions[{function_index}] must be a non-empty string")
                        .into()
                })
        })
        .collect::<Result<BTreeSet<_>>>()?;
    Ok(InterfaceTableFact {
        artifact: usize::try_from(integer(value, "artifact", &context)?)
            .map_err(|_| format!("invalid artifact index in {context}"))?,
        root: parse_root(
            value
                .get("root")
                .ok_or_else(|| format!("{context} requires object \"root\""))?,
            &format!("{context}.root"),
        )?,
        container_path: parse_steps(value, "container_path", &context)?,
        slots: parse_slots(value, &context, &functions)?,
        functions,
    })
}

fn parse_slots(
    object: &Map<String, Value>,
    context: &str,
    fallback_functions: &BTreeSet<String>,
) -> Result<Vec<InterfaceFactSlot>> {
    array(object, "slots", context)?
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let context = format!("{context}.slots[{index}]");
            let value = self::object(value, &context)?;
            let functions = value
                .get("functions")
                .map(|_| {
                    array(value, "functions", &context)?
                        .iter()
                        .enumerate()
                        .map(|(function_index, value)| {
                            value
                                .as_str()
                                .filter(|value| !value.is_empty())
                                .map(str::to_owned)
                                .ok_or_else(|| {
                                    format!(
                                        "{context}.functions[{function_index}] must be a non-empty string"
                                    )
                                    .into()
                                })
                        })
                        .collect::<Result<BTreeSet<_>>>()
                })
                .transpose()?
                .unwrap_or_else(|| fallback_functions.clone());
            Ok(InterfaceFactSlot {
                offset: signed_integer(value, "offset", &context)?
                    .try_into()
                    .map_err(|_| format!("offset does not fit i32 in {context}"))?,
                width: integer(value, "width", &context)?
                    .try_into()
                    .map_err(|_| format!("width does not fit u8 in {context}"))?,
                functions,
            })
        })
        .collect()
}

fn parse_root(value: &Value, context: &str) -> Result<InterfaceFactRoot> {
    let value = object(value, context)?;
    Ok(match string(value, "kind", context)? {
        "relocated-symbol" => InterfaceFactRoot::RelocatedSymbol {
            member: optional_string(value, "member", context)?,
            symbol: string(value, "symbol", context)?.to_owned(),
            addend: signed_integer(value, "addend", context)?,
            addressing: string(value, "addressing", context)?.to_owned(),
        },
        "function-argument" => InterfaceFactRoot::FunctionArgument {
            argument: integer(value, "argument", context)?
                .try_into()
                .map_err(|_| format!("invalid argument index in {context}"))?,
        },
        "absolute-address" => InterfaceFactRoot::AbsoluteAddress {
            address: address(value, "address", context)?,
        },
        kind => {
            return Err(format!("unsupported interface root kind {kind:?} in {context}").into());
        }
    })
}

fn parse_steps(
    object: &Map<String, Value>,
    key: &str,
    context: &str,
) -> Result<Vec<InterfaceFactStep>> {
    array(object, key, context)?
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let context = format!("{context}.{key}[{index}]");
            let value = self::object(value, &context)?;
            Ok(InterfaceFactStep {
                offset: signed_integer(value, "offset", &context)?
                    .try_into()
                    .map_err(|_| format!("offset does not fit i32 in {context}"))?,
                width: integer(value, "width", &context)?
                    .try_into()
                    .map_err(|_| format!("width does not fit u8 in {context}"))?,
            })
        })
        .collect()
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

fn optional_string(
    object: &Map<String, Value>,
    key: &str,
    context: &str,
) -> Result<Option<String>> {
    object
        .get(key)
        .filter(|value| !value.is_null())
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("{context}.{key} must be a string or null").into())
        })
        .transpose()
}

fn integer(object: &Map<String, Value>, key: &str, context: &str) -> Result<u64> {
    object
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("{context} requires non-negative integer {key:?}").into())
}

fn signed_integer(object: &Map<String, Value>, key: &str, context: &str) -> Result<i64> {
    object
        .get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| format!("{context} requires integer {key:?}").into())
}

fn address(object: &Map<String, Value>, key: &str, context: &str) -> Result<u32> {
    let value = string(object, key, context)?;
    parse_u32(value).ok_or_else(|| format!("invalid address {value:?} in {context}").into())
}
