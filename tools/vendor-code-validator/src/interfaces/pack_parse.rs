//! Mechanical TOML decoding for reviewed interface packs.

use toml_edit::{Array, ArrayOfTables, DocumentMut, InlineTable, Item, Table, Value};

use super::{
    InterfaceAnchor, InterfaceFactStep, InterfaceGuard, InterfacePack, InterfaceRootSelector,
    InterfaceSlot, PackOrigin, ReviewStatus,
};
use crate::Result;

pub(super) fn parse(document: &DocumentMut) -> Result<InterfacePack> {
    Ok(InterfacePack {
        id: required_string(document.as_item(), "id", "interface pack")?,
        calling_convention: required_string(
            document.as_item(),
            "calling-convention",
            "interface pack",
        )?,
        anchors: document
            .get("anchors")
            .and_then(Item::as_array_of_tables)
            .map(parse_anchors)
            .transpose()?
            .unwrap_or_default(),
    })
}

fn parse_anchors(tables: &ArrayOfTables) -> Result<Vec<InterfaceAnchor>> {
    tables
        .iter()
        .enumerate()
        .map(|(index, table)| {
            let context = format!("anchors[{index}]");
            Ok(InterfaceAnchor {
                id: required_table_string(table, "id", &context)?,
                status: parse_status(table, &context)?,
                origin: parse_origin(table, &context)?,
                source: required_table_string(table, "source", &context)?,
                root: parse_root(table, &context)?,
                container_path: table
                    .get("container-path")
                    .and_then(Item::as_array)
                    .map(|array| parse_steps(array, &format!("{context}.container-path")))
                    .transpose()?
                    .unwrap_or_default(),
                layout_version: optional_table_string(table, "layout-version"),
                pointer_width: optional_u8(table, "pointer-width", &context)?,
                layout_size: optional_u32(table, "layout-size", &context)?,
                slot_stride: optional_u8(table, "slot-stride", &context)?,
                guards: table
                    .get("guards")
                    .and_then(Item::as_array_of_tables)
                    .map(|tables| parse_guards(tables, &context))
                    .transpose()?
                    .unwrap_or_default(),
                slots: table
                    .get("slots")
                    .and_then(Item::as_array_of_tables)
                    .map(|tables| parse_slots(tables, &context))
                    .transpose()?
                    .unwrap_or_default(),
            })
        })
        .collect()
}

fn parse_root(table: &Table, context: &str) -> Result<InterfaceRootSelector> {
    Ok(
        match required_table_string(table, "root-kind", context)?.as_str() {
            "relocated-symbol" => InterfaceRootSelector::RelocatedSymbol {
                member: optional_table_string(table, "member"),
                symbol: required_table_string(table, "symbol", context)?,
                addend: optional_i64(table, "addend", context)?.unwrap_or(0),
                addressing: required_table_string(table, "addressing", context)?,
            },
            "function-argument" => InterfaceRootSelector::FunctionArgument {
                argument: required_u8(table, "argument", context)?,
            },
            "absolute-address" => InterfaceRootSelector::AbsoluteAddress {
                address: required_u32(table, "address", context)?,
            },
            kind => return Err(format!("invalid root-kind {kind:?} in {context}").into()),
        },
    )
}

fn parse_guards(tables: &ArrayOfTables, anchor: &str) -> Result<Vec<InterfaceGuard>> {
    tables
        .iter()
        .enumerate()
        .map(|(index, table)| {
            let context = format!("{anchor}.guards[{index}]");
            Ok(
                match required_table_string(table, "kind", &context)?.as_str() {
                    "artifact-sha256" => InterfaceGuard::ArtifactSha256 {
                        sha256: required_table_string(table, "sha256", &context)?,
                    },
                    "runtime-value" => InterfaceGuard::RuntimeValue {
                        purpose: required_table_string(table, "purpose", &context)?,
                        offset: required_i32(table, "offset", &context)?,
                        width: required_u8(table, "width", &context)?,
                        mask: required_u64(table, "mask", &context)?,
                        value: required_u64(table, "value", &context)?,
                    },
                    kind => return Err(format!("invalid guard kind {kind:?} in {context}").into()),
                },
            )
        })
        .collect()
}

fn parse_slots(tables: &ArrayOfTables, anchor: &str) -> Result<Vec<InterfaceSlot>> {
    tables
        .iter()
        .enumerate()
        .map(|(index, table)| {
            let context = format!("{anchor}.slots[{index}]");
            Ok(InterfaceSlot {
                offset: required_i32(table, "offset", &context)?,
                width: required_u8(table, "width", &context)?,
                status: parse_status(table, &context)?,
                origin: parse_origin(table, &context)?,
                name: optional_table_string(table, "name"),
                arguments: table
                    .get("arguments")
                    .and_then(Item::as_array)
                    .map(|array| parse_string_array(array, "arguments", &context))
                    .transpose()?,
                return_type: optional_table_string(table, "return"),
                variadic: table
                    .get("variadic")
                    .map(|item| -> Result<bool> {
                        item.as_bool()
                            .ok_or_else(|| format!("{context}.variadic must be a boolean").into())
                    })
                    .transpose()?
                    .unwrap_or(false),
                semantic: optional_table_string(table, "semantic"),
            })
        })
        .collect()
}

