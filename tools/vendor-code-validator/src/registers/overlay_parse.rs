//! Mechanical TOML decoding for the reviewed register overlay.

use toml_edit::{ArrayOfTables, DocumentMut, Item, Table};

use super::{
    DeviceOverlay, FieldOrigin, FieldOverlay, PeripheralOverlay, RegisterOverlay,
    RegisterOverlayFile, RegisterStatus, overlay,
};
use crate::Result;

pub(super) fn parse(document: &DocumentMut) -> Result<RegisterOverlayFile> {
    Ok(RegisterOverlayFile {
        device: DeviceOverlay {
            name: required_string(document.as_item(), "device-name", "register overlay")?,
            vendor: optional_string(document.as_item(), "vendor"),
            version: optional_string(document.as_item(), "version")
                .unwrap_or_else(|| "0.1".to_owned()),
            description: optional_string(document.as_item(), "description")
                .unwrap_or_else(|| "Reviewed MMIO register workspace".to_owned()),
            address_unit_bits: optional_integer(document.as_item(), "address-unit-bits")
                .unwrap_or(8)
                .try_into()
                .map_err(|_| "invalid address-unit-bits in register overlay")?,
            width: optional_integer(document.as_item(), "width")
                .unwrap_or(32)
                .try_into()
                .map_err(|_| "invalid device width in register overlay")?,
        },
        peripherals: optional_tables(document, "peripherals")
            .map(parse_peripherals)
            .transpose()?
            .unwrap_or_default(),
        registers: optional_tables(document, "registers")
            .map(parse_registers)
            .transpose()?
            .unwrap_or_default(),
    })
}

fn parse_peripherals(tables: &ArrayOfTables) -> Result<Vec<PeripheralOverlay>> {
    tables
        .iter()
        .enumerate()
        .map(|(index, table)| {
            let context = format!("peripherals[{index}]");
            Ok(PeripheralOverlay {
                range: required_table_string(table, "range", &context)?,
                name: required_table_string(table, "name", &context)?,
                description: optional_table_string(table, "description"),
            })
        })
        .collect()
}

fn parse_registers(tables: &ArrayOfTables) -> Result<Vec<RegisterOverlay>> {
    tables
        .iter()
        .enumerate()
        .map(|(index, table)| {
            let context = format!("registers[{index}]");
            let status = match optional_table_string(table, "status").as_deref() {
                None | Some("reviewed") => RegisterStatus::Reviewed,
                Some("ignored") => RegisterStatus::Ignored,
                Some("manual") => RegisterStatus::Manual,
                Some(value) => {
                    return Err(format!("invalid register status {value:?} in {context}").into());
                }
            };
            Ok(RegisterOverlay {
                address: required_table_u32(table, "address", &context)?,
                width: required_table_u8(table, "width", &context)?,
                status,
                name: optional_table_string(table, "name"),
                description: optional_table_string(table, "description"),
                access: optional_table_string(table, "access")
                    .map(|value| overlay::validate_access(value, &context))
                    .transpose()?,
                reset_value: optional_table_u32(table, "reset-value", &context)?,
                reset_mask: optional_table_u32(table, "reset-mask", &context)?,
                fields: if let Some(item) = table.get("fields") {
                    let tables = item
                        .as_array_of_tables()
                        .ok_or_else(|| format!("{context}.fields must be an array of tables"))?;
                    parse_fields(tables, &context)?
                } else {
                    Vec::new()
                },
            })
        })
        .collect()
}

fn parse_fields(tables: &ArrayOfTables, register: &str) -> Result<Vec<FieldOverlay>> {
    tables
        .iter()
        .enumerate()
        .map(|(index, table)| {
            let context = format!("{register}.fields[{index}]");
            Ok(FieldOverlay {
                name: required_table_string(table, "name", &context)?,
                lsb: required_table_u8(table, "lsb", &context)?,
                width: required_table_u8(table, "width", &context)?,
                description: optional_table_string(table, "description"),
                access: optional_table_string(table, "access")
                    .map(|value| overlay::validate_access(value, &context))
                    .transpose()?,
                modified_write_values: optional_table_string(table, "modified-write-values")
                    .map(|value| overlay::validate_modified_write_values(value, &context))
                    .transpose()?,
                read_action: optional_table_string(table, "read-action")
                    .map(|value| overlay::validate_read_action(value, &context))
                    .transpose()?,
                origin: match optional_table_string(table, "origin").as_deref() {
                    None | Some("manual") => FieldOrigin::Manual,
                    Some("write-pattern") => FieldOrigin::WritePattern,
                    Some(value) => {
                        return Err(format!("invalid field origin {value:?} in {context}").into());
                    }
                },
            })
        })
        .collect()
}

fn optional_tables<'a>(document: &'a DocumentMut, key: &str) -> Option<&'a ArrayOfTables> {
    document.get(key).and_then(Item::as_array_of_tables)
}

fn required_string(item: &Item, key: &str, context: &str) -> Result<String> {
    optional_string(item, key).ok_or_else(|| format!("{context} requires string {key:?}").into())
}

fn optional_string(item: &Item, key: &str) -> Option<String> {
    item.get(key).and_then(Item::as_str).map(str::to_owned)
}

fn optional_integer(item: &Item, key: &str) -> Option<i64> {
    item.get(key).and_then(Item::as_integer)
}

fn required_table_string(table: &Table, key: &str, context: &str) -> Result<String> {
    optional_table_string(table, key)
        .ok_or_else(|| format!("{context} requires string {key:?}").into())
}

fn optional_table_string(table: &Table, key: &str) -> Option<String> {
    table.get(key).and_then(Item::as_str).map(str::to_owned)
}

fn required_table_u32(table: &Table, key: &str, context: &str) -> Result<u32> {
    optional_table_u32(table, key, context)?
        .ok_or_else(|| format!("{context} requires non-negative integer {key:?}").into())
}

fn optional_table_u32(table: &Table, key: &str, context: &str) -> Result<Option<u32>> {
    table
        .get(key)
        .map(|item| {
            item.as_integer()
                .ok_or_else(|| format!("{context}.{key} must be an integer"))?
                .try_into()
                .map_err(|_| format!("{context}.{key} must fit u32").into())
        })
        .transpose()
}

fn required_table_u8(table: &Table, key: &str, context: &str) -> Result<u8> {
    required_table_u32(table, key, context)?
        .try_into()
        .map_err(|_| format!("{context}.{key} must fit u8").into())
}
