//! Project-owned address spaces and memory regions.

use std::{collections::BTreeMap, fs, path::Path};

use toml_edit::{ArrayOfTables, Item, Table};

use crate::{
    MmioRegion, Result,
    error::{BlobrayError, ManifestContext},
};

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
    fn parse(value: &str, context: &str) -> std::result::Result<Self, String> {
        match value {
            "code" => Ok(Self::Code),
            "rodata" => Ok(Self::ReadOnlyData),
            "ram" => Ok(Self::Ram),
            "mmio" => Ok(Self::Mmio),
            "device" => Ok(Self::Device),
            "unknown" => Ok(Self::Unknown),
            _ => Err(format!(
                "unsupported memory region kind {value:?} in {context}"
            )),
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
        let document = toml_edit::Document::parse(input.as_str()).map_err(|error| {
            BlobrayError::manifest_source("memory map", path, &input, &error, error.span())
        })?;
        let source = ManifestContext::new("memory map", path, &input);
        Self::parse(&document, source)
    }

    fn parse(document: &Table, source: ManifestContext<'_>) -> Result<Self> {
        reject_unknown_keys(
            document,
            &[
                "schema",
                "default-address-space",
                "address-spaces",
                "regions",
            ],
            "memory map",
            source,
        )?;
        expect_schema(document, source)?;

        let default_address_space =
            required_string(document, "default-address-space", "memory map", source)?;
        let address_space_tables =
            required_tables(document, "address-spaces", "memory map", source)?;
        let region_tables = required_tables(document, "regions", "memory map", source)?;
        let address_spaces = parse_address_spaces(address_space_tables, source)?;
        let regions = parse_regions(region_tables, source)?;
        let map = Self {
            default_address_space,
            address_spaces,
            regions,
        };
        map.validate(document, address_space_tables, region_tables, source)?;
        Ok(map)
    }

    pub(crate) fn resolved_mmio_regions(&self) -> Result<Vec<MmioRegion>> {
        self.mmio_regions()
            .map(|region| {
                Ok(MmioRegion {
                    name: region.name.clone(),
                    start: u32::try_from(region.start)
                        .map_err(|_| {
                            format!(
                                "MMIO region {} start does not fit the current 32-bit backend",
                                region.name
                            )
                        })
                        .map_err(crate::Error::invalid)?,
                    end: u32::try_from(region.end)
                        .map_err(|_| {
                            format!(
                                "MMIO region {} end does not fit the current 32-bit backend",
                                region.name
                            )
                        })
                        .map_err(crate::Error::invalid)?,
                    readable: region.permissions.contains('r'),
                    writable: region.permissions.contains('w'),
                })
            })
            .collect()
    }

    pub(crate) fn mmio_ranges(&self) -> Result<Vec<(String, u32, u32)>> {
        self.mmio_regions()
            .map(|region| {
                Ok((
                    region.name.clone(),
                    u32::try_from(region.start)
                        .map_err(|_| {
                            format!(
                                "MMIO region {} start does not fit the current 32-bit backend",
                                region.name
                            )
                        })
                        .map_err(crate::Error::invalid)?,
                    u32::try_from(region.end)
                        .map_err(|_| {
                            format!(
                                "MMIO region {} end does not fit the current 32-bit backend",
                                region.name
                            )
                        })
                        .map_err(crate::Error::invalid)?,
                ))
            })
            .collect()
    }

    fn mmio_regions(&self) -> impl Iterator<Item = &MemoryRegion> {
        self.regions.iter().filter(|region| {
            region.address_space == self.default_address_space
                && region.kind == MemoryRegionKind::Mmio
                && region.alias_of.is_none()
        })
    }

    fn validate(
        &self,
        document: &Table,
        address_space_tables: &ArrayOfTables,
        region_tables: &ArrayOfTables,
        source: ManifestContext<'_>,
    ) -> Result<()> {
        validate_id(&self.default_address_space, "default address space")
            .map_err(|message| source.table_key(document, "default-address-space", message))?;
        let mut spaces = BTreeMap::new();
        for (index, space) in self.address_spaces.iter().enumerate() {
            let table = address_space_tables
                .get(index)
                .expect("decoded address space has a source table");
            validate_id(&space.id, "address-space")
                .map_err(|message| source.table_key(table, "id", message))?;
            if !(1..=64).contains(&space.address_width) {
                return Err(source.table_key(
                    table,
                    "address-width",
                    format!(
                        "address space {} has unsupported width {}",
                        space.id, space.address_width
                    ),
                ));
            }
            if !matches!(space.endianness.as_str(), "little" | "big") {
                return Err(source.table_key(
                    table,
                    "endianness",
                    format!(
                        "address space {} has unsupported endianness {:?}",
                        space.id, space.endianness
                    ),
                ));
            }
            if spaces.insert(space.id.as_str(), space).is_some() {
                return Err(source.table_key(
                    table,
                    "id",
                    format!("duplicate address space {:?}", space.id),
                ));
            }
        }
        if !spaces.contains_key(self.default_address_space.as_str()) {
            return Err(source.table_key(
                document,
                "default-address-space",
                format!(
                    "default address space {:?} is not declared",
                    self.default_address_space
                ),
            ));
        }

        let mut regions = BTreeMap::new();
        for (index, region) in self.regions.iter().enumerate() {
            let table = region_tables
                .get(index)
                .expect("decoded memory region has a source table");
            validate_id(&region.name, "memory region")
                .map_err(|message| source.table_key(table, "name", message))?;
            let Some(space) = spaces.get(region.address_space.as_str()) else {
                return Err(source.table_key(
                    table,
                    "address-space",
                    format!(
                        "memory region {} uses undeclared address space {:?}",
                        region.name, region.address_space
                    ),
                ));
            };
            if region.start >= region.end {
                return Err(source.table_key(
                    table,
                    "end-exclusive",
                    format!("memory region {} is empty or reversed", region.name),
                ));
            }
            if space.address_width < 64 && region.end > (1_u64 << space.address_width) {
                return Err(source.table_key(
                    table,
                    "end-exclusive",
                    format!(
                        "memory region {} exceeds its {}-bit address space",
                        region.name, space.address_width
                    ),
                ));
            }
            validate_permissions(&region.permissions, &region.name)
                .map_err(|message| source.table_key(table, "permissions", message))?;
            let key = (region.address_space.as_str(), region.name.as_str());
            if regions.insert(key, region).is_some() {
                return Err(source.table_key(
                    table,
                    "name",
                    format!(
                        "duplicate memory region {} in address space {}",
                        region.name, region.address_space
                    ),
                ));
            }
        }

        for (index, region) in self.regions.iter().enumerate() {
            let table = region_tables
                .get(index)
                .expect("decoded memory region has a source table");
            if let Some(alias) = &region.alias_of {
                validate_id(alias, "memory alias target")
                    .map_err(|message| source.table_key(table, "alias-of", message))?;
                let Some(target) = regions.get(&(region.address_space.as_str(), alias.as_str()))
                else {
                    return Err(source.table_key(
                        table,
                        "alias-of",
                        format!(
                            "memory region {} aliases unknown region {alias:?}",
                            region.name
                        ),
                    ));
                };
                if region.start != target.start || region.end != target.end {
                    return Err(source.table_key(
                        table,
                        "alias-of",
                        format!(
                            "memory alias {} must have the same range as {}",
                            region.name, target.name
                        ),
                    ));
                }
            }
        }

        for (index, left) in self.regions.iter().enumerate() {
            for (right_index, right) in self.regions.iter().enumerate().skip(index + 1) {
                if left.address_space != right.address_space
                    || left.end <= right.start
                    || right.end <= left.start
                {
                    continue;
                }
                let declared_alias = left.alias_of.as_deref() == Some(right.name.as_str())
                    || right.alias_of.as_deref() == Some(left.name.as_str());
                if !declared_alias {
                    return Err(source.table_key(
                        region_tables
                            .get(right_index)
                            .expect("decoded memory region has a source table"),
                        "start",
                        format!(
                            "memory regions {} and {} overlap without an explicit alias",
                            left.name, right.name
                        ),
                    ));
                }
            }
        }
        Ok(())
    }
}

