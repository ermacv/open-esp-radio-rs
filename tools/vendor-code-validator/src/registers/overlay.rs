//! Human-reviewed register and field semantics stored separately from facts.

use std::{
    collections::BTreeSet,
    fmt::Write as _,
    fs::{self, OpenOptions},
    io::Write as _,
    path::Path,
};

use toml_edit::{DocumentMut, Item};

use super::RegisterFacts;
use crate::Result;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DeviceOverlay {
    pub(crate) name: String,
    pub(crate) vendor: Option<String>,
    pub(crate) version: String,
    pub(crate) description: String,
    pub(crate) address_unit_bits: u8,
    pub(crate) width: u8,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PeripheralOverlay {
    pub(crate) range: String,
    pub(crate) name: String,
    pub(crate) description: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RegisterStatus {
    Reviewed,
    Ignored,
    Manual,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FieldOrigin {
    Manual,
    WritePattern,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FieldOverlay {
    pub(crate) name: String,
    pub(crate) lsb: u8,
    pub(crate) width: u8,
    pub(crate) description: Option<String>,
    pub(crate) access: Option<String>,
    pub(crate) modified_write_values: Option<String>,
    pub(crate) read_action: Option<String>,
    pub(crate) origin: FieldOrigin,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RegisterOverlay {
    pub(crate) address: u32,
    pub(crate) width: u8,
    pub(crate) status: RegisterStatus,
    pub(crate) name: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) access: Option<String>,
    pub(crate) reset_value: Option<u32>,
    pub(crate) reset_mask: Option<u32>,
    pub(crate) fields: Vec<FieldOverlay>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RegisterOverlayFile {
    pub(crate) device: DeviceOverlay,
    pub(crate) peripherals: Vec<PeripheralOverlay>,
    pub(crate) registers: Vec<RegisterOverlay>,
}

impl RegisterOverlayFile {
    pub(crate) fn load(path: &Path, facts: &RegisterFacts) -> Result<Self> {
        let input = fs::read_to_string(path)?;
        let document = input.parse::<DocumentMut>()?;
        if document.get("schema").and_then(Item::as_integer) != Some(1) {
            return Err(format!("{} requires schema = 1", path.display()).into());
        }
        let overlay = super::overlay_parse::parse(&document)?;
        overlay.validate(facts)?;
        Ok(overlay)
    }

    fn validate(&self, facts: &RegisterFacts) -> Result<()> {
        validate_identifier(&self.device.name, "device name")?;
        if self.device.address_unit_bits != 8 {
            return Err("register overlay currently requires address-unit-bits = 8".into());
        }
        if !matches!(self.device.width, 8 | 16 | 32 | 64) {
            return Err(format!("unsupported device width {}", self.device.width).into());
        }

        let fact_ranges = facts
            .ranges
            .iter()
            .map(|range| range.name.as_str())
            .collect::<BTreeSet<_>>();
        let mut peripheral_ranges = BTreeSet::new();
        let mut peripheral_names = BTreeSet::new();
        for peripheral in &self.peripherals {
            validate_identifier(&peripheral.name, "peripheral name")?;
            if !fact_ranges.contains(peripheral.range.as_str()) {
                return Err(format!(
                    "peripheral {} refers to unknown discovery range {:?}",
                    peripheral.name, peripheral.range
                )
                .into());
            }
            if !peripheral_ranges.insert(peripheral.range.as_str()) {
                return Err(format!(
                    "duplicate peripheral overlay for range {:?}",
                    peripheral.range
                )
                .into());
            }
            if !peripheral_names.insert(peripheral.name.as_str()) {
                return Err(format!("duplicate peripheral name {:?}", peripheral.name).into());
            }
        }

        let mut register_keys = BTreeSet::new();
        for register in &self.registers {
            let key = (register.address, register.width);
            if !register_keys.insert(key) {
                return Err(format!(
                    "duplicate register overlay at {:#010x}/{}",
                    register.address, register.width
                )
                .into());
            }
            if !matches!(register.width, 8 | 16 | 32) {
                return Err(format!(
                    "register overlay at {:#010x} has unsupported width {}",
                    register.address, register.width
                )
                .into());
            }
            if facts.range_for(register.address).is_none() {
                return Err(format!(
                    "register overlay at {:#010x}/{} is outside every discovery range",
                    register.address, register.width
                )
                .into());
            }
            let fact = facts
                .registers
                .iter()
                .find(|fact| (fact.address, fact.width) == key);
            match register.status {
                RegisterStatus::Reviewed | RegisterStatus::Ignored if fact.is_none() => {
                    return Err(format!(
                        "observed register overlay at {:#010x}/{} is stale; use status = \"manual\" for a non-discovered register",
                        register.address, register.width
                    )
                    .into());
                }
                RegisterStatus::Manual if fact.is_some() => {
                    return Err(format!(
                        "manual register overlay at {:#010x}/{} already has a discovery fact; use status = \"reviewed\"",
                        register.address, register.width
                    )
                    .into());
                }
                _ => {}
            }
            if register.status == RegisterStatus::Reviewed
                || register.status == RegisterStatus::Manual
            {
                let name = register.name.as_deref().ok_or_else(|| {
                    format!(
                        "register at {:#010x}/{} requires a name",
                        register.address, register.width
                    )
                })?;
                validate_identifier(name, "register name")?;
            }
            if register.status == RegisterStatus::Ignored && !register.fields.is_empty() {
                return Err(format!(
                    "ignored register at {:#010x}/{} cannot define fields",
                    register.address, register.width
                )
                .into());
            }
            validate_reset(register)?;
            validate_fields(register, fact)?;
        }
        Ok(())
    }
}

pub(crate) fn write_overlay_template(
    path: &Path,
    facts: &RegisterFacts,
    project_id: &str,
) -> Result<()> {
    if path.exists() {
        return Err(format!("refusing to overwrite existing overlay {}", path.display()).into());
    }
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    let mut output = String::new();
    output.push_str("# Human-reviewed names and semantics. Generated MMIO facts stay separate.\n");
    output.push_str("schema = 1\n");
    writeln!(
        output,
        "device-name = \"{}\"",
        identifier_from(project_id).to_ascii_uppercase()
    )
    .expect("writing to String cannot fail");
    output.push_str("version = \"0.1\"\n");
    output.push_str("description = \"Reviewed register map\"\n");
    output.push_str("address-unit-bits = 8\nwidth = 32\n");
    let mut names = BTreeSet::new();
    for range in &facts.ranges {
        let base = identifier_from(&range.name).to_ascii_uppercase();
        let mut name = base.clone();
        let mut suffix = 2usize;
        while !names.insert(name.clone()) {
            name = format!("{base}_{suffix}");
            suffix += 1;
        }
        output.push_str("\n[[peripherals]]\n");
        writeln!(output, "range = \"{}\"", toml_string(&range.name))
            .expect("writing to String cannot fail");
        writeln!(output, "name = \"{name}\"").expect("writing to String cannot fail");
        writeln!(
            output,
            "description = \"MMIO range {:#010x}..{:#010x}\"",
            range.start, range.end
        )
        .expect("writing to String cannot fail");
    }
    output.push_str(
        "\n# Add reviewed entries as shown below. `manual` permits an address absent\n\
         # from the facts; `ignored` suppresses a false positive.\n\
         # [[registers]]\n\
         # address = 0x20100010\n\
         # width = 32\n\
         # status = \"reviewed\"\n\
         # name = \"CONTROL\"\n\
         # description = \"Reviewed purpose\"\n\
         # access = \"read-write\"\n\
         #\n\
         # [[registers.fields]]\n\
         # name = \"ENABLE\"\n\
         # lsb = 0\n\
         # width = 1\n\
         # origin = \"write-pattern\" # or \"manual\"\n",
    );
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(output.as_bytes())?;
    Ok(())
}

fn validate_reset(register: &RegisterOverlay) -> Result<()> {
    if register.reset_value.is_some() != register.reset_mask.is_some() {
        return Err(format!(
            "register at {:#010x}/{} must define reset-value and reset-mask together",
            register.address, register.width
        )
        .into());
    }
    let width_mask = width_mask(register.width);
    if register
        .reset_value
        .is_some_and(|value| value & !width_mask != 0)
        || register
            .reset_mask
            .is_some_and(|value| value & !width_mask != 0)
    {
        return Err(format!(
            "register reset metadata at {:#010x}/{} exceeds its width",
            register.address, register.width
        )
        .into());
    }
    Ok(())
}

fn validate_fields(register: &RegisterOverlay, fact: Option<&super::RegisterFact>) -> Result<()> {
    let mut names = BTreeSet::new();
    let mut occupied = 0_u32;
    for field in &register.fields {
        validate_identifier(&field.name, "field name")?;
        if !names.insert(field.name.as_str()) {
            return Err(format!(
                "duplicate field name {:?} at {:#010x}/{}",
                field.name, register.address, register.width
            )
            .into());
        }
        let end = field
            .lsb
            .checked_add(field.width)
            .ok_or("field range overflows")?;
        if field.width == 0 || end > register.width {
            return Err(format!(
                "field {} at {:#010x}/{} exceeds the register width",
                field.name, register.address, register.width
            )
            .into());
        }
        let mask = width_mask(field.width) << field.lsb;
        if occupied & mask != 0 {
            return Err(format!(
                "field {} overlaps another field at {:#010x}/{}",
                field.name, register.address, register.width
            )
            .into());
        }
        occupied |= mask;
        if field.origin == FieldOrigin::WritePattern
            && !fact.is_some_and(|fact| {
                fact.candidate_masks
                    .iter()
                    .any(|candidate| candidate & mask == mask)
            })
        {
            return Err(format!(
                "field {} at {:#010x}/{} claims write-pattern origin but no observed pattern covers its bits",
                field.name, register.address, register.width
            )
            .into());
        }
    }
    Ok(())
}

pub(super) fn validate_access(value: String, context: &str) -> Result<String> {
    if matches!(
        value.as_str(),
        "read-only" | "write-only" | "read-write" | "writeOnce" | "read-writeOnce"
    ) {
        Ok(value)
    } else {
        Err(format!("invalid CMSIS-SVD access {value:?} in {context}").into())
    }
}

pub(super) fn validate_modified_write_values(value: String, context: &str) -> Result<String> {
    if matches!(
        value.as_str(),
        "oneToClear"
            | "oneToSet"
            | "oneToToggle"
            | "zeroToClear"
            | "zeroToSet"
            | "zeroToToggle"
            | "clear"
            | "set"
            | "modify"
    ) {
        Ok(value)
    } else {
        Err(format!("invalid modified-write-values {value:?} in {context}").into())
    }
}

pub(super) fn validate_read_action(value: String, context: &str) -> Result<String> {
    if matches!(
        value.as_str(),
        "clear" | "set" | "modify" | "modifyExternal"
    ) {
        Ok(value)
    } else {
        Err(format!("invalid read-action {value:?} in {context}").into())
    }
}

pub(super) fn identifier_from(value: &str) -> String {
    let mut output = String::new();
    for character in value.chars() {
        let character = if character.is_ascii_alphanumeric() || character == '_' {
            character
        } else {
            '_'
        };
        if output.is_empty() && character.is_ascii_digit() {
            output.push('_');
        }
        output.push(character);
    }
    if output.is_empty() {
        "UNNAMED".to_owned()
    } else {
        output
    }
}

pub(super) fn validate_identifier(value: &str, context: &str) -> Result<()> {
    let mut characters = value.chars();
    let valid_start = characters
        .next()
        .is_some_and(|character| character.is_ascii_alphabetic() || character == '_');
    if !valid_start
        || !characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        return Err(format!("invalid CMSIS-SVD {context} {value:?}").into());
    }
    Ok(())
}

fn width_mask(width: u8) -> u32 {
    if width == 32 {
        u32::MAX
    } else {
        (1_u32 << width) - 1
    }
}

fn toml_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
