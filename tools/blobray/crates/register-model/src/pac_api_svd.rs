//! Cross-validation of reviewed PAC transactions against their release SVD.

use std::collections::{BTreeMap, BTreeSet};

use svd_rs::{
    Access, Device, FieldInfo, MaybeArray, ModifiedWriteValues, RegisterCluster, RegisterInfo,
    RegisterProperties, Usage, WriteConstraint,
};

use crate::{Error, PacApiPack, Result};

#[derive(Clone, Copy)]
pub(super) struct RegisterBinding<'a> {
    pub(super) info: &'a RegisterInfo,
    pub(super) properties: RegisterProperties,
    pub(super) is_array: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct OwnedRegisterRange {
    partition: String,
    identity: String,
    start: u64,
    end_exclusive: u64,
}

#[derive(Clone, Copy)]
struct RegisterRangeContext<'a> {
    partition: &'a str,
    peripheral_base: u64,
}

impl PacApiPack {
    /// Prove that every reviewed transaction is compatible with the release SVD.
    pub fn validate_against_svd(&self, svd: &str) -> Result<()> {
        self.validate()?;
        let device = svd_parser::parse(svd).map_err(|error| Error::message(error.to_string()))?;
        self.validate_ownership_partitions_against_svd(&device)?;

        for operation in &self.interrupt_snapshots {
            let status = register(
                &device,
                &operation.name,
                &operation.peripheral,
                &operation.status_register,
            )?;
            require_size_access(
                &operation.name,
                status,
                Access::ReadOnly,
                "read-only status",
            )?;
            let clear = register(
                &device,
                &operation.name,
                &operation.peripheral,
                &operation.clear_register,
            )?;
            if clear.properties.size != Some(32)
                || clear.info.modified_write_values != Some(ModifiedWriteValues::OneToClear)
            {
                return Err(Error::message(format!(
                    "PAC API interrupt snapshot {:?} clear register must be 32-bit one-to-clear",
                    operation.name
                )));
            }
            let field = field(&operation.name, clear.info, &operation.clear_field)?;
            if field.bit_offset() != 0 || field.bit_width() != 32 {
                return Err(Error::message(format!(
                    "PAC API interrupt snapshot {:?} clear field must cover all 32 bits",
                    operation.name
                )));
            }
        }

        for operation in &self.full_register_writes {
            let binding = writable_register(
                &device,
                &operation.name,
                &operation.peripheral,
                &operation.register,
            )?;
            let fields = binding.info.fields.as_deref().ok_or_else(|| {
                Error::message(format!(
                    "PAC API operation {:?} register has no fields",
                    operation.name
                ))
            })?;
            if fields.len() != 1 {
                return Err(Error::message(format!(
                    "PAC API full-register-write {:?} requires exactly one field",
                    operation.name
                )));
            }
            let field = field(&operation.name, binding.info, &operation.field)?;
            require_full_field(&operation.name, field)?;
            require_full_range(&operation.name, field, 32)?;
        }

        for operation in &self.fixed_register_writes {
            let binding = writable_register(
                &device,
                &operation.name,
                &operation.peripheral,
                &operation.register,
            )?;
            let fields = binding.info.fields.as_deref().ok_or_else(|| {
                Error::message(format!(
                    "PAC API operation {:?} register has no fields",
                    operation.name
                ))
            })?;
            if fields.len() != 1 {
                return Err(Error::message(format!(
                    "PAC API fixed-register-write {:?} requires exactly one field",
                    operation.name
                )));
            }
            let field = field(&operation.name, binding.info, &operation.field)?;
            require_full_field(&operation.name, field)?;
            let writable_variant = field
                .enumerated_values
                .iter()
                .filter(|values| values.usage != Some(Usage::Read))
                .flat_map(|values| &values.values)
                .any(|value| value.name == operation.variant);
            if !writable_variant {
                return Err(Error::message(format!(
                    "PAC API fixed-register-write {:?} references unknown writable variant {:?}",
                    operation.name, operation.variant
                )));
            }
        }

        for operation in &self.fixed_register_images {
            ordinary_writable_register(
                &device,
                &operation.name,
                &operation.peripheral,
                &operation.register,
            )?;
        }
        for operation in &self.w1c_register_snapshots {
            let binding = register(
                &device,
                &operation.name,
                &operation.peripheral,
                &operation.register,
            )?;
            require_size_access(&operation.name, binding, Access::ReadWrite, "read-write")?;
            if binding.info.modified_write_values != Some(ModifiedWriteValues::OneToClear) {
                return Err(Error::message(format!(
                    "PAC API w1c-register-snapshot {:?} register must be one-to-clear",
                    operation.name
                )));
            }
            if binding.is_array {
                return Err(Error::message(format!(
                    "PAC API w1c-register-snapshot {:?} requires one exact non-array register",
                    operation.name
                )));
            }
            let field = field(&operation.name, binding.info, &operation.field)?;
            if field.bit_width() == 0 || field.bit_width() > 32 {
                return Err(Error::message(format!(
                    "PAC API w1c-register-snapshot {:?} field has invalid width {}",
                    operation.name,
                    field.bit_width()
                )));
            }
        }
        for operation in &self.register_image_writes {
            ordinary_writable_register(
                &device,
                &operation.name,
                &operation.peripheral,
                &operation.register,
            )?;
        }
        for operation in &self.zero_register_writes {
            ordinary_writable_register(
                &device,
                &operation.name,
                &operation.peripheral,
                &operation.register,
            )?;
        }

        for operation in &self.zero_based_field_writes {
            let binding = writable_register(
                &device,
                &operation.name,
                &operation.peripheral,
                &operation.register,
            )?;
            for field_name in &operation.fields {
                let field = field(&operation.name, binding.info, field_name)?;
                if field.access == Some(Access::ReadOnly) {
                    return Err(Error::message(format!(
                        "PAC API zero-based-field-write {:?} field {field_name:?} is read-only",
                        operation.name
                    )));
                }
                let width = field.bit_width();
                if !(1..=32).contains(&width) {
                    return Err(Error::message(format!(
                        "PAC API zero-based-field-write {:?} field {field_name:?} has invalid width {width}",
                        operation.name
                    )));
                }
                if width != 1 {
                    require_full_range(&operation.name, field, width)?;
                }
            }
        }

        for operation in &self.masked_register_modifies {
            let binding = register(
                &device,
                &operation.name,
                &operation.peripheral,
                &operation.register,
            )?;
            require_size_access(&operation.name, binding, Access::ReadWrite, "read-write")?;
            require_ordinary(&operation.name, binding.info)?;
        }
        for operation in &self.indexed_bit_set_modifies {
            let binding = writable_register(
                &device,
                &operation.name,
                &operation.peripheral,
                &operation.register,
            )?;
            require_size_access(&operation.name, binding, Access::ReadWrite, "read-write")?;
            require_ordinary(&operation.name, binding.info)?;
            let field = field(&operation.name, binding.info, &operation.field)?;
            require_full_field(&operation.name, field)?;
            require_full_range(&operation.name, field, 32)?;
        }
        for domain in &self.opaque_domains {
            // A register-specific opaque value domain is a type boundary, not
            // an access semantic. It is valid for ordinary, W1C and other
            // SVD-declared writable registers; the bound operation below is
            // still responsible for validating the actual transaction.
            writable_register(&device, &domain.name, &domain.peripheral, &domain.register)?;
        }
        Ok(())
    }