fn parse_address_spaces(
    tables: &ArrayOfTables,
    source: ManifestContext<'_>,
) -> Result<Vec<AddressSpace>> {
    tables
        .iter()
        .enumerate()
        .map(|(index, table)| {
            let context = format!("address-spaces[{index}]");
            reject_unknown_keys(
                table,
                &["id", "address-width", "endianness"],
                &context,
                source,
            )?;
            Ok(AddressSpace {
                id: required_table_string(table, "id", &context, source)?,
                address_width: required_table_integer(table, "address-width", &context, source)?
                    .try_into()
                    .map_err(|_| {
                        source.table_key(
                            table,
                            "address-width",
                            format!("invalid address-width in {context}"),
                        )
                    })?,
                endianness: required_table_string(table, "endianness", &context, source)?,
            })
        })
        .collect()
}

fn parse_regions(tables: &ArrayOfTables, source: ManifestContext<'_>) -> Result<Vec<MemoryRegion>> {
    tables
        .iter()
        .enumerate()
        .map(|(index, table)| {
            let context = format!("regions[{index}]");
            reject_unknown_keys(
                table,
                &[
                    "name",
                    "address-space",
                    "kind",
                    "start",
                    "end-exclusive",
                    "permissions",
                    "volatile",
                    "alias-of",
                ],
                &context,
                source,
            )?;
            let kind_value = required_table_string(table, "kind", &context, source)?;
            let kind = MemoryRegionKind::parse(&kind_value, &context)
                .map_err(|message| source.table_key(table, "kind", message))?;
            Ok(MemoryRegion {
                name: required_table_string(table, "name", &context, source)?,
                address_space: required_table_string(table, "address-space", &context, source)?,
                kind,
                start: required_table_nonnegative_integer(table, "start", &context, source)?,
                end: required_table_nonnegative_integer(table, "end-exclusive", &context, source)?,
                permissions: optional_table_string(table, "permissions", &context, source)?
                    .unwrap_or_default(),
                volatile: optional_table_bool(table, "volatile", &context, source)?
                    .unwrap_or(kind == MemoryRegionKind::Mmio),
                alias_of: optional_table_string(table, "alias-of", &context, source)?,
            })
        })
        .collect()
}

