//! Mechanical TOML decoding for reviewed interface packs.

use toml_edit::{Array, ArrayOfTables, DocumentMut, InlineTable, Item, Table, Value};

use super::{
    InterfaceAnchor, InterfaceFactStep, InterfaceGuard, InterfaceIndexDomain, InterfacePack,
    InterfaceRootSelector, InterfaceSlot, PackOrigin, ReviewStatus,
    templates::InterfaceTemplateCatalog,
};
use crate::Result;

pub(super) fn parse(
    document: &DocumentMut,
    schema_3: bool,
    templates: &InterfaceTemplateCatalog,
) -> Result<InterfacePack> {
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
            .map(|tables| parse_anchors(tables, schema_3, templates))
            .transpose()?
            .unwrap_or_default(),
    })
}

fn parse_anchors(
    tables: &ArrayOfTables,
    schema_3: bool,
    templates: &InterfaceTemplateCatalog,
) -> Result<Vec<InterfaceAnchor>> {
    tables
        .iter()
        .enumerate()
        .map(|(index, table)| {
            let context = format!("anchors[{index}]");
            let template_id = optional_table_string(table, "template");
            if template_id.is_some() && !schema_3 {
                return Err(crate::Error::invalid(format!(
                    "{context}.template requires interface pack schema = 3"
                )));
            }
            if template_id.is_none() && table.get("overrides").is_some() {
                return Err(crate::Error::invalid(format!(
                    "{context}.overrides requires a reusable template"
                )));
            }
            let origin = parse_origin(table, &context)?;
            let mut slots = if let Some(template_id) = &template_id {
                for key in [
                    "layout-version",
                    "pointer-width",
                    "layout-size",
                    "slot-stride",
                    "slots",
                ] {
                    if table.get(key).is_some() {
                        return Err(crate::Error::invalid(format!(
                            "{context}.{key} conflicts with reusable template {template_id:?}"
                        )));
                    }
                }
                let template = templates.get(template_id).ok_or_else(|| {
                    crate::Error::invalid(format!(
                        "{context}.template refers to unknown interface template {template_id:?}"
                    ))
                })?;
                template
                    .slots
                    .iter()
                    .map(|slot| slot.materialize())
                    .collect::<Vec<_>>()
            } else {
                table
                    .get("slots")
                    .and_then(Item::as_array_of_tables)
                    .map(|tables| parse_slots(tables, &context))
                    .transpose()?
                    .unwrap_or_default()
            };
            let template_overrides = table
                .get("overrides")
                .and_then(Item::as_array_of_tables)
                .map(|overrides| apply_overrides(&mut slots, overrides, &context))
                .transpose()?
                .unwrap_or_default();
            let template = template_id.as_deref().and_then(|id| templates.get(id));
            Ok(InterfaceAnchor {
                id: required_table_string(table, "id", &context)?,
                template: template_id,
                template_overrides,
                status: parse_status(table, &context)?,
                origin,
                source: required_table_string(table, "source", &context)?,
                root: parse_root(table, &context)?,
                container_path: table
                    .get("container-path")
                    .and_then(Item::as_array)
                    .map(|array| parse_steps(array, &format!("{context}.container-path")))
                    .transpose()?
                    .unwrap_or_default(),
                layout_version: template
                    .map(|template| template.layout_version.clone())
                    .or_else(|| optional_table_string(table, "layout-version")),
                pointer_width: template
                    .map(|template| template.pointer_width)
                    .or(optional_u8(table, "pointer-width", &context)?),
                layout_size: template
                    .map(|template| template.layout_size)
                    .or(optional_u32(table, "layout-size", &context)?),
                slot_stride: template
                    .map(|template| template.slot_stride)
                    .or(optional_u8(table, "slot-stride", &context)?),
                execution_contract: optional_table_string(table, "execution-contract"),
                index_domains: table
                    .get("index-domains")
                    .and_then(Item::as_array_of_tables)
                    .map(|domains| parse_index_domains(domains, &context))
                    .transpose()?
                    .unwrap_or_default(),
                guards: table
                    .get("guards")
                    .and_then(Item::as_array_of_tables)
                    .map(|tables| parse_guards(tables, &context))
                    .transpose()?
                    .unwrap_or_default(),
                slots,
            })
        })
        .collect()
}