fn parse_steps(array: &Array, context: &str) -> Result<Vec<InterfaceFactStep>> {
    array
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let context = format!("{context}[{index}]");
            let table = value
                .as_inline_table()
                .ok_or_else(|| format!("{context} must be an inline table"))?;
            Ok(InterfaceFactStep {
                offset: inline_i64(table, "offset", &context)?
                    .try_into()
                    .map_err(|_| format!("{context}.offset must fit i32"))?,
                width: inline_i64(table, "width", &context)?
                    .try_into()
                    .map_err(|_| format!("{context}.width must fit u8"))?,
            })
        })
        .collect()
}

fn parse_status(table: &Table, context: &str) -> Result<ReviewStatus> {
    Ok(match optional_table_string(table, "status").as_deref() {
        None | Some("unreviewed") => ReviewStatus::Unreviewed,
        Some("reviewed") => ReviewStatus::Reviewed,
        Some("ignored") => ReviewStatus::Ignored,
        Some(value) => return Err(format!("invalid review status {value:?} in {context}").into()),
    })
}

fn parse_origin(table: &Table, context: &str) -> Result<PackOrigin> {
    Ok(match optional_table_string(table, "origin").as_deref() {
        None | Some("observed") => PackOrigin::Observed,
        Some("manual") => PackOrigin::Manual,
        Some(value) => return Err(format!("invalid origin {value:?} in {context}").into()),
    })
}

fn required_string(item: &Item, key: &str, context: &str) -> Result<String> {
    item.get(key)
        .and_then(Item::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("{context} requires string {key:?}").into())
}

fn required_table_string(table: &Table, key: &str, context: &str) -> Result<String> {
    optional_table_string(table, key)
        .ok_or_else(|| format!("{context} requires string {key:?}").into())
}

fn optional_table_string(table: &Table, key: &str) -> Option<String> {
    table.get(key).and_then(Item::as_str).map(str::to_owned)
}

fn parse_string_array(array: &Array, key: &str, context: &str) -> Result<Vec<String>> {
    array
        .iter()
        .enumerate()
        .map(|(index, value)| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("{context}.{key}[{index}] must be a string").into())
        })
        .collect()
}

fn required_u8(table: &Table, key: &str, context: &str) -> Result<u8> {
    required_i64(table, key, context)?
        .try_into()
        .map_err(|_| format!("{context}.{key} must fit u8").into())
}

fn required_u32(table: &Table, key: &str, context: &str) -> Result<u32> {
    required_i64(table, key, context)?
        .try_into()
        .map_err(|_| format!("{context}.{key} must fit u32").into())
}

fn required_i32(table: &Table, key: &str, context: &str) -> Result<i32> {
    required_i64(table, key, context)?
        .try_into()
        .map_err(|_| format!("{context}.{key} must fit i32").into())
}

fn required_i64(table: &Table, key: &str, context: &str) -> Result<i64> {
    optional_i64(table, key, context)?
        .ok_or_else(|| format!("{context} requires integer {key:?}").into())
}

fn optional_i64(table: &Table, key: &str, context: &str) -> Result<Option<i64>> {
    table
        .get(key)
        .map(|item| {
            item.as_integer()
                .ok_or_else(|| format!("{context}.{key} must be an integer").into())
        })
        .transpose()
}

fn optional_u8(table: &Table, key: &str, context: &str) -> Result<Option<u8>> {
    optional_i64(table, key, context)?
        .map(|value| {
            value
                .try_into()
                .map_err(|_| format!("{context}.{key} must fit u8").into())
        })
        .transpose()
}

fn optional_u32(table: &Table, key: &str, context: &str) -> Result<Option<u32>> {
    optional_i64(table, key, context)?
        .map(|value| {
            value
                .try_into()
                .map_err(|_| format!("{context}.{key} must fit u32").into())
        })
        .transpose()
}

fn required_u64(table: &Table, key: &str, context: &str) -> Result<u64> {
    let value = table
        .get(key)
        .and_then(Item::as_value)
        .ok_or_else(|| format!("{context} requires integer or string {key:?}"))?;
    match value {
        Value::Integer(value) => value
            .value()
            .to_owned()
            .try_into()
            .map_err(|_| format!("{context}.{key} must be non-negative").into()),
        Value::String(value) => parse_u64(value.value()).ok_or_else(|| {
            format!("invalid u64 literal {:?} in {context}.{key}", value.value()).into()
        }),
        _ => Err(format!("{context}.{key} must be an integer or string").into()),
    }
}

fn inline_i64(table: &InlineTable, key: &str, context: &str) -> Result<i64> {
    table
        .get(key)
        .and_then(Value::as_integer)
        .ok_or_else(|| format!("{context} requires integer {key:?}").into())
}

fn parse_u64(value: &str) -> Option<u64> {
    let value = value.replace('_', "");
    if let Some(value) = value.strip_prefix("0x") {
        u64::from_str_radix(value, 16).ok()
    } else {
        value.parse().ok()
    }
}