    fn validate_ownership_partitions_against_svd(&self, device: &Device) -> Result<()> {
        if self.ownership_partitions.is_empty() {
            return Ok(());
        }

        let mut known = BTreeSet::new();
        for peripheral in &device.peripherals {
            if matches!(peripheral, MaybeArray::Array(_, _)) {
                return Err(Error::message(format!(
                    "PAC API ownership does not support SVD peripheral arrays: {:?}",
                    peripheral.name
                )));
            }
            if !known.insert(peripheral.name.as_str()) {
                return Err(Error::message(format!(
                    "release SVD repeats peripheral {:?}",
                    peripheral.name
                )));
            }
        }
        let assigned = self
            .ownership_partitions
            .iter()
            .flat_map(|partition| partition.peripherals.iter().map(String::as_str))
            .collect::<BTreeSet<_>>();
        let unknown = assigned.difference(&known).copied().collect::<Vec<_>>();
        if !unknown.is_empty() {
            return Err(Error::message(format!(
                "PAC API ownership references unknown SVD peripherals: {}",
                unknown.join(", ")
            )));
        }
        let missing = known.difference(&assigned).copied().collect::<Vec<_>>();
        if !missing.is_empty() {
            return Err(Error::message(format!(
                "PAC API ownership leaves SVD peripherals unassigned: {}",
                missing.join(", ")
            )));
        }
        self.validate_partition_register_ranges(device)?;
        Ok(())
    }

