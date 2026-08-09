//! Parser for the project-generated PAC binding index.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use crate::Result;
use serde::Deserialize;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
pub enum PacAccess {
    #[serde(rename = "read-only")]
    ReadOnly,
    #[serde(rename = "write-only")]
    WriteOnly,
    #[serde(rename = "read-write")]
    ReadWrite,
    #[serde(rename = "read-writeOnce")]
    ReadWriteOnce,
    #[serde(rename = "writeOnce")]
    WriteOnce,
    #[serde(rename = "unspecified")]
    Unspecified,
}

impl PacAccess {
    pub const fn readable(self) -> bool {
        matches!(self, Self::ReadOnly | Self::ReadWrite | Self::ReadWriteOnce)
    }

    pub const fn writable(self) -> bool {
        matches!(
            self,
            Self::WriteOnly | Self::ReadWrite | Self::ReadWriteOnce | Self::WriteOnce
        )
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PacScopeBinding {
    pub method: String,
    pub index: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct PacFieldBinding {
    pub svd_name: String,
    pub method: String,
    pub index: Option<u32>,
    pub bit_offset: u8,
    pub bit_width: u8,
    pub access: PacAccess,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct PacRegisterBinding {
    pub address: u32,
    pub width: u8,
    pub access: PacAccess,
    pub identity: String,
    pub peripheral: String,
    pub peripheral_type: String,
    pub peripheral_module: String,
    pub scope: Vec<PacScopeBinding>,
    pub register_method: String,
    pub register_index: Option<u32>,
    pub alternate_register: Option<String>,
    pub fields: Vec<PacFieldBinding>,
}

impl PacRegisterBinding {
    pub fn method_path(&self, root: &str) -> String {
        let mut output = root.to_owned();
        for scope in &self.scope {
            output.push('.');
            output.push_str(&scope.method);
            match scope.index {
                Some(index) => output.push_str(&format!("({index})")),
                None => output.push_str("()"),
            }
        }
        output.push('.');
        output.push_str(&self.register_method);
        match self.register_index {
            Some(index) => output.push_str(&format!("({index})")),
            None => output.push_str("()"),
        }
        output
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PacBindingIndex {
    pub crate_name: String,
    registers: BTreeMap<(u32, u8), Vec<PacRegisterBinding>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct PacBindingDocument {
    schema: u32,
    crate_name: String,
    registers: Vec<PacRegisterBinding>,
}

impl PacBindingIndex {
    pub fn load(path: &Path) -> Result<Self> {
        Self::parse(&fs::read_to_string(path)?)
    }

    pub fn parse(input: &str) -> Result<Self> {
        let document: PacBindingDocument = toml_edit::de::from_str(input)?;
        if document.schema != 2 {
            return Err("PAC binding TOML requires schema = 2".into());
        }
        if document.crate_name.is_empty() || document.crate_name.contains(char::is_whitespace) {
            return Err("PAC binding TOML has an invalid crate-name".into());
        }
        let mut registers = BTreeMap::<(u32, u8), Vec<PacRegisterBinding>>::new();
        let mut identities = BTreeSet::new();
        for register in document.registers {
            if !matches!(register.width, 8 | 16 | 32 | 64) {
                return Err(format!(
                    "unsupported PAC register width {} for {}",
                    register.width, register.identity
                )
                .into());
            }
            if !identities.insert(register.identity.clone()) {
                return Err(
                    format!("duplicate PAC register identity {}", register.identity).into(),
                );
            }
            for field in &register.fields {
                let end = u16::from(field.bit_offset) + u16::from(field.bit_width);
                if field.bit_width == 0 || end > u16::from(register.width) {
                    return Err(
                        format!("PAC field {} has an invalid bit range", field.svd_name).into(),
                    );
                }
            }
            registers
                .entry((register.address, register.width))
                .or_default()
                .push(register);
        }
        if registers.is_empty() {
            return Err("PAC binding TOML has no registers".into());
        }
        Ok(Self {
            crate_name: document.crate_name,
            registers,
        })
    }

    pub fn register(&self, address: u32, width: u8, identity: &str) -> Result<&PacRegisterBinding> {
        let candidates = self.registers.get(&(address, width)).ok_or_else(|| {
            format!("PAC binding index has no {width}-bit register at {address:#010x}")
        })?;
        candidates
            .iter()
            .find(|binding| binding.identity == identity)
            .ok_or_else(|| {
                format!(
                    "PAC binding index has no identity {identity} at {address:#010x}; candidates: {}",
                    candidates
                        .iter()
                        .map(|binding| binding.identity.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
                .into()
            })
    }
}