fn apply_overrides(
    slots: &mut [InterfaceSlot],
    tables: &ArrayOfTables,
    anchor: &str,
) -> Result<Vec<super::InterfaceTemplateOverride>> {
    let mut offsets = std::collections::BTreeSet::new();
    let mut records = Vec::new();
    for (index, table) in tables.iter().enumerate() {
        let context = format!("{anchor}.overrides[{index}]");
        for (key, item) in table.iter() {
            if !matches!(
                key,
                "offset"
                    | "reason"
                    | "origin"
                    | "name"
                    | "arguments"
                    | "return"
                    | "variadic"
                    | "semantic"
                    | "execution-model"
            ) {
                return Err(crate::Error::invalid(format!(
                    "unknown {context} key {key:?} at {:?}",
                    item.span()
                )));
            }
        }
        let offset = required_i32(table, "offset", &context)?;
        if !offsets.insert(offset) {
            return Err(crate::Error::invalid(format!(
                "{anchor} has duplicate override for template offset {offset:+#x}"
            )));
        }
        let reason = required_table_string(table, "reason", &context)?;
        if reason.trim().is_empty() || reason.contains(['\r', '\n']) {
            return Err(crate::Error::invalid(format!(
                "{context}.reason must be one non-empty line"
            )));
        }
        let slot = slots
            .iter_mut()
            .find(|slot| slot.offset == offset)
            .ok_or_else(|| {
                crate::Error::invalid(format!(
                    "{context} targets unknown template slot offset {offset:+#x}"
                ))
            })?;
        let mut changed = false;
        let mut fields = Vec::new();
        if table.get("origin").is_some() {
            slot.origin = parse_origin(table, &context)?;
            changed = true;
            fields.push("origin".to_owned());
        }
        if let Some(name) = optional_table_string(table, "name") {
            slot.name = Some(name);
            changed = true;
            fields.push("name".to_owned());
        }
        if let Some(arguments) = table.get("arguments").and_then(Item::as_array) {
            slot.arguments = Some(parse_string_array(arguments, "arguments", &context)?);
            changed = true;
            fields.push("arguments".to_owned());
        }
        if let Some(return_type) = optional_table_string(table, "return") {
            slot.return_type = Some(return_type);
            changed = true;
            fields.push("return".to_owned());
        }
        if let Some(variadic) = table.get("variadic") {
            slot.variadic = variadic.as_bool().ok_or_else(|| {
                crate::Error::invalid(format!("{context}.variadic must be a boolean"))
            })?;
            changed = true;
            fields.push("variadic".to_owned());
        }
        if let Some(semantic) = optional_table_string(table, "semantic") {
            slot.semantic = Some(semantic);
            changed = true;
            fields.push("semantic".to_owned());
        }
        if let Some(model) = optional_table_string(table, "execution-model") {
            slot.execution_model = Some(model);
            changed = true;
            fields.push("execution-model".to_owned());
        }
        if !changed {
            return Err(crate::Error::invalid(format!(
                "{context} must override origin, ABI, semantic, or execution-model"
            )));
        }
        records.push(super::InterfaceTemplateOverride {
            offset,
            reason,
            fields,
        });
    }
    records.sort_by_key(|record| record.offset);
    Ok(records)
}

fn parse_index_domains(tables: &ArrayOfTables, anchor: &str) -> Result<Vec<InterfaceIndexDomain>> {
    tables
        .iter()
        .enumerate()
        .map(|(index, table)| {
            let context = format!("{anchor}.index-domains[{index}]");
            Ok(InterfaceIndexDomain {
                argument: required_u8(table, "argument", &context)?,
                min: required_u32(table, "min", &context)?,
                max: required_u32(table, "max", &context)?,
                evidence: required_table_string(table, "evidence", &context)?,
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
            kind => {
                return Err(crate::Error::invalid(format!(
                    "invalid root-kind {kind:?} in {context}"
                )));
            }
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
                    kind => {
                        return Err(crate::Error::invalid(format!(
                            "invalid guard kind {kind:?} in {context}"
                        )));
                    }
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
                        item.as_bool().ok_or_else(|| {
                            crate::Error::invalid(format!("{context}.variadic must be a boolean"))
                        })
                    })
                    .transpose()?
                    .unwrap_or(false),
                semantic: optional_table_string(table, "semantic"),
                execution_model: optional_table_string(table, "execution-model"),
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
                .ok_or_else(|| format!("{context} must be an inline table"))
                .map_err(crate::Error::invalid)?;
            Ok(InterfaceFactStep {
                offset: inline_i64(table, "offset", &context)?
                    .try_into()
                    .map_err(|_| format!("{context}.offset must fit i32"))
                    .map_err(crate::Error::invalid)?,
                width: inline_i64(table, "width", &context)?
                    .try_into()
                    .map_err(|_| format!("{context}.width must fit u8"))
                    .map_err(crate::Error::invalid)?,
                selector: None,
            })
        })
        .collect()
}

