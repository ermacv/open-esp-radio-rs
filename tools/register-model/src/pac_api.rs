//! Reviewed, target-owned safe PAC transaction declarations.

use std::{collections::BTreeSet, fs, path::Path};

use serde::{Deserialize, Serialize};

use crate::Result;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct PacApiPack {
    pub schema: u32,
    #[serde(default)]
    pub options: PacApiOptions,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub interrupt_snapshots: Vec<InterruptSnapshot>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub full_register_writes: Vec<FullRegisterWrite>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fixed_register_writes: Vec<FixedRegisterWrite>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fixed_register_images: Vec<FixedRegisterImage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub register_image_writes: Vec<RegisterImageWrite>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub zero_based_field_writes: Vec<ZeroBasedFieldWrite>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub zero_register_writes: Vec<ZeroRegisterWrite>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub masked_register_modifies: Vec<MaskedRegisterModify>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct PacApiOptions {
    #[serde(default)]
    pub peripheral_ownership: bool,
    #[serde(default)]
    pub device_access: bool,
    #[serde(default)]
    pub allow_clippy_empty_docs: bool,
}

macro_rules! common_operation {
    ($name:ident { $($field:ident: $type:ty),* $(,)? }) => {
        #[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
        #[serde(rename_all = "kebab-case", deny_unknown_fields)]
        pub struct $name {
            pub name: String,
            pub peripheral: String,
            $(pub $field: $type,)*
            pub sources: Vec<String>,
        }
    };
}

common_operation!(InterruptSnapshot {
    status_register: String,
    clear_register: String,
    clear_field: String,
});
common_operation!(FullRegisterWrite {
    register: String,
    field: String,
});
common_operation!(FixedRegisterWrite {
    register: String,
    field: String,
    variant: String,
});
common_operation!(FixedRegisterImage {
    register: String,
    value: u32,
});
common_operation!(RegisterImageWrite { register: String });
common_operation!(ZeroBasedFieldWrite {
    register: String,
    fields: Vec<String>,
});
common_operation!(ZeroRegisterWrite { register: String });
common_operation!(MaskedRegisterModify {
    register: String,
    preserve_mask: u32,
    input_mask: u32,
    set_mask: u32,
});

impl PacApiPack {
    pub fn load(path: &Path) -> Result<Self> {
        let input = fs::read_to_string(path)?;
        let pack: Self = toml_edit::de::from_str(&input)?;
        pack.validate()?;
        Ok(pack)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema != 1 {
            return Err(format!("PAC API pack requires schema = 1, got {}", self.schema).into());
        }
        if self.options.peripheral_ownership && self.interrupt_snapshots.is_empty() {
            return Err(
                "PAC API peripheral-ownership requires at least one interrupt snapshot".into(),
            );
        }
        validate_operations("interrupt-snapshot", &self.interrupt_snapshots)?;
        validate_operations("full-register-write", &self.full_register_writes)?;
        validate_operations("fixed-register-write", &self.fixed_register_writes)?;
        validate_operations("fixed-register-image", &self.fixed_register_images)?;
        validate_operations("register-image-write", &self.register_image_writes)?;
        validate_operations("zero-based-field-write", &self.zero_based_field_writes)?;
        validate_operations("zero-register-write", &self.zero_register_writes)?;
        validate_operations("masked-register-modify", &self.masked_register_modifies)?;

        for operation in &self.fixed_register_writes {
            validate_component("field", &operation.name, &operation.field)?;
            validate_component("variant", &operation.name, &operation.variant)?;
        }
        for operation in &self.full_register_writes {
            validate_component("field", &operation.name, &operation.field)?;
        }
        for operation in &self.interrupt_snapshots {
            validate_component(
                "status-register",
                &operation.name,
                &operation.status_register,
            )?;
            validate_component("clear-register", &operation.name, &operation.clear_register)?;
            validate_component("clear-field", &operation.name, &operation.clear_field)?;
        }
        for operation in &self.zero_based_field_writes {
            if operation.fields.is_empty() {
                return Err(format!(
                    "PAC API operation {:?} requires at least one field",
                    operation.name
                )
                .into());
            }
            let mut fields = BTreeSet::new();
            for field in &operation.fields {
                validate_component("field", &operation.name, field)?;
                if !fields.insert(field) {
                    return Err(format!(
                        "PAC API operation {:?} repeats field {field:?}",
                        operation.name
                    )
                    .into());
                }
            }
        }
        for operation in &self.masked_register_modifies {
            if operation.preserve_mask & operation.input_mask != 0
                || operation.preserve_mask & operation.set_mask != 0
                || operation.input_mask & operation.set_mask != 0
            {
                return Err(format!(
                    "PAC API masked-register-modify {:?} has overlapping masks",
                    operation.name
                )
                .into());
            }
            if operation.preserve_mask | operation.input_mask | operation.set_mask != u32::MAX {
                return Err(format!(
                    "PAC API masked-register-modify {:?} masks do not partition all 32 bits",
                    operation.name
                )
                .into());
            }
        }
        Ok(())
    }

    pub fn operation_count(&self) -> usize {
        self.interrupt_snapshots.len()
            + self.full_register_writes.len()
            + self.fixed_register_writes.len()
            + self.fixed_register_images.len()
            + self.register_image_writes.len()
            + self.zero_based_field_writes.len()
            + self.zero_register_writes.len()
            + self.masked_register_modifies.len()
    }

    pub fn source_ids(&self) -> BTreeSet<&str> {
        self.interrupt_snapshots
            .iter()
            .map(Operation::sources)
            .chain(self.full_register_writes.iter().map(Operation::sources))
            .chain(self.fixed_register_writes.iter().map(Operation::sources))
            .chain(self.fixed_register_images.iter().map(Operation::sources))
            .chain(self.register_image_writes.iter().map(Operation::sources))
            .chain(self.zero_based_field_writes.iter().map(Operation::sources))
            .chain(self.zero_register_writes.iter().map(Operation::sources))
            .chain(self.masked_register_modifies.iter().map(Operation::sources))
            .flatten()
            .map(String::as_str)
            .collect()
    }
}

trait Operation {
    fn name(&self) -> &str;
    fn peripheral(&self) -> &str;
    fn register(&self) -> Option<&str>;
    fn sources(&self) -> &[String];
}

macro_rules! impl_register_operation {
    ($type:ty) => {
        impl Operation for $type {
            fn name(&self) -> &str {
                &self.name
            }
            fn peripheral(&self) -> &str {
                &self.peripheral
            }
            fn register(&self) -> Option<&str> {
                Some(&self.register)
            }
            fn sources(&self) -> &[String] {
                &self.sources
            }
        }
    };
}

impl Operation for InterruptSnapshot {
    fn name(&self) -> &str {
        &self.name
    }
    fn peripheral(&self) -> &str {
        &self.peripheral
    }
    fn register(&self) -> Option<&str> {
        None
    }
    fn sources(&self) -> &[String] {
        &self.sources
    }
}

impl_register_operation!(FullRegisterWrite);
impl_register_operation!(FixedRegisterWrite);
impl_register_operation!(FixedRegisterImage);
impl_register_operation!(RegisterImageWrite);
impl_register_operation!(ZeroBasedFieldWrite);
impl_register_operation!(ZeroRegisterWrite);
impl_register_operation!(MaskedRegisterModify);

fn validate_operations<T: Operation>(kind: &str, operations: &[T]) -> Result<()> {
    let mut names = BTreeSet::new();
    for operation in operations {
        if !is_lower_snake_case(operation.name()) {
            return Err(format!(
                "PAC API {kind} name {:?} is not lower snake case",
                operation.name()
            )
            .into());
        }
        if !names.insert(operation.name()) {
            return Err(format!(
                "PAC API contains duplicate {kind} name {:?}",
                operation.name()
            )
            .into());
        }
        validate_component("peripheral", operation.name(), operation.peripheral())?;
        if let Some(register) = operation.register() {
            validate_component("register", operation.name(), register)?;
        }
        if operation.sources().is_empty() {
            return Err(format!("PAC API operation {:?} has no sources", operation.name()).into());
        }
        let mut sources = BTreeSet::new();
        for source in operation.sources() {
            if source.is_empty() || !sources.insert(source) {
                return Err(format!(
                    "PAC API operation {:?} has an empty or duplicate source",
                    operation.name()
                )
                .into());
            }
        }
    }
    Ok(())
}

fn validate_component(kind: &str, operation: &str, value: &str) -> Result<()> {
    if value.is_empty() || value.chars().any(char::is_whitespace) {
        return Err(format!("PAC API operation {operation:?} has invalid {kind} {value:?}").into());
    }
    Ok(())
}

fn is_lower_snake_case(value: &str) -> bool {
    value.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && value
            .bytes()
            .all(|byte| byte == b'_' || byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_pack() -> PacApiPack {
        PacApiPack {
            schema: 1,
            options: PacApiOptions::default(),
            interrupt_snapshots: Vec::new(),
            full_register_writes: Vec::new(),
            fixed_register_writes: Vec::new(),
            fixed_register_images: Vec::new(),
            register_image_writes: Vec::new(),
            zero_based_field_writes: Vec::new(),
            zero_register_writes: Vec::new(),
            masked_register_modifies: Vec::new(),
        }
    }

    #[test]
    fn accepts_a_partitioned_masked_transaction() {
        let mut pack = empty_pack();
        pack.masked_register_modifies.push(MaskedRegisterModify {
            name: "publish_command".to_owned(),
            peripheral: "RADIO".to_owned(),
            register: "COMMAND".to_owned(),
            preserve_mask: 0xffff_0000,
            input_mask: 0x0000_fff0,
            set_mask: 0x0000_000f,
            sources: vec!["REVIEW".to_owned()],
        });
        assert_eq!(pack.operation_count(), 1);
        assert!(pack.validate().is_ok());
    }

    #[test]
    fn rejects_implicit_ownership_and_incomplete_masks() {
        let mut pack = empty_pack();
        pack.options.peripheral_ownership = true;
        assert!(pack.validate().is_err());
        pack.options.peripheral_ownership = false;
        pack.masked_register_modifies.push(MaskedRegisterModify {
            name: "publish_command".to_owned(),
            peripheral: "RADIO".to_owned(),
            register: "COMMAND".to_owned(),
            preserve_mask: 0xffff_0000,
            input_mask: 0x0000_fff0,
            set_mask: 0,
            sources: vec!["REVIEW".to_owned()],
        });
        assert!(pack.validate().is_err());
    }
}