    fn validate_partition_register_ranges(&self, device: &Device) -> Result<()> {
        let partitions = self
            .ownership_partitions
            .iter()
            .flat_map(|partition| {
                partition
                    .peripherals
                    .iter()
                    .map(|peripheral| (peripheral.as_str(), partition.name.as_str()))
            })
            .collect::<BTreeMap<_, _>>();
        let mut ranges = Vec::new();
        for peripheral in &device.peripherals {
            let partition = partitions.get(peripheral.name.as_str()).ok_or_else(|| {
                Error::message(format!(
                    "PAC API ownership has no partition for SVD peripheral {:?}",
                    peripheral.name
                ))
            })?;
            let properties = merge_properties(
                device.default_register_properties,
                peripheral.default_register_properties,
            );
            if let Some(registers) = &peripheral.registers {
                let context = RegisterRangeContext {
                    partition,
                    peripheral_base: peripheral.base_address,
                };
                collect_owned_register_ranges(
                    context,
                    0,
                    &peripheral.name,
                    registers,
                    properties,
                    &mut ranges,
                )?;
            }
        }

        ranges.sort_by(|left, right| {
            (
                left.start,
                left.end_exclusive,
                &left.partition,
                &left.identity,
            )
                .cmp(&(
                    right.start,
                    right.end_exclusive,
                    &right.partition,
                    &right.identity,
                ))
        });
        for (index, left) in ranges.iter().enumerate() {
            for right in &ranges[index + 1..] {
                if right.start >= left.end_exclusive {
                    break;
                }
                if left.partition == right.partition
                    || left.start >= right.end_exclusive
                    || right.start >= left.end_exclusive
                {
                    continue;
                }
                let overlap_start = left.start.max(right.start);
                let overlap_end = left.end_exclusive.min(right.end_exclusive);
                return Err(Error::message(format!(
                    "PAC API ownership partitions {:?} and {:?} overlap physical register bytes {overlap_start:#x}..{overlap_end:#x}: {} ({:#x}..{:#x}) and {} ({:#x}..{:#x})",
                    left.partition,
                    right.partition,
                    left.identity,
                    left.start,
                    left.end_exclusive,
                    right.identity,
                    right.start,
                    right.end_exclusive,
                )));
            }
        }
        Ok(())
    }
}

fn collect_owned_register_ranges(
    context: RegisterRangeContext<'_>,
    parent_offset: u64,
    parent_identity: &str,
    children: &[RegisterCluster],
    inherited: RegisterProperties,
    ranges: &mut Vec<OwnedRegisterRange>,
) -> Result<()> {
    for child in children {
        match child {
            RegisterCluster::Register(register) => match register {
                MaybeArray::Single(info) => collect_owned_register_range(
                    context,
                    parent_offset,
                    parent_identity,
                    info,
                    inherited,
                    ranges,
                )?,
                MaybeArray::Array(info, dimension) => {
                    for expanded in svd_rs::register::expand(info, dimension) {
                        collect_owned_register_range(
                            context,
                            parent_offset,
                            parent_identity,
                            &expanded,
                            inherited,
                            ranges,
                        )?;
                    }
                }
            },
            RegisterCluster::Cluster(cluster) => match cluster {
                MaybeArray::Single(info) => collect_owned_cluster_ranges(
                    context,
                    parent_offset,
                    parent_identity,
                    info,
                    inherited,
                    ranges,
                )?,
                MaybeArray::Array(info, dimension) => {
                    for expanded in svd_rs::cluster::expand(info, dimension) {
                        collect_owned_cluster_ranges(
                            context,
                            parent_offset,
                            parent_identity,
                            &expanded,
                            inherited,
                            ranges,
                        )?;
                    }
                }
            },
        }
    }
    Ok(())
}