fn expect_schema(document: &Table, source: ManifestContext<'_>) -> Result<()> {
    if document.get("schema").and_then(Item::as_integer) != Some(1) {
        return Err(source.item(document.get("schema"), "memory map requires schema = 1"));
    }
    Ok(())
}

fn required_tables<'a>(
    document: &'a Table,
    key: &str,
    context: &str,
    source: ManifestContext<'_>,
) -> Result<&'a ArrayOfTables> {
    document
        .get(key)
        .and_then(Item::as_array_of_tables)
        .ok_or_else(|| source.table_key(document, key, format!("{context} requires [[{key}]]")))
}

fn required_string(
    table: &Table,
    key: &str,
    context: &str,
    source: ManifestContext<'_>,
) -> Result<String> {
    table
        .get(key)
        .and_then(Item::as_str)
        .map(str::to_owned)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            source.table_key(
                table,
                key,
                format!("{context} requires non-empty string {key:?}"),
            )
        })
}

fn required_table_string(
    table: &Table,
    key: &str,
    context: &str,
    source: ManifestContext<'_>,
) -> Result<String> {
    required_string(table, key, context, source)
}

fn optional_table_string(
    table: &Table,
    key: &str,
    context: &str,
    source: ManifestContext<'_>,
) -> Result<Option<String>> {
    table
        .get(key)
        .map(|item| {
            item.as_str()
                .map(str::to_owned)
                .ok_or_else(|| source.item(Some(item), format!("{context}.{key} must be a string")))
        })
        .transpose()
}

fn optional_table_bool(
    table: &Table,
    key: &str,
    context: &str,
    source: ManifestContext<'_>,
) -> Result<Option<bool>> {
    table
        .get(key)
        .map(|item| {
            item.as_bool().ok_or_else(|| {
                source.item(Some(item), format!("{context}.{key} must be a boolean"))
            })
        })
        .transpose()
}

fn required_table_integer(
    table: &Table,
    key: &str,
    context: &str,
    source: ManifestContext<'_>,
) -> Result<i64> {
    table
        .get(key)
        .and_then(Item::as_integer)
        .ok_or_else(|| source.table_key(table, key, format!("{context} requires integer {key:?}")))
}

