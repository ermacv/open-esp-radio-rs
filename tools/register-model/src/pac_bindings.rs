//! Stable address-to-svd2rust binding index derived from a release SVD.

use std::collections::{BTreeMap, BTreeSet};

use svd_rs::{Access, MaybeArray, RegisterCluster, RegisterProperties};

use crate::{Error, Result};

#[derive(Clone, Debug, Eq, PartialEq)]
struct ExpandedRegister {
    identity: String,
    peripheral: String,
    scope: Vec<ExpandedScope>,
    rust_name: String,
    array_index: Option<u32>,
    size_bits: u32,
    access: Option<Access>,
    alternate_register: Option<String>,
    fields: Vec<ExpandedField>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ExpandedScope {
    identity_name: String,
    rust_name: String,
    array_index: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ExpandedField {
    name: String,
    rust_name: String,
    array_index: Option<u32>,
    bit_offset: u32,
    bit_width: u32,
    access: Option<Access>,
}

/// Generate the stable text index used to bind observed MMIO addresses to PAC paths.
///
/// `crate_name` is the Rust crate identifier, not the Cargo package name.
pub fn generate_pac_binding_index(svd: &str, crate_name: &str) -> Result<String> {
    validate_pac_crate_name(crate_name)?;
    let addresses = expanded_register_map(svd)?;
    let mut output = format!("pac-binding-index 2\ncrate {crate_name}\n");
    for (address, registers) in addresses {
        for register in registers {
            let peripheral_type = type_binding_name(&register.peripheral);
            let peripheral_module = member_binding_name(&register.peripheral);
            let scope = if register.scope.is_empty() {
                "-".to_owned()
            } else {
                register
                    .scope
                    .iter()
                    .map(|scope| match scope.array_index {
                        Some(index) => format!("{}[{index}]", scope.rust_name),
                        None => scope.rust_name.clone(),
                    })
                    .collect::<Vec<_>>()
                    .join(".")
            };
            let register_index = register
                .array_index
                .map_or_else(|| "-".to_owned(), |index| index.to_string());
            let alternate = register.alternate_register.as_deref().unwrap_or("-");
            output.push_str(&format!(
                "register 0x{address:08x} {} {} {} {} {} {} {} {} {} {}\n",
                register.size_bits,
                access_label(register.access),
                register.identity,
                register.peripheral,
                peripheral_type,
                peripheral_module,
                scope,
                register.rust_name,
                register_index,
                alternate,
            ));
            for field in register.fields {
                let field_index = field
                    .array_index
                    .map_or_else(|| "-".to_owned(), |index| index.to_string());
                output.push_str(&format!(
                    "field 0x{address:08x} {} {} {} {} {} {} {}\n",
                    register.identity,
                    field.name,
                    field.rust_name,
                    field_index,
                    field.bit_offset,
                    field.bit_width,
                    access_label(field.access),
                ));
            }
        }
    }
    Ok(output)
}

/// Validate the Rust crate identifier stored in a binding index header.
pub fn validate_pac_crate_name(value: &str) -> Result<()> {
    let mut characters = value.chars();
    if !characters
        .next()
        .is_some_and(|character| character == '_' || character.is_ascii_alphabetic())
        || characters.any(|character| character != '_' && !character.is_ascii_alphanumeric())
    {
        return Err(Error::message(format!(
            "PAC binding crate name must be a Rust identifier, got {value:?}"
        )));
    }
    if matches!(
        value,
        "Self"
            | "as"
            | "break"
            | "const"
            | "continue"
            | "crate"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "fn"
            | "for"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "pub"
            | "ref"
            | "return"
            | "self"
            | "static"
            | "struct"
            | "super"
            | "trait"
            | "true"
            | "type"
            | "unsafe"
            | "use"
            | "where"
            | "while"
    ) {
        return Err(Error::message(format!(
            "PAC binding crate name is a Rust keyword: {value:?}"
        )));
    }
    Ok(())
}

fn expanded_register_map(svd: &str) -> Result<BTreeMap<u64, Vec<ExpandedRegister>>> {
    let device = svd_parser::parse(svd).map_err(|error| Error::message(error.to_string()))?;
    let mut addresses = BTreeMap::new();
    for peripheral in &device.peripherals {
        let instances = match peripheral {
            MaybeArray::Single(info) => vec![info.clone()],
            MaybeArray::Array(info, dim) => svd_rs::peripheral::expand(info, dim).collect(),
        };
        for peripheral in instances {
            let properties = merge_properties(
                device.default_register_properties,
                peripheral.default_register_properties,
            );
            if let Some(registers) = &peripheral.registers {
                expand_registers(
                    &peripheral.name,
                    peripheral.base_address,
                    0,
                    &[],
                    registers,
                    properties,
                    &mut addresses,
                )?;
            }
        }
    }
    Ok(addresses)
}

#[allow(clippy::too_many_arguments)]
fn expand_registers(
    peripheral_name: &str,
    peripheral_base: u64,
    parent_offset: u64,
    scope: &[ExpandedScope],
    children: &[RegisterCluster],
    inherited: RegisterProperties,
    addresses: &mut BTreeMap<u64, Vec<ExpandedRegister>>,
) -> Result<()> {
    for child in children {
        match child {
            RegisterCluster::Register(register) => {
                let instances = match register {
                    MaybeArray::Single(info) => {
                        vec![(info.clone(), member_binding_name(&info.name), None)]
                    }
                    MaybeArray::Array(info, dim) => svd_rs::register::expand(info, dim)
                        .enumerate()
                        .map(|(index, expanded)| {
                            (
                                expanded,
                                array_binding_name(&info.name, dim.dim_name.as_deref()),
                                Some(index as u32),
                            )
                        })
                        .collect(),
                };
                for (register, rust_name, array_index) in instances {
                    let properties = merge_properties(inherited, register.properties);
                    let size_bits = properties.size.ok_or_else(|| {
                        Error::message(format!("register {} has no inherited size", register.name))
                    })?;
                    let fields = expand_fields(&register, properties)?;
                    let address = peripheral_base
                        .checked_add(parent_offset)
                        .and_then(|base| base.checked_add(u64::from(register.address_offset)))
                        .ok_or_else(|| Error::message("SVD register address overflow"))?;
                    let mut identity = peripheral_name.to_owned();
                    for item in scope {
                        identity.push('.');
                        identity.push_str(&item.identity_name);
                    }
                    identity.push('.');
                    identity.push_str(&register.name);
                    addresses
                        .entry(address)
                        .or_default()
                        .push(ExpandedRegister {
                            identity,
                            peripheral: peripheral_name.to_owned(),
                            scope: scope.to_vec(),
                            rust_name,
                            array_index,
                            size_bits,
                            access: properties.access,
                            alternate_register: register.alternate_register,
                            fields,
                        });
                }
            }
            RegisterCluster::Cluster(cluster) => {
                let instances = match cluster {
                    MaybeArray::Single(info) => {
                        vec![(info.clone(), member_binding_name(&info.name), None)]
                    }
                    MaybeArray::Array(info, dim) => svd_rs::cluster::expand(info, dim)
                        .enumerate()
                        .map(|(index, expanded)| {
                            (
                                expanded,
                                array_binding_name(&info.name, dim.dim_name.as_deref()),
                                Some(index as u32),
                            )
                        })
                        .collect(),
                };
                for (cluster, rust_name, array_index) in instances {
                    let properties =
                        merge_properties(inherited, cluster.default_register_properties);
                    let mut child_scope = scope.to_vec();
                    child_scope.push(ExpandedScope {
                        identity_name: cluster.name.clone(),
                        rust_name,
                        array_index,
                    });
                    let offset = parent_offset
                        .checked_add(u64::from(cluster.address_offset))
                        .ok_or_else(|| Error::message("SVD cluster address overflow"))?;
                    expand_registers(
                        peripheral_name,
                        peripheral_base,
                        offset,
                        &child_scope,
                        &cluster.children,
                        properties,
                        addresses,
                    )?;
                }
            }
        }
    }
    Ok(())
}

fn expand_fields(
    register: &svd_rs::RegisterInfo,
    properties: RegisterProperties,
) -> Result<Vec<ExpandedField>> {
    let size_bits = properties
        .size
        .ok_or_else(|| Error::message("register has no inherited size while expanding fields"))?;
    let mut expanded = Vec::new();
    let mut names = BTreeSet::new();
    let mut occupied = 0_u128;
    for field in register.fields.iter().flatten() {
        let instances = match field {
            MaybeArray::Single(info) => {
                vec![(info.clone(), member_binding_name(&info.name), None)]
            }
            MaybeArray::Array(info, dim) => svd_rs::field::expand(info, dim)
                .enumerate()
                .map(|(index, expanded)| {
                    (
                        expanded,
                        array_binding_name(&info.name, dim.dim_name.as_deref()),
                        Some(index as u32),
                    )
                })
                .collect(),
        };
        for (field, rust_name, array_index) in instances {
            if !names.insert(field.name.clone()) {
                return Err(Error::message(format!(
                    "register {} contains duplicate expanded field name {}",
                    register.name, field.name
                )));
            }
            let bit_offset = field.bit_offset();
            let bit_width = field.bit_width();
            if bit_width == 0
                || bit_offset
                    .checked_add(bit_width)
                    .is_none_or(|end| end > size_bits)
            {
                return Err(Error::message(format!(
                    "register {} field {} has invalid expanded bit range {bit_offset}+{bit_width} for a {size_bits}-bit register",
                    register.name, field.name
                )));
            }
            let mask = if bit_width == 128 {
                u128::MAX
            } else {
                ((1_u128 << bit_width) - 1) << bit_offset
            };
            if occupied & mask != 0 {
                return Err(Error::message(format!(
                    "register {} field {} overlaps another expanded field",
                    register.name, field.name
                )));
            }
            occupied |= mask;
            expanded.push(ExpandedField {
                name: field.name,
                rust_name,
                array_index,
                bit_offset,
                bit_width,
                access: field.access.or(properties.access),
            });
        }
    }
    expanded
        .sort_by(|left, right| (left.bit_offset, &left.name).cmp(&(right.bit_offset, &right.name)));
    Ok(expanded)
}

const fn merge_properties(
    parent: RegisterProperties,
    child: RegisterProperties,
) -> RegisterProperties {
    let mut properties = parent;
    if child.size.is_some() {
        properties.size = child.size;
    }
    if child.access.is_some() {
        properties.access = child.access;
    }
    if child.protection.is_some() {
        properties.protection = child.protection;
    }
    if child.reset_value.is_some() {
        properties.reset_value = child.reset_value;
    }
    if child.reset_mask.is_some() {
        properties.reset_mask = child.reset_mask;
    }
    properties
}

fn remove_dimension_placeholder(value: &str) -> String {
    value.replace("[%s]", "").replace("%s", "")
}

fn member_binding_name(value: &str) -> String {
    remove_dimension_placeholder(value).to_ascii_lowercase()
}

fn array_binding_name(value: &str, dim_name: Option<&str>) -> String {
    member_binding_name(dim_name.unwrap_or(value))
}

fn type_binding_name(value: &str) -> String {
    let value = remove_dimension_placeholder(value);
    let mut output = String::new();
    let mut capitalize = true;
    for character in value.chars() {
        if character == '_' || character == '-' {
            capitalize = true;
        } else if capitalize {
            output.push(character.to_ascii_uppercase());
            capitalize = false;
        } else {
            output.push(character.to_ascii_lowercase());
        }
    }
    output
}

fn access_label(access: Option<Access>) -> &'static str {
    access.map(Access::as_str).unwrap_or("unspecified")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIMPLE_SVD: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<device schemaVersion="1.3" xmlns:xs="http://www.w3.org/2001/XMLSchema-instance">
  <name>FIXTURE</name>
  <version>1</version>
  <description>fixture</description>
  <addressUnitBits>8</addressUnitBits>
  <width>32</width>
  <peripherals>
    <peripheral>
      <name>UART0</name>
      <description>fixture peripheral</description>
      <baseAddress>0x40000000</baseAddress>
      <registers>
        <register>
          <name>CTRL</name>
          <description>control</description>
          <addressOffset>0</addressOffset>
          <size>32</size>
          <access>read-write</access>
          <fields>
            <field>
              <name>ENABLE</name>
              <description>enable</description>
              <bitOffset>0</bitOffset>
              <bitWidth>1</bitWidth>
            </field>
          </fields>
        </register>
      </registers>
    </peripheral>
  </peripherals>
</device>
"#;

    #[test]
    fn emits_configurable_crate_and_address_bindings() {
        assert_eq!(
            generate_pac_binding_index(SIMPLE_SVD, "fixture_pac").unwrap(),
            "pac-binding-index 2\ncrate fixture_pac\n\
             register 0x40000000 32 read-write UART0.CTRL UART0 Uart0 uart0 - ctrl - -\n\
             field 0x40000000 UART0.CTRL ENABLE enable - 0 1 read-write\n"
        );
    }

    #[test]
    fn rejects_package_names_in_place_of_rust_crate_identifiers() {
        assert!(generate_pac_binding_index("", "vendor-pac").is_err());
        assert!(validate_pac_crate_name("crate").is_err());
    }
}
