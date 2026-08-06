//! Parser for the project-generated PAC binding index.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use crate::Result;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PacAccess {
    ReadOnly,
    WriteOnly,
    ReadWrite,
    ReadWriteOnce,
    WriteOnce,
    Unspecified,
}

impl PacAccess {
    fn parse(value: &str, line: usize) -> Result<Self> {
        match value {
            "read-only" => Ok(Self::ReadOnly),
            "write-only" => Ok(Self::WriteOnly),
            "read-write" => Ok(Self::ReadWrite),
            "read-writeOnce" => Ok(Self::ReadWriteOnce),
            "writeOnce" => Ok(Self::WriteOnce),
            "unspecified" => Ok(Self::Unspecified),
            _ => Err(format!("unknown PAC access {value:?} at line {line}").into()),
        }
    }

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PacScopeBinding {
    pub method: String,
    pub index: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PacFieldBinding {
    pub svd_name: String,
    pub method: String,
    pub index: Option<u32>,
    pub bit_offset: u8,
    pub bit_width: u8,
    pub access: PacAccess,
}

#[derive(Clone, Debug, Eq, PartialEq)]
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

fn parse_u32(value: &str, kind: &str, line: usize) -> Result<u32> {
    let parsed = value
        .strip_prefix("0x")
        .map_or_else(|| value.parse(), |hex| u32::from_str_radix(hex, 16))
        .map_err(|_| format!("invalid PAC {kind} {value:?} at line {line}"))?;
    Ok(parsed)
}

fn parse_u8(value: &str, kind: &str, line: usize) -> Result<u8> {
    value
        .parse()
        .map_err(|_| format!("invalid PAC {kind} {value:?} at line {line}").into())
}

fn parse_optional_index(value: &str, line: usize) -> Result<Option<u32>> {
    if value == "-" {
        Ok(None)
    } else {
        Ok(Some(parse_u32(value, "array index", line)?))
    }
}

fn parse_scope(value: &str, line: usize) -> Result<Vec<PacScopeBinding>> {
    if value == "-" {
        return Ok(Vec::new());
    }
    value
        .split('.')
        .map(|item| {
            if let Some((method, index)) = item
                .strip_suffix(']')
                .and_then(|item| item.rsplit_once('['))
            {
                if method.is_empty() {
                    return Err(format!("empty PAC scope method at line {line}").into());
                }
                Ok(PacScopeBinding {
                    method: method.to_owned(),
                    index: Some(parse_u32(index, "scope index", line)?),
                })
            } else if item.is_empty() {
                Err(format!("empty PAC scope method at line {line}").into())
            } else {
                Ok(PacScopeBinding {
                    method: item.to_owned(),
                    index: None,
                })
            }
        })
        .collect()
}

fn optional_name(value: &str) -> Option<String> {
    (value != "-").then(|| value.to_owned())
}

impl PacBindingIndex {
    pub fn load(path: &Path) -> Result<Self> {
        Self::parse(&fs::read_to_string(path)?)
    }

    pub fn parse(input: &str) -> Result<Self> {
        let mut lines = input.lines().enumerate();
        let (_, header) = lines.next().ok_or("PAC binding index is empty")?;
        if header.trim() != "pac-binding-index 2" {
            return Err(format!(
                "unsupported PAC binding index header {:?}; expected pac-binding-index 2",
                header.trim()
            )
            .into());
        }
        let (crate_line, crate_directive) = lines
            .next()
            .map(|(line, value)| (line + 1, value.trim()))
            .ok_or("PAC binding index has no crate directive")?;
        let crate_name = crate_directive
            .strip_prefix("crate ")
            .filter(|name| !name.is_empty() && !name.contains(char::is_whitespace))
            .ok_or_else(|| format!("invalid PAC crate directive at line {crate_line}"))?
            .to_owned();

        let mut registers = BTreeMap::<(u32, u8), Vec<PacRegisterBinding>>::new();
        let mut identities = BTreeSet::new();
        for (line_index, raw_line) in lines {
            let line_number = line_index + 1;
            let line = raw_line.trim();
            if line.is_empty() {
                continue;
            }
            let words = line.split_whitespace().collect::<Vec<_>>();
            match words.first().copied() {
                Some("register") if words.len() == 12 => {
                    let address = parse_u32(words[1], "register address", line_number)?;
                    let width = parse_u8(words[2], "register width", line_number)?;
                    if !matches!(width, 8 | 16 | 32 | 64) {
                        return Err(format!(
                            "unsupported PAC register width {width} at line {line_number}"
                        )
                        .into());
                    }
                    let identity = words[4].to_owned();
                    if !identities.insert(identity.clone()) {
                        return Err(format!(
                            "duplicate PAC register identity {identity} at line {line_number}"
                        )
                        .into());
                    }
                    registers
                        .entry((address, width))
                        .or_default()
                        .push(PacRegisterBinding {
                            address,
                            width,
                            access: PacAccess::parse(words[3], line_number)?,
                            identity,
                            peripheral: words[5].to_owned(),
                            peripheral_type: words[6].to_owned(),
                            peripheral_module: words[7].to_owned(),
                            scope: parse_scope(words[8], line_number)?,
                            register_method: words[9].to_owned(),
                            register_index: parse_optional_index(words[10], line_number)?,
                            alternate_register: optional_name(words[11]),
                            fields: Vec::new(),
                        });
                }
                Some("field") if words.len() == 9 => {
                    let address = parse_u32(words[1], "field address", line_number)?;
                    let identity = words[2];
                    let field = PacFieldBinding {
                        svd_name: words[3].to_owned(),
                        method: words[4].to_owned(),
                        index: parse_optional_index(words[5], line_number)?,
                        bit_offset: parse_u8(words[6], "field offset", line_number)?,
                        bit_width: parse_u8(words[7], "field width", line_number)?,
                        access: PacAccess::parse(words[8], line_number)?,
                    };
                    let Some(register) = registers
                        .values_mut()
                        .flat_map(|bindings| bindings.iter_mut())
                        .find(|register| {
                            register.address == address && register.identity == identity
                        })
                    else {
                        return Err(format!(
                            "PAC field refers to missing register {identity} at line {line_number}"
                        )
                        .into());
                    };
                    let end = u16::from(field.bit_offset) + u16::from(field.bit_width);
                    if field.bit_width == 0 || end > u16::from(register.width) {
                        return Err(format!(
                            "PAC field {} has invalid bit range at line {line_number}",
                            field.svd_name
                        )
                        .into());
                    }
                    register.fields.push(field);
                }
                Some(kind) => {
                    return Err(format!(
                        "invalid PAC binding {kind:?} field count at line {line_number}"
                    )
                    .into());
                }
                None => unreachable!(),
            }
        }
        if registers.is_empty() {
            return Err("PAC binding index has no registers".into());
        }
        Ok(Self {
            crate_name,
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