fn collect_owned_register_range(
    context: RegisterRangeContext<'_>,
    parent_offset: u64,
    parent_identity: &str,
    register: &RegisterInfo,
    inherited: RegisterProperties,
    ranges: &mut Vec<OwnedRegisterRange>,
) -> Result<()> {
    let identity = format!("{parent_identity}.{}", register.name);
    let properties = merge_properties(inherited, register.properties);
    let size_bits = properties
        .size
        .ok_or_else(|| Error::message(format!("register {identity} has no inherited size")))?;
    if size_bits == 0 {
        return Err(Error::message(format!(
            "register {identity} has zero physical size"
        )));
    }
    let start = context
        .peripheral_base
        .checked_add(parent_offset)
        .and_then(|base| base.checked_add(u64::from(register.address_offset)))
        .ok_or_else(|| Error::message(format!("register {identity} address overflows u64")))?;
    let end_exclusive = start
        .checked_add(u64::from(size_bits).div_ceil(8))
        .ok_or_else(|| Error::message(format!("register {identity} range overflows u64")))?;
    ranges.push(OwnedRegisterRange {
        partition: context.partition.to_owned(),
        identity,
        start,
        end_exclusive,
    });
    Ok(())
}

fn collect_owned_cluster_ranges(
    context: RegisterRangeContext<'_>,
    parent_offset: u64,
    parent_identity: &str,
    cluster: &svd_rs::ClusterInfo,
    inherited: RegisterProperties,
    ranges: &mut Vec<OwnedRegisterRange>,
) -> Result<()> {
    let identity = format!("{parent_identity}.{}", cluster.name);
    let offset = parent_offset
        .checked_add(u64::from(cluster.address_offset))
        .ok_or_else(|| Error::message(format!("cluster {identity} address overflows u64")))?;
    collect_owned_register_ranges(
        context,
        offset,
        &identity,
        &cluster.children,
        merge_properties(inherited, cluster.default_register_properties),
        ranges,
    )
}

pub(super) fn register<'a>(
    device: &'a Device,
    operation: &str,
    peripheral_name: &str,
    register_name: &str,
) -> Result<RegisterBinding<'a>> {
    let peripheral = device
        .peripherals
        .iter()
        .find(|peripheral| peripheral.name == peripheral_name)
        .ok_or_else(|| {
            Error::message(format!(
                "PAC API operation {operation:?} references unknown peripheral {peripheral_name:?}"
            ))
        })?;
    let children = peripheral.registers.as_deref().ok_or_else(|| {
        Error::message(format!(
            "PAC API operation {operation:?} peripheral {peripheral_name:?} has no registers"
        ))
    })?;
    let (info, is_array) = children
        .iter()
        .find_map(|child| match child {
            RegisterCluster::Register(register) if register.name == register_name => {
                Some((&**register, matches!(register, svd_rs::MaybeArray::Array(_, _))))
            }
            _ => None,
        })
        .ok_or_else(|| {
            Error::message(format!(
                "PAC API operation {operation:?} references unknown register {peripheral_name}.{register_name}"
            ))
        })?;
    let properties = merge_properties(
        merge_properties(
            device.default_register_properties,
            peripheral.default_register_properties,
        ),
        info.properties,
    );
    Ok(RegisterBinding {
        info,
        properties,
        is_array,
    })
}

pub(super) fn field<'a>(
    operation: &str,
    register: &'a RegisterInfo,
    name: &str,
) -> Result<&'a FieldInfo> {
    register
        .fields
        .as_deref()
        .and_then(|fields| fields.iter().find(|field| field.name == name))
        .map(|field| &**field)
        .ok_or_else(|| {
            Error::message(format!(
                "PAC API operation {operation:?} references unknown field {}.{name}",
                register.name
            ))
        })
}