fn required_table_nonnegative_integer(
    table: &Table,
    key: &str,
    context: &str,
    source: ManifestContext<'_>,
) -> Result<u64> {
    required_table_integer(table, key, context, source)?
        .try_into()
        .map_err(|_| {
            source.table_key(
                table,
                key,
                format!("{context} requires non-negative integer {key:?}"),
            )
        })
}

fn reject_unknown_keys(
    table: &Table,
    allowed: &[&str],
    context: &str,
    source: ManifestContext<'_>,
) -> Result<()> {
    for (key, item) in table.iter() {
        if !allowed.contains(&key) {
            return Err(source.item(Some(item), format!("unknown key {key:?} in {context}")));
        }
    }
    Ok(())
}

fn validate_id(value: &str, context: &str) -> std::result::Result<(), String> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(format!("invalid {context} id {value:?}"));
    }
    Ok(())
}

fn validate_permissions(value: &str, region: &str) -> std::result::Result<(), String> {
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
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_map(name: &str, contents: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "open-radio-blobray-memory-{}-{name}.toml",
            std::process::id(),
        ));
        std::fs::write(&path, contents).unwrap();
        path
    }

    fn invalid_map_span(input: &str, name: &str) -> (usize, usize, String) {
        let path = write_map(name, input);
        let error = MemoryMap::load(&path).unwrap_err();
        std::fs::remove_file(path).unwrap();
        let message = error.to_string();
        let BlobrayError::ManifestSource {
            kind: "memory map",
            span,
            ..
        } = error
        else {
            panic!("expected source-aware memory map error, got {message}")
        };
        (span.offset(), span.len(), message)
    }

    #[test]
    fn loads_mmio_regions_with_backend_permissions() {
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
            map.resolved_mmio_regions().unwrap(),
            [MmioRegion {
                name: "radio".to_owned(),
                start: 0x2010_0000,
                end: 0x2020_0000,
                readable: true,
                writable: true,
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
            BlobrayError::ManifestSource {
                kind: "memory map",
                path: reported,
                ..
            } if reported == path
        ));
    }

    #[test]
    fn semantic_map_errors_retain_the_exact_physical_value_span() {
        let prefix = "schema = 1\ndefault-address-space = \"cpu\"\n[[address-spaces]]\nid = \"cpu\"\naddress-width = 32\nendianness = \"little\"\n";
        let cases = [
            (
                "invalid-kind",
                format!(
                    "{prefix}[[regions]]\nname = \"radio\"\naddress-space = \"cpu\"\nkind = \"peripheral\"\nstart = 0x1000\nend-exclusive = 0x2000\n"
                ),
                "\"peripheral\"",
                "unsupported memory region kind",
            ),
            (
                "volatile-type",
                format!(
                    "{prefix}[[regions]]\nname = \"radio\"\naddress-space = \"cpu\"\nkind = \"mmio\"\nstart = 0x1000\nend-exclusive = 0x2000\nvolatile = \"yes\"\n"
                ),
                "\"yes\"",
                "volatile must be a boolean",
            ),
            (
                "reversed-range",
                format!(
                    "{prefix}[[regions]]\nname = \"radio\"\naddress-space = \"cpu\"\nkind = \"mmio\"\nstart = 0x2000\nend-exclusive = 0x1000\n"
                ),
                "0x1000",
                "empty or reversed",
            ),
            (
                "unknown-region-key",
                format!(
                    "{prefix}[[regions]]\nname = \"radio\"\naddress-space = \"cpu\"\nkind = \"mmio\"\nstart = 0x1000\nend-exclusive = 0x2000\npermission = \"rw\"\n"
                ),
                "\"rw\"",
                "unknown key \"permission\"",
            ),
        ];

        for (name, input, physical_value, expected_message) in cases {
            let (offset, length, message) = invalid_map_span(&input, name);
            assert_eq!(
                offset,
                input.find(physical_value).unwrap(),
                "{name}: {message}"
            );
            assert_eq!(length, physical_value.len(), "{name}: {message}");
            assert!(message.contains(expected_message), "{name}: {message}");
        }
    }
}
