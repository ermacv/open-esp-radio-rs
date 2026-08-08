//! Strict JSON projection for generated interface discovery facts.

use std::collections::BTreeSet;

use serde_json::{Map, Value};

use crate::{Result, parse_u32};

use super::*;

pub(super) fn parse(input: &str) -> Result<InterfaceFacts> {
    let root: Value = serde_json::from_str(input)?;
    let root = object(&root, "interface facts root")?;
    if integer(root, "schema_version", "interface facts")? != 2 {
        return Err(crate::Error::invalid(
            "interface facts require schema_version 2",
        ));
    }
    if string(root, "command", "interface facts")? != "interfaces discover" {
        return Err(crate::Error::invalid(
            "interface workspace requires an interfaces discover JSON report",
        ));
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
    let calls = array(root, "calls", "interface facts")?
        .iter()
        .enumerate()
        .map(|(index, value)| parse_call(value, index))
        .collect::<Result<Vec<_>>>()?;
    let facts = InterfaceFacts {
        artifacts,
        tables,
        calls,
    };
    super::validate::validate(&facts)?;
    Ok(facts)
}

fn parse_artifact(value: &Value, index: usize) -> Result<InterfaceFactArtifact> {
    let context = format!("artifacts[{index}]");
    let value = object(value, &context)?;
    Ok(InterfaceFactArtifact {
        index: usize::try_from(integer(value, "index", &context)?)
            .map_err(|_| format!("invalid artifact index in {context}"))
            .map_err(crate::Error::invalid)?,
        sources: array(value, "sources", &context)?
            .iter()
            .enumerate()
            .map(|(source_index, value)| {
                value
                    .as_str()
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
                    .ok_or_else(|| {
                        crate::Error::invalid(format!(
                            "{context}.sources[{source_index}] must be a non-empty string"
                        ))
                    })
            })
            .collect::<Result<_>>()?,
        sha256: value
            .get("sha256")
            .map(|value| -> Result<String> {
                value.as_str().map(str::to_owned).ok_or_else(|| {
                    crate::Error::invalid(format!("{context}.sha256 must be a string"))
                })
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
                    crate::Error::invalid(format!(
                        "{context}.functions[{function_index}] must be a non-empty string"
                    ))
                })
        })
        .collect::<Result<BTreeSet<_>>>()?;
    Ok(InterfaceTableFact {
        artifact: usize::try_from(integer(value, "artifact", &context)?)
            .map_err(|_| format!("invalid artifact index in {context}"))
            .map_err(crate::Error::invalid)?,
        root: parse_root(
            value
                .get("root")
                .ok_or_else(|| format!("{context} requires object \"root\""))
                .map_err(crate::Error::invalid)?,
            &format!("{context}.root"),
        )?,
        container_path: parse_steps(value, "container_path", &context)?,
        slots: parse_slots(value, &context, &functions)?,
        functions,
    })
}

fn parse_call(value: &Value, index: usize) -> Result<InterfaceCallFact> {
    let context = format!("calls[{index}]");
    let value = object(value, &context)?;
    let target_context = format!("{context}.target");
    let target = object(
        value
            .get("target")
            .ok_or_else(|| format!("{context} requires object \"target\""))
            .map_err(crate::Error::invalid)?,
        &target_context,
    )?;
    Ok(InterfaceCallFact {
        artifact: usize::try_from(integer(value, "artifact", &context)?)
            .map_err(|_| format!("invalid artifact index in {context}"))
            .map_err(crate::Error::invalid)?,
        member: optional_string(value, "member", &context)?,
        function: string(value, "function", &context)?.to_owned(),
        function_address: address(value, "function_address", &context)?,
        site: address(value, "site", &context)?,
        kind: string(value, "kind", &context)?.to_owned(),
        root: parse_root(
            target
                .get("root")
                .ok_or_else(|| format!("{target_context} requires object \"root\""))
                .map_err(crate::Error::invalid)?,
            &format!("{target_context}.root"),
        )?,
        loads: parse_steps(target, "loads", &target_context)?,
        container_depth: usize::try_from(integer(target, "container_depth", &target_context)?)
            .map_err(|_| format!("invalid container depth in {target_context}"))
            .map_err(crate::Error::invalid)?,
        slot_offset: optional_signed_integer(target, "slot_offset", &target_context)?
            .map(|value| {
                value
                    .try_into()
                    .map_err(|_| format!("slot offset does not fit i32 in {target_context}"))
            })
            .transpose()
            .map_err(crate::Error::invalid)?,
        jalr_offset: signed_integer(target, "jalr_offset", &target_context)?
            .try_into()
            .map_err(|_| format!("jalr offset does not fit i32 in {target_context}"))
            .map_err(crate::Error::invalid)?,
        arguments: parse_arguments(value, &context)?,
    })
}

fn parse_arguments(
    object: &Map<String, Value>,
    context: &str,
) -> Result<Vec<InterfaceArgumentFact>> {
    array(object, "arguments", context)?
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let context = format!("{context}.arguments[{index}]");
            let value = self::object(value, &context)?;
            let kind = string(value, "kind", &context)?.to_owned();
            let expression = match kind.as_str() {
                "unknown" => "?".to_owned(),
                "constant" => format!("{:#010x}", address(value, "value", &context)?),
                "pointer-provenance" => string(value, "canonical", &context)?.to_owned(),
                _ => String::new(),
            };
            Ok(InterfaceArgumentFact {
                index: usize::try_from(integer(value, "index", &context)?)
                    .map_err(|_| format!("invalid argument index in {context}"))
                    .map_err(crate::Error::invalid)?,
                kind,
                expression,
            })
        })
        .collect()
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
                                    crate::Error::invalid(format!(
                                        "{context}.functions[{function_index}] must be a non-empty string"
                                    )
                                    )
                                })
                        })
                        .collect::<Result<BTreeSet<_>>>()
                })
                .transpose()?
                .unwrap_or_else(|| fallback_functions.clone());
            Ok(InterfaceFactSlot {
                offset: signed_integer(value, "offset", &context)?
                    .try_into()
                    .map_err(|_| format!("offset does not fit i32 in {context}")).map_err(crate::Error::invalid)?,
                width: integer(value, "width", &context)?
                    .try_into()
                    .map_err(|_| format!("width does not fit u8 in {context}")).map_err(crate::Error::invalid)?,
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
                .map_err(|_| format!("invalid argument index in {context}"))
                .map_err(crate::Error::invalid)?,
        },
        "absolute-address" => InterfaceFactRoot::AbsoluteAddress {
            address: address(value, "address", context)?,
        },
        kind => {
            return Err(crate::Error::invalid(format!(
                "unsupported interface root kind {kind:?} in {context}"
            )));
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
                    .map_err(|_| format!("offset does not fit i32 in {context}"))
                    .map_err(crate::Error::invalid)?,
                width: integer(value, "width", &context)?
                    .try_into()
                    .map_err(|_| format!("width does not fit u8 in {context}"))
                    .map_err(crate::Error::invalid)?,
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

fn optional_string(
    object: &Map<String, Value>,
    key: &str,
    context: &str,
) -> Result<Option<String>> {
    object
        .get(key)
        .filter(|value| !value.is_null())
        .map(|value| {
            value.as_str().map(str::to_owned).ok_or_else(|| {
                crate::Error::invalid(format!("{context}.{key} must be a string or null"))
            })
        })
        .transpose()
}

fn integer(object: &Map<String, Value>, key: &str, context: &str) -> Result<u64> {
    object.get(key).and_then(Value::as_u64).ok_or_else(|| {
        crate::Error::invalid(format!("{context} requires non-negative integer {key:?}"))
    })
}

fn signed_integer(object: &Map<String, Value>, key: &str, context: &str) -> Result<i64> {
    object
        .get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| crate::Error::invalid(format!("{context} requires integer {key:?}")))
}

fn optional_signed_integer(
    object: &Map<String, Value>,
    key: &str,
    context: &str,
) -> Result<Option<i64>> {
    object
        .get(key)
        .filter(|value| !value.is_null())
        .map(|value| {
            value.as_i64().ok_or_else(|| {
                crate::Error::invalid(format!("{context}.{key} must be an integer or null"))
            })
        })
        .transpose()
}

fn address(object: &Map<String, Value>, key: &str, context: &str) -> Result<u32> {
    let value = string(object, key, context)?;
    parse_u32(value)
        .ok_or_else(|| crate::Error::invalid(format!("invalid address {value:?} in {context}")))
}