fn writable_register<'a>(
    device: &'a Device,
    operation: &str,
    peripheral: &str,
    register_name: &str,
) -> Result<RegisterBinding<'a>> {
    let binding = register(device, operation, peripheral, register_name)?;
    if binding.properties.size != Some(32)
        || !matches!(
            binding.properties.access,
            Some(Access::WriteOnly | Access::ReadWrite)
        )
    {
        return Err(Error::message(format!(
            "PAC API operation {operation:?} requires a writable 32-bit register"
        )));
    }
    Ok(binding)
}

fn ordinary_writable_register<'a>(
    device: &'a Device,
    operation: &str,
    peripheral: &str,
    register_name: &str,
) -> Result<RegisterBinding<'a>> {
    let binding = writable_register(device, operation, peripheral, register_name)?;
    require_ordinary(operation, binding.info)?;
    Ok(binding)
}

fn require_ordinary(operation: &str, register: &RegisterInfo) -> Result<()> {
    if register.modified_write_values.is_some() {
        return Err(Error::message(format!(
            "PAC API operation {operation:?} cannot target modified-write semantics"
        )));
    }
    Ok(())
}

fn require_size_access(
    operation: &str,
    binding: RegisterBinding<'_>,
    access: Access,
    label: &str,
) -> Result<()> {
    if binding.properties.size != Some(32) || binding.properties.access != Some(access) {
        return Err(Error::message(format!(
            "PAC API operation {operation:?} requires a 32-bit {label} register"
        )));
    }
    Ok(())
}

fn require_full_field(operation: &str, field: &FieldInfo) -> Result<()> {
    if field.bit_offset() != 0 || field.bit_width() != 32 {
        return Err(Error::message(format!(
            "PAC API operation {operation:?} field must cover the complete 32-bit register"
        )));
    }
    Ok(())
}

