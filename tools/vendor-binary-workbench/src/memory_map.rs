//! Project-owned address spaces and memory regions.

use std::{collections::BTreeMap, fs, path::Path};

use toml_edit::{ArrayOfTables, DocumentMut, Item, Table};

use crate::{Result, Window, error::WorkbenchError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MemoryRegionKind {
    Code,
    ReadOnlyData,
    Ram,
    Mmio,
    Device,
    Unknown,
}

impl MemoryRegionKind {
    fn parse(value: &str, context: &str) -> Result<Self> {
        match value {
            "code" => Ok(Self::Code),
            "rodata" => Ok(Self::ReadOnlyData),
            "ram" => Ok(Self::Ram),
            "mmio" => Ok(Self::Mmio),
            "device" => Ok(Self::Device),
            "unknown" => Ok(Self::Unknown),
            _ => Err(format!("unsupported memory region kind {value:?} in {context}").into()),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AddressSpace {
    pub(crate) id: String,
    pub(crate) address_width: u8,
    pub(crate) endianness: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MemoryRegion {
    pub(crate) name: String,
    pub(crate) address_space: String,
    pub(crate) kind: MemoryRegionKind,
    pub(crate) start: u64,
    pub(crate) end: u64,
    pub(crate) permissions: String,
    pub(crate) volatile: bool,
    pub(crate) alias_of: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MemoryMap {
    pub(crate) default_address_space: String,
    pub(crate) address_spaces: Vec<AddressSpace>,
    pub(crate) regions: Vec<MemoryRegion>,
}

impl MemoryMap {
    #[tracing::instrument(name = "load_memory_map", fields(path = %path.display()))]
    pub(crate) fn load(path: &Path) -> Result<Self> {
        let input = fs::read_to_string(path)?;
        let document = input.parse::<DocumentMut>().map_err(|error| {
            WorkbenchError::manifest_source("memory map", path, &input, &error, error.span())
        })?;
        Self::parse(document, path)
            .map_err(|error| WorkbenchError::manifest("memory map", path, error))
    }

    fn parse(document: DocumentMut, path: &Path) -> Result<Self> {
        expect_schema(&document, path)?;

        let default_address_space =
            required_string(document.as_item(), "default-address-space", "memory map")?;
        let address_spaces =
            parse_address_spaces(required_tables(&document, "address-spaces", "memory map")?)?;
        let regions = parse_regions(required_tables(&document, "regions", "memory map")?)?;
        let map = Self {
            default_address_space,
            address_spaces,
            regions,
        };
        map.validate()?;
        Ok(map)
    }

    pub(crate) fn mmio_windows(&self) -> Result<Vec<Window>> {
        self.mmio_regions()
            .map(|region| {
                Ok(Window {
                    start: u32::try_from(region.start).map_err(|_| {
                        format!(
                            "MMIO region {} start does not fit the current 32-bit backend",
                            region.name
                        )
                    })?,
                    end: u32::try_from(region.end).map_err(|_| {
                        format!(
                            "MMIO region {} end does not fit the current 32-bit backend",
                            region.name
                        )
                    })?,
                })
            })
            .collect()
    }

    pub(crate) fn mmio_ranges(&self) -> Result<Vec<(String, u32, u32)>> {
        self.mmio_regions()
            .map(|region| {
                Ok((
                    region.name.clone(),
                    u32::try_from(region.start).map_err(|_| {
                        format!(
                            "MMIO region {} start does not fit the current 32-bit backend",
                            region.name
                        )
                    })?,
                    u32::try_from(region.end).map_err(|_| {
                        format!(
                            "MMIO region {} end does not fit the current 32-bit backend",
                            region.name
                        )
                    })?,
                ))
            })
            .collect()
    }

    fn mmio_regions(&self) -> impl Iterator<Item = &MemoryRegion> {
        self.regions.iter().filter(|region| {
            region.address_space == self.default_address_space
                && region.kind == MemoryRegionKind::Mmio
        })
    }

    fn validate(&self) -> Result<()> {
        validate_id(&self.default_address_space, "default address space")?;
        let mut spaces = BTreeMap::new();
        for space in &self.address_spaces {
            validate_id(&space.id, "address-space")?;
            if !(1..=64).contains(&space.address_width) {
                return Err(format!(
                    "address space {} has unsupported width {}",
                    space.id, space.address_width
                )
                .into());
            }
            if !matches!(space.endianness.as_str(), "little" | "big") {
                return Err(format!(
                    "address space {} has unsupported endianness {:?}",
                    space.id, space.endianness
                )
                .into());
            }
            if spaces.insert(space.id.as_str(), space).is_some() {
                return Err(format!("duplicate address space {:?}", space.id).into());
            }
        }
        if !spaces.contains_key(self.default_address_space.as_str()) {
            return Err(format!(
                "default address space {:?} is not declared",
                self.default_address_space
            )
            .into());
        }

        let mut regions = BTreeMap::new();
        for region in &self.regions {
            validate_id(&region.name, "memory region")?;
            let Some(space) = spaces.get(region.address_space.as_str()) else {
                return Err(format!(
                    "memory region {} uses undeclared address space {:?}",
                    region.name, region.address_space
                )
                .into());
            };
            if region.start >= region.end {
                return Err(format!("memory region {} is empty or reversed", region.name).into());
            }
            if space.address_width < 64 && region.end > (1_u64 << space.address_width) {
                return Err(format!(
                    "memory region {} exceeds its {}-bit address space",
                    region.name, space.address_width
                )
                .into());
            }
            validate_permissions(&region.permissions, &region.name)?;
            let key = (region.address_space.as_str(), region.name.as_str());
            if regions.insert(key, region).is_some() {
                return Err(format!(
                    "duplicate memory region {} in address space {}",
                    region.name, region.address_space
                )
                .into());
            }
        }

        for region in &self.regions {
            if let Some(alias) = &region.alias_of {
                validate_id(alias, "memory alias target")?;
                let Some(target) = regions.get(&(region.address_space.as_str(), alias.as_str()))
                else {
                    return Err(format!(
                        "memory region {} aliases unknown region {alias:?}",
                        region.name
                    )
                    .into());
                };
                if region.start != target.start || region.end != target.end {
                    return Err(format!(
                        "memory alias {} must have the same range as {}",
                        region.name, target.name
                    )
                    .into());
                }
            }
        }

        for (index, left) in self.regions.iter().enumerate() {
            for right in self.regions.iter().skip(index + 1) {
                if left.address_space != right.address_space
                    || left.end <= right.start
                    || right.end <= left.start
                {
                    continue;
                }
                let declared_alias = left.alias_of.as_deref() == Some(right.name.as_str())
                    || right.alias_of.as_deref() == Some(left.name.as_str());
                if !declared_alias {
                    return Err(format!(
                        "memory regions {} and {} overlap without an explicit alias",
                        left.name, right.name
                    )
                    .into());
                }
            }
        }
        Ok(())
    }
}

fn parse_address_spaces(tables: &ArrayOfTables) -> Result<Vec<AddressSpace>> {
    tables
        .iter()
        .enumerate()
        .map(|(index, table)| {
            let context = format!("address-spaces[{index}]");
            Ok(AddressSpace {
                id: required_table_string(table, "id", &context)?,
                address_width: required_table_integer(table, "address-width", &context)?
                    .try_into()
                    .map_err(|_| format!("invalid address-width in {context}"))?,
                endianness: required_table_string(table, "endianness", &context)?,
            })
        })
        .collect()
}

fn parse_regions(tables: &ArrayOfTables) -> Result<Vec<MemoryRegion>> {
    tables
        .iter()
        .enumerate()
        .map(|(index, table)| {
            let context = format!("regions[{index}]");
            let kind = MemoryRegionKind::parse(
                &required_table_string(table, "kind", &context)?,
                &context,
            )?;
            Ok(MemoryRegion {
                name: required_table_string(table, "name", &context)?,
                address_space: required_table_string(table, "address-space", &context)?,
                kind,
                start: required_table_nonnegative_integer(table, "start", &context)?,
                end: required_table_nonnegative_integer(table, "end-exclusive", &context)?,
                permissions: optional_table_string(table, "permissions").unwrap_or_default(),
                volatile: optional_table_bool(table, "volatile")
                    .unwrap_or(kind == MemoryRegionKind::Mmio),
                alias_of: optional_table_string(table, "alias-of"),
            })
        })
        .collect()
}

fn expect_schema(document: &DocumentMut, path: &Path) -> Result<()> {
    if document.get("schema").and_then(Item::as_integer) != Some(1) {
        return Err(format!("{} requires schema = 1", path.display()).into());
    }
    Ok(())
}

fn required_tables<'a>(
    document: &'a DocumentMut,
    key: &str,
    context: &str,
) -> Result<&'a ArrayOfTables> {
    document
        .get(key)
        .and_then(Item::as_array_of_tables)
        .ok_or_else(|| format!("{context} requires [[{key}]]").into())
}

fn required_string(item: &Item, key: &str, context: &str) -> Result<String> {
    item.get(key)
        .and_then(Item::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("{context} requires string {key:?}").into())
}

fn required_table_string(table: &Table, key: &str, context: &str) -> Result<String> {
    table
        .get(key)
        .and_then(Item::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("{context} requires string {key:?}").into())
}

fn optional_table_string(table: &Table, key: &str) -> Option<String> {
    table.get(key).and_then(Item::as_str).map(str::to_owned)
}

fn optional_table_bool(table: &Table, key: &str) -> Option<bool> {
    table.get(key).and_then(Item::as_bool)
}

fn required_table_integer(table: &Table, key: &str, context: &str) -> Result<i64> {
    table
        .get(key)
        .and_then(Item::as_integer)
        .ok_or_else(|| format!("{context} requires integer {key:?}").into())
}

fn required_table_nonnegative_integer(table: &Table, key: &str, context: &str) -> Result<u64> {
    required_table_integer(table, key, context)?
        .try_into()
        .map_err(|_| format!("{context} requires non-negative integer {key:?}").into())
}

fn validate_id(value: &str, context: &str) -> Result<()> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(format!("invalid {context} id {value:?}").into());
    }
    Ok(())
}

fn validate_permissions(value: &str, region: &str) -> Result<()> {
    let canonical = value
        .bytes()
        .filter(|byte| matches!(byte, b'r' | b'w' | b'x'));
    if canonical.count() != value.len()
        || value.matches('r').count() > 1
        || value.matches('w').count() > 1
        || value.matches('x').count() > 1
    {
        return Err(format!(
            "memory region {region} has invalid permissions {value:?}; use each of r, w and x at most once"
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_map(name: &str, contents: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "open-radio-workbench-memory-{}-{name}.toml",
            std::process::id(),
        ));
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn loads_mmio_regions_as_backend_windows() {
        let path = write_map(
            "valid",
            r#"
schema = 1
default-address-space = "cpu"

[[address-spaces]]
id = "cpu"
address-width = 32
endianness = "little"

[[regions]]
name = "radio"
address-space = "cpu"
kind = "mmio"
start = 0x20100000
end-exclusive = 0x20200000
permissions = "rw"
"#,
        );
        let map = MemoryMap::load(&path).unwrap();
        std::fs::remove_file(path).unwrap();

        assert_eq!(
            map.mmio_windows().unwrap(),
            [Window {
                start: 0x2010_0000,
                end: 0x2020_0000,
            }]
        );
        assert!(map.regions[0].volatile);
    }

    #[test]
    fn rejects_implicit_overlaps() {
        let path = write_map(
            "overlap",
            r#"
schema = 1
default-address-space = "cpu"
[[address-spaces]]
id = "cpu"
address-width = 32
endianness = "little"
[[regions]]
name = "a"
address-space = "cpu"
kind = "ram"
start = 0x1000
end-exclusive = 0x2000
[[regions]]
name = "b"
address-space = "cpu"
kind = "ram"
start = 0x1800
end-exclusive = 0x2800
"#,
        );
        let error = MemoryMap::load(&path).unwrap_err();
        std::fs::remove_file(path).unwrap();
        assert!(error.to_string().contains("overlap"));
    }

    #[test]
    fn malformed_file_reports_its_manifest_path() {
        let path = write_map("malformed", "schema = [\n");
        let error = MemoryMap::load(&path).unwrap_err();
        std::fs::remove_file(&path).unwrap();

        assert!(matches!(
            error,
            WorkbenchError::ManifestSource {
                kind: "memory map",
                path: reported,
                ..
            } if reported == path
        ));
    }
}
