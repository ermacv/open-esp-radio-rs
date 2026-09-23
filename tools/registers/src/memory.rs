use crate::Result;
use open_esp_radio_register_model::{RegisterEvidenceSet, RegisterModel};
use serde::Deserialize;
use std::{collections::BTreeSet, path::Path};
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct Memory {
    schema: u32,
    default_address_space: String,
    address_spaces: Vec<Space>,
    regions: Vec<Region>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct Space {
    id: String,
    address_width: u8,
    endianness: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct Region {
    name: String,
    address_space: String,
    kind: String,
    start: u64,
    end_exclusive: u64,
    permissions: String,
    #[serde(default, rename = "volatile")]
    _volatile: bool,
    alias_of: Option<String>,
}
impl Memory {
    pub fn load(path: &Path) -> Result<Self> {
        Ok(toml_edit::de::from_str(&std::fs::read_to_string(path)?)?)
    }
    pub fn validate(
        &self,
        model: &RegisterModel,
        owned: &[String],
        evidence: &RegisterEvidenceSet,
    ) -> Result<()> {
        if self.schema != 1 || self.default_address_space != model.address_space() {
            return Err("invalid memory map schema/address space".into());
        }
        let mut ids = BTreeSet::new();
        for space in &self.address_spaces {
            if !valid_id(&space.id)
                || !ids.insert(&space.id)
                || !(1..=64).contains(&space.address_width)
                || !matches!(space.endianness.as_str(), "little" | "big")
            {
                return Err("invalid memory address space".into());
            }
        }
        if !ids.contains(&self.default_address_space) {
            return Err("default memory address space is absent".into());
        }
        let mut names = BTreeSet::new();
        for region in &self.regions {
            let space = self
                .address_spaces
                .iter()
                .find(|s| s.id == region.address_space)
                .ok_or("unknown region address space")?;
            if !valid_id(&region.name)
                || !names.insert(&region.name)
                || region.start >= region.end_exclusive
                || (space.address_width < 64
                    && region.end_exclusive > (1u64 << space.address_width))
                || !matches!(
                    region.kind.as_str(),
                    "code" | "rodata" | "ram" | "mmio" | "device" | "unknown"
                )
                || ['r', 'w', 'x']
                    .into_iter()
                    .any(|p| region.permissions.matches(p).count() > 1)
                || region.permissions.chars().any(|c| !"rwx".contains(c))
            {
                return Err(format!("invalid memory region {}", region.name).into());
            }
            if let Some(alias) = &region.alias_of
                && (alias == &region.name
                    || !self.regions.iter().any(|r| {
                        &r.name == alias
                            && r.address_space == region.address_space
                            && r.start == region.start
                            && r.end_exclusive == region.end_exclusive
                    }))
            {
                return Err("invalid memory alias".into());
            }
        }
        for (index, left) in self.regions.iter().enumerate() {
            for right in &self.regions[index + 1..] {
                if left.address_space == right.address_space
                    && left.start < right.end_exclusive
                    && right.start < left.end_exclusive
                    && left.alias_of.as_deref() != Some(right.name.as_str())
                    && right.alias_of.as_deref() != Some(left.name.as_str())
                {
                    return Err("memory regions overlap without explicit alias".into());
                }
            }
        }
        let ranges: Vec<_> = self
            .regions
            .iter()
            .filter(|r| {
                r.kind == "mmio" && r.address_space == model.address_space() && r.alias_of.is_none()
            })
            .collect();
        let mut selected = BTreeSet::new();
        for name in owned {
            if !selected.insert(name) || !ranges.iter().any(|r| &r.name == name) {
                return Err(format!("invalid owned MMIO range {name}").into());
            }
        }
        for ((address, width), name) in model.register_identities()? {
            let end = address
                .checked_add(u64::from(width).div_ceil(8))
                .ok_or("register range overflow")?;
            if !ranges
                .iter()
                .any(|r| selected.contains(&r.name) && r.start <= address && end <= r.end_exclusive)
            {
                return Err(format!("register {name} is outside owned MMIO ranges").into());
            }
        }
        for range in &evidence.ranges {
            if !ranges
                .iter()
                .any(|r| r.start <= range.start && range.end_exclusive <= r.end_exclusive)
            {
                return Err(format!("evidence range {} is outside MMIO map", range.name).into());
            }
        }
        Ok(())
    }
}

fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}