fn require_full_range(operation: &str, field: &FieldInfo, width: u32) -> Result<()> {
    let maximum = if width == 32 {
        u64::from(u32::MAX)
    } else {
        (1_u64 << width) - 1
    };
    if !matches!(
        field.write_constraint,
        Some(WriteConstraint::Range(range)) if range.min == 0 && range.max == maximum
    ) {
        return Err(Error::message(format!(
            "PAC API operation {operation:?} field must accept every {width}-bit value"
        )));
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    const TWO_PERIPHERAL_SVD: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<device schemaVersion="1.3" xmlns:xs="http://www.w3.org/2001/XMLSchema-instance">
  <name>FIXTURE</name>
  <version>1</version>
  <description>ownership fixture</description>
  <addressUnitBits>8</addressUnitBits>
  <width>32</width>
  <peripherals>
    <peripheral>
      <name>RADIO</name>
      <description>radio</description>
      <baseAddress>0x40000000</baseAddress>
    </peripheral>
    <peripheral>
      <name>INTERRUPT</name>
      <description>interrupt</description>
      <baseAddress>0x40001000</baseAddress>
    </peripheral>
  </peripherals>
</device>
"#;

    const PARTIALLY_OVERLAPPING_REGISTER_SVD: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<device schemaVersion="1.3" xmlns:xs="http://www.w3.org/2001/XMLSchema-instance">
  <name>FIXTURE</name>
  <version>1</version>
  <description>physical range fixture</description>
  <addressUnitBits>8</addressUnitBits>
  <width>32</width>
  <peripherals>
    <peripheral>
      <name>RADIO</name>
      <description>radio</description>
      <baseAddress>0x40000000</baseAddress>
      <registers>
        <register>
          <name>WINDOW</name>
          <description>four-byte radio window</description>
          <addressOffset>0x4</addressOffset>
          <size>32</size>
        </register>
      </registers>
    </peripheral>
    <peripheral>
      <name>INTERRUPT</name>
      <description>interrupt</description>
      <baseAddress>0x40000000</baseAddress>
      <registers>
        <register>
          <name>ALIAS</name>
          <description>two-byte overlapping view</description>
          <addressOffset>0x6</addressOffset>
          <size>16</size>
        </register>
      </registers>
    </peripheral>
  </peripherals>
</device>
"#;

    const OVERLAPPING_REGISTER_ARRAY_SVD: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<device schemaVersion="1.3" xmlns:xs="http://www.w3.org/2001/XMLSchema-instance">
  <name>FIXTURE</name>
  <version>1</version>
  <description>register array fixture</description>
  <addressUnitBits>8</addressUnitBits>
  <width>32</width>
  <peripherals>
    <peripheral>
      <name>RADIO</name>
      <description>radio</description>
      <baseAddress>0x40000000</baseAddress>
      <registers>
        <register>
          <dim>2</dim>
          <dimIncrement>4</dimIncrement>
          <dimIndex>0-1</dimIndex>
          <name>WINDOW%s</name>
          <description>radio window array</description>
          <addressOffset>0</addressOffset>
          <size>32</size>
        </register>
      </registers>
    </peripheral>
    <peripheral>
      <name>INTERRUPT</name>
      <description>interrupt</description>
      <baseAddress>0x40000004</baseAddress>
      <registers>
        <register>
          <name>ALIAS</name>
          <description>alias of the second radio window</description>
          <addressOffset>0</addressOffset>
          <size>32</size>
        </register>
      </registers>
    </peripheral>
  </peripherals>
</device>
"#;

    const W1C_REGISTER_SNAPSHOT_SVD: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<device schemaVersion="1.3" xmlns:xs="http://www.w3.org/2001/XMLSchema-instance">
  <name>FIXTURE</name>
  <version>1</version>
  <description>same-register W1C snapshot fixture</description>
  <addressUnitBits>8</addressUnitBits>
  <width>32</width>
  <peripherals>
    <peripheral>
      <name>RADIO</name>
      <description>radio</description>
      <baseAddress>0x40000000</baseAddress>
      <registers>
        <register>
          <name>EVENT_STATUS</name>
          <description>W1C event status view</description>
          <addressOffset>0</addressOffset>
          <size>32</size>
          <access>read-write</access>
          <modifiedWriteValues>oneToClear</modifiedWriteValues>
          <fields><field><name>EVENTS</name><description>events</description><bitOffset>0</bitOffset><bitWidth>14</bitWidth></field></fields>
        </register>
        <register>
          <name>CONTROL</name>
          <description>writable control</description>
          <addressOffset>4</addressOffset>
          <size>32</size>
          <access>read-write</access>
          <fields><field><name>EVENTS</name><description>events</description><bitOffset>0</bitOffset><bitWidth>14</bitWidth></field></fields>
        </register>
        <register>
          <name>SHORT_STATUS</name>
          <description>narrow W1C status</description>
          <addressOffset>8</addressOffset>
          <size>16</size>
          <access>read-write</access>
          <modifiedWriteValues>oneToClear</modifiedWriteValues>
          <fields><field><name>EVENTS</name><description>events</description><bitOffset>0</bitOffset><bitWidth>14</bitWidth></field></fields>
        </register>
        <register>
          <dim>2</dim>
          <dimIncrement>4</dimIncrement>
          <dimIndex>0-1</dimIndex>
          <name>STATUS%s</name>
          <description>W1C status array</description>
          <addressOffset>12</addressOffset>
          <size>32</size>
          <access>read-write</access>
          <modifiedWriteValues>oneToClear</modifiedWriteValues>
          <fields><field><name>EVENTS</name><description>events</description><bitOffset>0</bitOffset><bitWidth>14</bitWidth></field></fields>
        </register>
      </registers>
    </peripheral>
  </peripherals>
</device>
"#;

    fn w1c_register_snapshot_pack(register: &str) -> PacApiPack {
        toml_edit::de::from_str(&format!(
            r#"schema = 4

[[w1c-register-snapshots]]
name = "event_status"
peripheral = "RADIO"
register = "{register}"
field = "EVENTS"
sources = ["PUBLIC_EVENT_STATUS_W1C"]
"#
        ))
        .unwrap()
    }

    fn ownership_pack(peripherals: &str) -> PacApiPack {
        toml_edit::de::from_str(&format!(
            r#"schema = 4

[[ownership-partitions]]
name = "FixturePeripherals"
member = "fixture"
description = "Exact fixture ownership."
peripherals = [{peripherals}]
"#
        ))
        .unwrap()
    }

    fn separated_ownership_pack() -> PacApiPack {
        toml_edit::de::from_str(
            r#"schema = 4

[[ownership-partitions]]
name = "RadioPeripherals"
member = "radio"
description = "Radio register ownership."
peripherals = ["RADIO"]

[[ownership-partitions]]
name = "InterruptPeripherals"
member = "interrupt"
description = "Interrupt register ownership."
peripherals = ["INTERRUPT"]
"#,
        )
        .unwrap()
    }

    #[test]
    fn accepts_an_exact_exhaustive_ownership_partition() {
        let pack = ownership_pack("\"RADIO\", \"INTERRUPT\"");
        assert!(pack.validate_against_svd(TWO_PERIPHERAL_SVD).is_ok());
    }

    #[test]
    fn w1c_register_snapshot_requires_one_read_write_w1c_32_bit_target() {
        assert!(
            w1c_register_snapshot_pack("EVENT_STATUS")
                .validate_against_svd(W1C_REGISTER_SNAPSHOT_SVD)
                .is_ok()
        );

        for register in ["CONTROL", "SHORT_STATUS", "STATUS%s"] {
            let error = w1c_register_snapshot_pack(register)
                .validate_against_svd(W1C_REGISTER_SNAPSHOT_SVD)
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("one-to-clear")
                    || error.contains("32-bit")
                    || error.contains("non-array"),
                "unexpected rejection for {register}: {error}"
            );
        }
    }

    #[test]
    fn rejects_missing_and_new_svd_peripherals() {
        let pack = ownership_pack("\"RADIO\"");
        let error = pack
            .validate_against_svd(TWO_PERIPHERAL_SVD)
            .unwrap_err()
            .to_string();
        assert!(error.contains("unassigned"));
        assert!(error.contains("INTERRUPT"));
    }

    #[test]
    fn rejects_unknown_declared_peripherals() {
        let pack = ownership_pack("\"RADIO\", \"INTERRUPT\", \"GHOST\"");
        let error = pack
            .validate_against_svd(TWO_PERIPHERAL_SVD)
            .unwrap_err()
            .to_string();
        assert!(error.contains("unknown"));
        assert!(error.contains("GHOST"));
    }

    #[test]
    fn rejects_partially_overlapping_register_bytes_across_partitions() {
        let error = separated_ownership_pack()
            .validate_against_svd(PARTIALLY_OVERLAPPING_REGISTER_SVD)
            .unwrap_err()
            .to_string();

        assert!(error.contains("overlap physical register bytes 0x40000006..0x40000008"));
        assert!(error.contains("RadioPeripherals"));
        assert!(error.contains("InterruptPeripherals"));
        assert!(error.contains("RADIO.WINDOW (0x40000004..0x40000008)"));
        assert!(error.contains("INTERRUPT.ALIAS (0x40000006..0x40000008)"));
    }

    #[test]
    fn permits_overlapping_register_views_inside_one_partition() {
        let pack = ownership_pack("\"RADIO\", \"INTERRUPT\"");
        assert!(
            pack.validate_against_svd(PARTIALLY_OVERLAPPING_REGISTER_SVD)
                .is_ok()
        );
    }

    #[test]
    fn expands_register_arrays_before_checking_physical_overlap() {
        let error = separated_ownership_pack()
            .validate_against_svd(OVERLAPPING_REGISTER_ARRAY_SVD)
            .unwrap_err()
            .to_string();

        assert!(error.contains("overlap physical register bytes 0x40000004..0x40000008"));
        assert!(error.contains("RADIO.WINDOW1"));
        assert!(error.contains("INTERRUPT.ALIAS"));
    }
}
