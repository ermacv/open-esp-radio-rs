//! Cross-validation of reviewed PAC transactions against their release SVD.

use svd_rs::{
    Access, Device, FieldInfo, ModifiedWriteValues, RegisterCluster, RegisterInfo,
    RegisterProperties, Usage, WriteConstraint,
};

use crate::{PacApiPack, Result};

#[derive(Clone, Copy)]
pub(super) struct RegisterBinding<'a> {
    pub(super) info: &'a RegisterInfo,
    pub(super) properties: RegisterProperties,
    pub(super) is_array: bool,
}

impl PacApiPack {
    /// Prove that every reviewed transaction is compatible with the release SVD.
    pub fn validate_against_svd(&self, svd: &str) -> Result<()> {
        self.validate()?;
        let device = svd_parser::parse(svd).map_err(|error| error.to_string())?;

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
                return Err(format!(
                    "PAC API interrupt snapshot {:?} clear register must be 32-bit one-to-clear",
                    operation.name
                )
                .into());
            }
            let field = field(&operation.name, clear.info, &operation.clear_field)?;
            if field.bit_offset() != 0 || field.bit_width() != 32 {
                return Err(format!(
                    "PAC API interrupt snapshot {:?} clear field must cover all 32 bits",
                    operation.name
                )
                .into());
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
                format!(
                    "PAC API operation {:?} register has no fields",
                    operation.name
                )
            })?;
            if fields.len() != 1 {
                return Err(format!(
                    "PAC API full-register-write {:?} requires exactly one field",
                    operation.name
                )
                .into());
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
                format!(
                    "PAC API operation {:?} register has no fields",
                    operation.name
                )
            })?;
            if fields.len() != 1 {
                return Err(format!(
                    "PAC API fixed-register-write {:?} requires exactly one field",
                    operation.name
                )
                .into());
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
                return Err(format!(
                    "PAC API fixed-register-write {:?} references unknown writable variant {:?}",
                    operation.name, operation.variant
                )
                .into());
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
                    return Err(format!(
                        "PAC API zero-based-field-write {:?} field {field_name:?} is read-only",
                        operation.name
                    )
                    .into());
                }
                let width = field.bit_width();
                if !(1..=32).contains(&width) {
                    return Err(format!(
                        "PAC API zero-based-field-write {:?} field {field_name:?} has invalid width {width}",
                        operation.name
                    )
                    .into());
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
        Ok(())
    }
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
            format!(
                "PAC API operation {operation:?} references unknown peripheral {peripheral_name:?}"
            )
        })?;
    let children = peripheral.registers.as_deref().ok_or_else(|| {
        format!("PAC API operation {operation:?} peripheral {peripheral_name:?} has no registers")
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
            format!(
                "PAC API operation {operation:?} references unknown register {peripheral_name}.{register_name}"
            )
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
            format!(
                "PAC API operation {operation:?} references unknown field {}.{name}",
                register.name
            )
            .into()
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
        return Err(
            format!("PAC API operation {operation:?} requires a writable 32-bit register").into(),
        );
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
        return Err(format!(
            "PAC API operation {operation:?} cannot target modified-write semantics"
        )
        .into());
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
        return Err(
            format!("PAC API operation {operation:?} requires a 32-bit {label} register").into(),
        );
    }
    Ok(())
}

fn require_full_field(operation: &str, field: &FieldInfo) -> Result<()> {
    if field.bit_offset() != 0 || field.bit_width() != 32 {
        return Err(format!(
            "PAC API operation {operation:?} field must cover the complete 32-bit register"
        )
        .into());
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
        return Err(format!(
            "PAC API operation {operation:?} field must accept every {width}-bit value"
        )
        .into());
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
