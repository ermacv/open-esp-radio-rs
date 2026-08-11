//! Reviewed, target-owned safe PAC transaction declarations.

use std::{collections::BTreeSet, fs, path::Path};

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct PacApiPack {
    pub schema: u32,
    #[serde(default)]
    pub options: PacApiOptions,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flag_domains: Vec<FlagDomain>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enum_domains: Vec<EnumDomain>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bounded_domains: Vec<BoundedDomain>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub opaque_domains: Vec<OpaqueDomain>,
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

/// Closed writable bit-mask domain emitted into the public PAC facade.
///
/// Values are reviewed capabilities, not an exhaustive description of every
/// hardware bit. The generated type has no public integer constructor.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct FlagDomain {
    pub name: String,
    pub description: String,
    pub values: Vec<FlagValue>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct FlagValue {
    pub name: String,
    pub value: u32,
    pub description: String,
    pub sources: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct EnumDomain {
    pub name: String,
    pub description: String,
    pub values: Vec<EnumValue>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct EnumValue {
    pub name: String,
    pub value: u32,
    pub description: String,
    pub sources: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct BoundedDomain {
    pub name: String,
    pub description: String,
    pub min: u32,
    pub max: u32,
    pub sources: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct OpaqueDomain {
    pub name: String,
    pub description: String,
    pub peripheral: String,
    pub register: String,
    pub sources: Vec<String>,
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
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct FullRegisterWrite {
    pub name: String,
    pub peripheral: String,
    pub register: String,
    pub field: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    pub sources: Vec<String>,
}
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
        let pack: Self = toml_edit::de::from_str(&input)
            .map_err(|error| Error::manifest("PAC API pack", path, error))?;
        pack.validate()
            .map_err(|error| Error::manifest("PAC API pack", path, error))?;
        Ok(pack)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema != 2 {
            return Err(Error::message(format!(
                "PAC API pack requires schema = 2, got {}",
                self.schema
            )));
        }
        self.validate_domains()?;
        let domain_names = self.domain_names();
        if self.options.peripheral_ownership && self.interrupt_snapshots.is_empty() {
            return Err(Error::message(
                "PAC API peripheral-ownership requires at least one interrupt snapshot",
            ));
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
            if let Some(domain) = &operation.domain {
                if !domain_names.contains(domain.as_str()) {
                    return Err(Error::message(format!(
                        "PAC API full-register-write {:?} references unknown domain {domain:?}",
                        operation.name
                    )));
                }
                if let Some(opaque) = self
                    .opaque_domains
                    .iter()
                    .find(|candidate| candidate.name == *domain)
                {
                    if opaque.peripheral != operation.peripheral
                        || opaque.register != operation.register
                    {
                        return Err(Error::message(format!(
                            "PAC API full-register-write {:?} uses opaque domain {domain:?} outside its register {}.{}",
                            operation.name, opaque.peripheral, opaque.register
                        )));
                    }
                }
            }
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
                return Err(Error::message(format!(
                    "PAC API operation {:?} requires at least one field",
                    operation.name
                )));
            }
            let mut fields = BTreeSet::new();
            for field in &operation.fields {
                validate_component("field", &operation.name, field)?;
                if !fields.insert(field) {
                    return Err(Error::message(format!(
                        "PAC API operation {:?} repeats field {field:?}",
                        operation.name
                    )));
                }
            }
        }
        for operation in &self.masked_register_modifies {
            if operation.preserve_mask & operation.input_mask != 0
                || operation.preserve_mask & operation.set_mask != 0
                || operation.input_mask & operation.set_mask != 0
            {
                return Err(Error::message(format!(
                    "PAC API masked-register-modify {:?} has overlapping masks",
                    operation.name
                )));
            }
            if operation.preserve_mask | operation.input_mask | operation.set_mask != u32::MAX {
                return Err(Error::message(format!(
                    "PAC API masked-register-modify {:?} masks do not partition all 32 bits",
                    operation.name
                )));
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

    pub fn domain_count(&self) -> usize {
        self.flag_domains.len()
            + self.enum_domains.len()
            + self.bounded_domains.len()
            + self.opaque_domains.len()
    }

    pub fn source_ids(&self) -> BTreeSet<&str> {
        self.flag_domains
            .iter()
            .flat_map(|domain| &domain.values)
            .flat_map(|value| &value.sources)
            .map(String::as_str)
            .chain(
                self.enum_domains
                    .iter()
                    .flat_map(|domain| &domain.values)
                    .flat_map(|value| &value.sources)
                    .map(String::as_str),
            )
            .chain(
                self.bounded_domains
                    .iter()
                    .flat_map(|domain| &domain.sources)
                    .map(String::as_str),
            )
            .chain(
                self.opaque_domains
                    .iter()
                    .flat_map(|domain| &domain.sources)
                    .map(String::as_str),
            )
            .chain(
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
                    .map(String::as_str),
            )
            .collect()
    }

    fn validate_domains(&self) -> Result<()> {
        let mut domain_names = BTreeSet::new();
        for domain in &self.flag_domains {
            validate_domain_header("flag", &domain.name, &domain.description, &mut domain_names)?;
            if domain.description.trim().is_empty() || domain.values.is_empty() {
                return Err(Error::message(format!(
                    "PAC API flag domain {:?} requires a description and at least one value",
                    domain.name
                )));
            }
            let mut value_names = BTreeSet::new();
            let mut values = BTreeSet::new();
            for value in &domain.values {
                if !is_upper_snake_case(&value.name) {
                    return Err(Error::message(format!(
                        "PAC API flag value {}::{:?} is not UPPER_SNAKE_CASE",
                        domain.name, value.name
                    )));
                }
                if !value_names.insert(value.name.as_str()) || !values.insert(value.value) {
                    return Err(Error::message(format!(
                        "PAC API flag domain {:?} repeats a name or numeric value",
                        domain.name
                    )));
                }
                if value.description.trim().is_empty() || value.sources.is_empty() {
                    return Err(Error::message(format!(
                        "PAC API flag value {}::{} requires a description and evidence sources",
                        domain.name, value.name
                    )));
                }
                let mut sources = BTreeSet::new();
                if value
                    .sources
                    .iter()
                    .any(|source| source.is_empty() || !sources.insert(source))
                {
                    return Err(Error::message(format!(
                        "PAC API flag value {}::{} has an empty or duplicate source",
                        domain.name, value.name
                    )));
                }
            }
        }
        for domain in &self.enum_domains {
            validate_domain_header("enum", &domain.name, &domain.description, &mut domain_names)?;
            if domain.values.is_empty() {
                return Err(Error::message(format!(
                    "PAC API enum domain {:?} requires at least one value",
                    domain.name
                )));
            }
            let mut value_names = BTreeSet::new();
            let mut values = BTreeSet::new();
            for value in &domain.values {
                if !is_upper_camel_case(&value.name) {
                    return Err(Error::message(format!(
                        "PAC API enum value {}::{:?} is not UpperCamelCase",
                        domain.name, value.name
                    )));
                }
                if !value_names.insert(value.name.as_str()) || !values.insert(value.value) {
                    return Err(Error::message(format!(
                        "PAC API enum domain {:?} repeats a name or numeric value",
                        domain.name
                    )));
                }
                validate_domain_evidence(
                    "enum value",
                    &format!("{}::{}", domain.name, value.name),
                    &value.description,
                    &value.sources,
                )?;
            }
        }
        for domain in &self.bounded_domains {
            validate_domain_header(
                "bounded",
                &domain.name,
                &domain.description,
                &mut domain_names,
            )?;
            if domain.min > domain.max {
                return Err(Error::message(format!(
                    "PAC API bounded domain {:?} has min greater than max",
                    domain.name
                )));
            }
            validate_sources("bounded domain", &domain.name, &domain.sources)?;
        }
        for domain in &self.opaque_domains {
            validate_domain_header(
                "opaque",
                &domain.name,
                &domain.description,
                &mut domain_names,
            )?;
            validate_component("peripheral", &domain.name, &domain.peripheral)?;
            validate_component("register", &domain.name, &domain.register)?;
            validate_sources("opaque domain", &domain.name, &domain.sources)?;
        }
        Ok(())
    }

    fn domain_names(&self) -> BTreeSet<&str> {
        self.flag_domains
            .iter()
            .map(|domain| domain.name.as_str())
            .chain(self.enum_domains.iter().map(|domain| domain.name.as_str()))
            .chain(
                self.bounded_domains
                    .iter()
                    .map(|domain| domain.name.as_str()),
            )
            .chain(
                self.opaque_domains
                    .iter()
                    .map(|domain| domain.name.as_str()),
            )
            .collect()
    }
}

fn validate_domain_header<'a>(
    kind: &str,
    name: &'a str,
    description: &str,
    names: &mut BTreeSet<&'a str>,
) -> Result<()> {
    if !is_upper_camel_case(name) {
        return Err(Error::message(format!(
            "PAC API {kind} domain name {name:?} is not UpperCamelCase"
        )));
    }
    if !names.insert(name) {
        return Err(Error::message(format!(
            "PAC API contains duplicate domain {name:?}"
        )));
    }
    if description.trim().is_empty() {
        return Err(Error::message(format!(
            "PAC API {kind} domain {name:?} requires a description"
        )));
    }
    Ok(())
}

fn validate_domain_evidence(
    kind: &str,
    name: &str,
    description: &str,
    sources: &[String],
) -> Result<()> {
    if description.trim().is_empty() {
        return Err(Error::message(format!(
            "PAC API {kind} {name} requires a description"
        )));
    }
    validate_sources(kind, name, sources)
}

fn validate_sources(kind: &str, name: &str, sources: &[String]) -> Result<()> {
    if sources.is_empty() {
        return Err(Error::message(format!(
            "PAC API {kind} {name} requires evidence sources"
        )));
    }
    let mut unique = BTreeSet::new();
    if sources
        .iter()
        .any(|source| source.is_empty() || !unique.insert(source))
    {
        return Err(Error::message(format!(
            "PAC API {kind} {name} has an empty or duplicate source"
        )));
    }
    Ok(())
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
            return Err(Error::message(format!(
                "PAC API {kind} name {:?} is not lower snake case",
                operation.name()
            )));
        }
        if !names.insert(operation.name()) {
            return Err(Error::message(format!(
                "PAC API contains duplicate {kind} name {:?}",
                operation.name()
            )));
        }
        validate_component("peripheral", operation.name(), operation.peripheral())?;
        if let Some(register) = operation.register() {
            validate_component("register", operation.name(), register)?;
        }
        if operation.sources().is_empty() {
            return Err(Error::message(format!(
                "PAC API operation {:?} has no sources",
                operation.name()
            )));
        }
        let mut sources = BTreeSet::new();
        for source in operation.sources() {
            if source.is_empty() || !sources.insert(source) {
                return Err(Error::message(format!(
                    "PAC API operation {:?} has an empty or duplicate source",
                    operation.name()
                )));
            }
        }
    }
    Ok(())
}

fn validate_component(kind: &str, operation: &str, value: &str) -> Result<()> {
    if value.is_empty() || value.chars().any(char::is_whitespace) {
        return Err(Error::message(format!(
            "PAC API operation {operation:?} has invalid {kind} {value:?}"
        )));
    }
    Ok(())
}

fn is_lower_snake_case(value: &str) -> bool {
    value.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && value
            .bytes()
            .all(|byte| byte == b'_' || byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

fn is_upper_snake_case(value: &str) -> bool {
    value.as_bytes().first().is_some_and(u8::is_ascii_uppercase)
        && value
            .bytes()
            .all(|byte| byte == b'_' || byte.is_ascii_uppercase() || byte.is_ascii_digit())
}

fn is_upper_camel_case(value: &str) -> bool {
    value.as_bytes().first().is_some_and(u8::is_ascii_uppercase)
        && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
        && !value.contains('_')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_pack() -> PacApiPack {
        PacApiPack {
            schema: 2,
            options: PacApiOptions::default(),
            flag_domains: Vec::new(),
            enum_domains: Vec::new(),
            bounded_domains: Vec::new(),
            opaque_domains: Vec::new(),
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
    fn validates_closed_flag_domains() {
        let mut pack = empty_pack();
        pack.flag_domains.push(FlagDomain {
            name: "InterruptMask".to_owned(),
            description: "Reviewed writable interrupt bits.".to_owned(),
            values: vec![FlagValue {
                name: "RX_READY".to_owned(),
                value: 1 << 3,
                description: "Enable the receive-ready interrupt.".to_owned(),
                sources: vec!["VENDOR_IRQ_ENABLE".to_owned()],
            }],
        });
        assert_eq!(pack.domain_count(), 1);
        assert!(pack.validate().is_ok());
        assert_eq!(pack.source_ids(), BTreeSet::from(["VENDOR_IRQ_ENABLE"]));
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

    #[test]
    fn rejects_removed_schema_one_without_compatibility() {
        let mut pack = empty_pack();
        pack.schema = 1;
        assert!(
            pack.validate()
                .unwrap_err()
                .to_string()
                .contains("requires schema = 2")
        );
    }

    #[test]
    fn typed_register_write_requires_a_declared_domain() {
        let mut pack = empty_pack();
        pack.full_register_writes.push(FullRegisterWrite {
            name: "write_interrupt_mask".to_owned(),
            peripheral: "RADIO".to_owned(),
            register: "ENABLE".to_owned(),
            field: "EVENTS".to_owned(),
            domain: Some("InterruptMask".to_owned()),
            sources: vec!["VENDOR_IRQ".to_owned()],
        });
        assert!(
            pack.validate()
                .unwrap_err()
                .to_string()
                .contains("unknown domain")
        );
    }
}