fn parse_status(table: &Table, context: &str) -> Result<ReviewStatus> {
    Ok(match optional_table_string(table, "status").as_deref() {
        Some("reviewed") => ReviewStatus::Reviewed,
        Some("ignored") => ReviewStatus::Ignored,
        None | Some("unreviewed") => {
            return Err(crate::Error::invalid(format!(
                "{context} is a sparse review overlay and requires status = \"reviewed\" or \"ignored\"; omit unreviewed generated observations"
            )));
        }
        Some(value) => {
            return Err(crate::Error::invalid(format!(
                "invalid review status {value:?} in {context}"
            )));
        }
    })
}

fn parse_origin(table: &Table, context: &str) -> Result<PackOrigin> {
    Ok(match optional_table_string(table, "origin").as_deref() {
        None | Some("observed") => PackOrigin::Observed,
        Some("reviewed") => PackOrigin::Reviewed,
        Some(value) => {
            return Err(crate::Error::invalid(format!(
                "invalid origin {value:?} in {context}"
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
    optional_table_string(table, key)
        .ok_or_else(|| crate::Error::invalid(format!("{context} requires string {key:?}")))
}

fn optional_table_string(table: &Table, key: &str) -> Option<String> {
    table.get(key).and_then(Item::as_str).map(str::to_owned)
}

fn parse_string_array(array: &Array, key: &str, context: &str) -> Result<Vec<String>> {
    array
        .iter()
        .enumerate()
        .map(|(index, value)| {
            value.as_str().map(str::to_owned).ok_or_else(|| {
                crate::Error::invalid(format!("{context}.{key}[{index}] must be a string"))
            })
        })
        .collect()
}

fn required_u8(table: &Table, key: &str, context: &str) -> Result<u8> {
    required_i64(table, key, context)?
        .try_into()
        .map_err(|_| crate::Error::invalid(format!("{context}.{key} must fit u8")))
}

fn required_u32(table: &Table, key: &str, context: &str) -> Result<u32> {
    required_i64(table, key, context)?
        .try_into()
        .map_err(|_| crate::Error::invalid(format!("{context}.{key} must fit u32")))
}

fn required_i32(table: &Table, key: &str, context: &str) -> Result<i32> {
    required_i64(table, key, context)?
        .try_into()
        .map_err(|_| crate::Error::invalid(format!("{context}.{key} must fit i32")))
}

fn required_i64(table: &Table, key: &str, context: &str) -> Result<i64> {
    optional_i64(table, key, context)?
        .ok_or_else(|| crate::Error::invalid(format!("{context} requires integer {key:?}")))
}

fn optional_i64(table: &Table, key: &str, context: &str) -> Result<Option<i64>> {
    table
        .get(key)
        .map(|item| {
            item.as_integer()
                .ok_or_else(|| crate::Error::invalid(format!("{context}.{key} must be an integer")))
        })
        .transpose()
}

fn optional_u8(table: &Table, key: &str, context: &str) -> Result<Option<u8>> {
    optional_i64(table, key, context)?
        .map(|value| {
            value
                .try_into()
                .map_err(|_| crate::Error::invalid(format!("{context}.{key} must fit u8")))
        })
        .transpose()
}

fn optional_u32(table: &Table, key: &str, context: &str) -> Result<Option<u32>> {
    optional_i64(table, key, context)?
        .map(|value| {
            value
                .try_into()
                .map_err(|_| crate::Error::invalid(format!("{context}.{key} must fit u32")))
        })
        .transpose()
}

fn required_u64(table: &Table, key: &str, context: &str) -> Result<u64> {
    let value = table
        .get(key)
        .and_then(Item::as_value)
        .ok_or_else(|| format!("{context} requires integer or string {key:?}"))
        .map_err(crate::Error::invalid)?;
    match value {
        Value::Integer(value) => value
            .value()
            .to_owned()
            .try_into()
            .map_err(|_| crate::Error::invalid(format!("{context}.{key} must be non-negative"))),
        Value::String(value) => parse_u64(value.value()).ok_or_else(|| {
            crate::Error::invalid(format!(
                "invalid u64 literal {:?} in {context}.{key}",
                value.value()
            ))
        }),
        _ => Err(crate::Error::invalid(format!(
            "{context}.{key} must be an integer or string"
        ))),
    }
}

fn inline_i64(table: &InlineTable, key: &str, context: &str) -> Result<i64> {
    table
        .get(key)
        .and_then(Value::as_integer)
        .ok_or_else(|| crate::Error::invalid(format!("{context} requires integer {key:?}")))
}

fn parse_u64(value: &str) -> Option<u64> {
    let value = value.replace('_', "");
    if let Some(value) = value.strip_prefix("0x") {
        u64::from_str_radix(value, 16).ok()
    } else {
        value.parse().ok()
    }
}
