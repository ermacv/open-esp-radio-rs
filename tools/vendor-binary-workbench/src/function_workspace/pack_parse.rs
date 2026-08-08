//! Mechanical TOML decoding for reviewed function/context packs.

use toml_edit::{ArrayOfTables, DocumentMut, Item, Table};

use super::{
    FunctionPack, FunctionReviewStatus, ReviewedContext, ReviewedContextField, ReviewedFunction,
    ReviewedFunctionInput, ReviewedLogicalType, ReviewedMemoryObject, ReviewedTypeBinding,
    ReviewedTypeField,
};
use crate::Result;

pub(super) fn parse(document: &DocumentMut) -> Result<FunctionPack> {
    Ok(FunctionPack {
        id: required_string(document.as_item(), "id", "function pack")?,
        inputs: document
            .get("inputs")
            .and_then(Item::as_array_of_tables)
            .map(parse_inputs)
            .transpose()?
            .unwrap_or_default(),
        functions: document
            .get("functions")
            .and_then(Item::as_array_of_tables)
            .map(parse_functions)
            .transpose()?
            .unwrap_or_default(),
        types: document
            .get("types")
            .and_then(Item::as_array_of_tables)
            .map(parse_types)
            .transpose()?
            .unwrap_or_default(),
    })
}

fn parse_types(tables: &ArrayOfTables) -> Result<Vec<ReviewedLogicalType>> {
    tables
        .iter()
        .enumerate()
        .map(|(index, table)| {
            let context = format!("types[{index}]");
            Ok(ReviewedLogicalType {
                id: required_table_string(table, "id", &context)?,
                name: required_table_string(table, "name", &context)?,
                description: optional_string(table, "description"),
                bindings: table
                    .get("bindings")
                    .and_then(Item::as_array_of_tables)
                    .map(|bindings| parse_type_bindings(bindings, &context))
                    .transpose()?
                    .unwrap_or_default(),
                fields: table
                    .get("fields")
                    .and_then(Item::as_array_of_tables)
                    .map(|fields| parse_type_fields(fields, &context))
                    .transpose()?
                    .unwrap_or_default(),
            })
        })
        .collect()
}

fn parse_type_bindings(
    tables: &ArrayOfTables,
    logical_type: &str,
) -> Result<Vec<ReviewedTypeBinding>> {
    tables
        .iter()
        .enumerate()
        .map(|(index, table)| {
            let context = format!("{logical_type}.bindings[{index}]");
            let object = match required_table_string(table, "kind", &context)?.as_str() {
                "argument" => ReviewedMemoryObject::Argument {
                    function: required_table_string(table, "function", &context)?,
                    index: required_integer(table, "argument", &context)?
                        .try_into()
                        .map_err(|_| format!("{context}.argument must fit u8"))
                        .map_err(crate::Error::invalid)?,
                },
                "global" => ReviewedMemoryObject::Global {
                    member: optional_string(table, "member"),
                    symbol: required_table_string(table, "symbol", &context)?,
                },
                "dereferenced-global" => ReviewedMemoryObject::DereferencedGlobal {
                    member: optional_string(table, "member"),
                    symbol: required_table_string(table, "symbol", &context)?,
                    pointer_offset: required_integer(table, "pointer-offset", &context)?,
                },
                "absolute" => ReviewedMemoryObject::Absolute {
                    address_space: required_table_string(table, "address-space", &context)?,
                    address: required_integer(table, "address", &context)?
                        .try_into()
                        .map_err(|_| format!("{context}.address must fit u32"))
                        .map_err(crate::Error::invalid)?,
                },
                kind => {
                    return Err(crate::Error::invalid(format!(
                        "invalid memory object kind {kind:?} in {context}"
                    )));
                }
            };
            Ok(ReviewedTypeBinding {
                profile: required_table_string(table, "profile", &context)?,
                source: required_table_string(table, "source", &context)?,
                name: required_table_string(table, "name", &context)?,
                object,
            })
        })
        .collect()
}

fn parse_type_fields(tables: &ArrayOfTables, logical_type: &str) -> Result<Vec<ReviewedTypeField>> {
    tables
        .iter()
        .enumerate()
        .map(|(index, table)| {
            let context = format!("{logical_type}.fields[{index}]");
            Ok(ReviewedTypeField {
                offset: required_integer(table, "offset", &context)?,
                width: required_integer(table, "width", &context)?
                    .try_into()
                    .map_err(|_| format!("{context}.width must fit u8"))
                    .map_err(crate::Error::invalid)?,
                status: parse_status(table, &context)?,
                name: optional_string(table, "name"),
                display_type: optional_string(table, "display-type"),
                description: optional_string(table, "description"),
            })
        })
        .collect()
}

fn parse_inputs(tables: &ArrayOfTables) -> Result<Vec<ReviewedFunctionInput>> {
    tables
        .iter()
        .enumerate()
        .map(|(index, table)| {
            let context = format!("inputs[{index}]");
            Ok(ReviewedFunctionInput {
                profile: required_table_string(table, "profile", &context)?,
                source: required_table_string(table, "source", &context)?,
                sha256: required_table_string(table, "artifact-sha256", &context)?,
            })
        })
        .collect()
}

fn parse_functions(tables: &ArrayOfTables) -> Result<Vec<ReviewedFunction>> {
    tables
        .iter()
        .enumerate()
        .map(|(index, table)| {
            let context = format!("functions[{index}]");
            Ok(ReviewedFunction {
                profile: required_table_string(table, "profile", &context)?,
                source: required_table_string(table, "source", &context)?,
                identity: required_table_string(table, "identity", &context)?,
                status: parse_status(table, &context)?,
                name: optional_string(table, "name"),
                role: optional_string(table, "role"),
                summary: optional_string(table, "summary"),
                accept_incomplete: optional_bool(table, "accept-incomplete", &context)?
                    .unwrap_or(false),
                contexts: table
                    .get("contexts")
                    .and_then(Item::as_array_of_tables)
                    .map(|contexts| parse_contexts(contexts, &context))
                    .transpose()?
                    .unwrap_or_default(),
            })
        })
        .collect()
}

fn parse_contexts(tables: &ArrayOfTables, function: &str) -> Result<Vec<ReviewedContext>> {
    tables
        .iter()
        .enumerate()
        .map(|(index, table)| {
            let context = format!("{function}.contexts[{index}]");
            Ok(ReviewedContext {
                argument: required_integer(table, "argument", &context)?
                    .try_into()
                    .map_err(|_| format!("{context}.argument must fit u8"))
                    .map_err(crate::Error::invalid)?,
                status: parse_status(table, &context)?,
                name: optional_string(table, "name"),
                type_name: optional_string(table, "type-name"),
                fields: table
                    .get("fields")
                    .and_then(Item::as_array_of_tables)
                    .map(|fields| parse_fields(fields, &context))
                    .transpose()?
                    .unwrap_or_default(),
            })
        })
        .collect()
}

fn parse_fields(tables: &ArrayOfTables, context: &str) -> Result<Vec<ReviewedContextField>> {
    tables
        .iter()
        .enumerate()
        .map(|(index, table)| {
            let context = format!("{context}.fields[{index}]");
            Ok(ReviewedContextField {
                offset: required_integer(table, "offset", &context)?
                    .try_into()
                    .map_err(|_| format!("{context}.offset must fit i32"))
                    .map_err(crate::Error::invalid)?,
                width: required_integer(table, "width", &context)?
                    .try_into()
                    .map_err(|_| format!("{context}.width must fit u8"))
                    .map_err(crate::Error::invalid)?,
                status: parse_status(table, &context)?,
                name: optional_string(table, "name"),
                display_type: optional_string(table, "display-type"),
                description: optional_string(table, "description"),
            })
        })
        .collect()
}

fn parse_status(table: &Table, context: &str) -> Result<FunctionReviewStatus> {
    Ok(match optional_string(table, "status").as_deref() {
        None | Some("unreviewed") => FunctionReviewStatus::Unreviewed,
        Some("reviewed") => FunctionReviewStatus::Reviewed,
        Some("ignored") => FunctionReviewStatus::Ignored,
        Some(status) => {
            return Err(crate::Error::invalid(format!(
                "invalid review status {status:?} in {context}"
            )));
        }
    })
}

fn required_string(item: &Item, key: &str, context: &str) -> Result<String> {
    item.get(key)
        .and_then(Item::as_str)
        .map(str::to_owned)
        .ok_or_else(|| crate::Error::invalid(format!("{context} requires string {key:?}")))
}

fn required_table_string(table: &Table, key: &str, context: &str) -> Result<String> {
    optional_string(table, key)
        .ok_or_else(|| crate::Error::invalid(format!("{context} requires string {key:?}")))
}

fn optional_string(table: &Table, key: &str) -> Option<String> {
    table.get(key).and_then(Item::as_str).map(str::to_owned)
}

fn required_integer(table: &Table, key: &str, context: &str) -> Result<i64> {
    table
        .get(key)
        .and_then(Item::as_integer)
        .ok_or_else(|| crate::Error::invalid(format!("{context} requires integer {key:?}")))
}

fn optional_bool(table: &Table, key: &str, context: &str) -> Result<Option<bool>> {
    table
        .get(key)
        .map(|item| {
            item.as_bool()
                .ok_or_else(|| crate::Error::invalid(format!("{context}.{key} must be a boolean")))
        })
        .transpose()
}
