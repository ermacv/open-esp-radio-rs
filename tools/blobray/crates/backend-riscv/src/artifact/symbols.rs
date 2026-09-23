//! Extraction of sized code symbols and their relocation context.

use std::{collections::HashMap, path::Path, sync::Arc};

use object::{FileKind, Object, ObjectKind, ObjectSection, ObjectSymbol, SectionKind, SymbolKind};

use crate::Result;

use super::{CapturedArtifact, CapturedCodeSymbol, model::*, relocations};
use open_radio_vendor_analysis_model::{ObjectLocation, SymbolLocation};

pub(super) fn collect_object_symbols(
    data: &[u8],
    member: Option<&str>,
    location: ObjectLocation,
    artifact_sha256: &str,
) -> Result<Vec<CapturedCodeSymbol>> {
    let mut output = Vec::new();
    let file = object::File::parse(data)?;
    if file.architecture() != object::Architecture::Riscv32 {
        return Err(format!("artifact member {member:?} is not RISC-V 32-bit").into());
    }
    if !file.is_little_endian() {
        return Err(format!("artifact member {member:?} is not little-endian").into());
    }
    let addresses_resolved = file.kind() != ObjectKind::Relocatable;
    let memory_regions: Arc<[MemoryRegion]> = if addresses_resolved {
        file.sections()
            .filter_map(|section| {
                let writable = match section.kind() {
                    SectionKind::Data
                    | SectionKind::UninitializedData
                    | SectionKind::Common
                    | SectionKind::Tls
                    | SectionKind::UninitializedTls => true,
                    SectionKind::Text | SectionKind::ReadOnlyData | SectionKind::ReadOnlyString => {
                        false
                    }
                    _ => return None,
                };
                let start = u32::try_from(section.address()).ok()?;
                let length = u32::try_from(section.size()).ok()?;
                if start == 0 || length == 0 || start.checked_add(length).is_none() {
                    return None;
                }
                Some(MemoryRegion {
                    start,
                    length,
                    writable,
                    name: section.name().unwrap_or("<unnamed>").to_owned(),
                })
            })
            .collect::<Vec<_>>()
            .into()
    } else {
        Arc::default()
    };
    let mut section_relocations = HashMap::new();

    for (table, symbol) in file
        .symbols()
        .map(|symbol| (ArtifactSymbolTable::Static, symbol))
        .chain(
            file.dynamic_symbols()
                .map(|symbol| (ArtifactSymbolTable::Dynamic, symbol)),
        )
    {
        if symbol.kind() != SymbolKind::Text || !symbol.is_definition() || symbol.size() == 0 {
            continue;
        }
        let name = symbol.name()?;
        let section_index = symbol
            .section_index()
            .ok_or_else(|| format!("text symbol {name} has no section"))?;
        let section = file.section_by_index(section_index)?;
        let section_data = section.data()?;
        let start = symbol
            .address()
            .checked_sub(section.address())
            .ok_or_else(|| format!("symbol {name} precedes its section"))?
            as usize;
        let end = start
            .checked_add(symbol.size() as usize)
            .ok_or_else(|| format!("symbol {name} size overflows"))?;
        let bytes = section_data
            .get(start..end)
            .ok_or_else(|| format!("symbol {name} exceeds its section"))?
            .to_vec();
        let symbol_start = symbol.address();
        let symbol_end = symbol_start
            .checked_add(symbol.size())
            .ok_or_else(|| format!("symbol {name} address range overflows"))?;
        if let std::collections::hash_map::Entry::Vacant(entry) =
            section_relocations.entry(section_index)
        {
            entry.insert(relocations::collect_section_relocations(
                &file,
                section_index,
            )?);
        }
        let mut relocations = Vec::new();
        for relocation in &section_relocations[&section_index] {
            if relocation.address < symbol_start || relocation.address >= symbol_end {
                continue;
            }
            let addend = relocation.addend();
            relocations.push(SymbolRelocation {
                reference: relocation.reference(artifact_sha256, location),
                address: u32::try_from(relocation.address)
                    .map_err(|_| format!("relocation in {name} exceeds RV32 address space"))?,
                kind: relocation.kind,
                symbol: relocation.symbol.clone(),
                addend,
            });
        }
        relocations.sort_by_key(|relocation| (relocation.address, relocation.kind as u8));
        output.push(CapturedCodeSymbol {
            location: SymbolLocation {
                object: location,
                table,
                index: symbol.index().0 as u64,
            },
            exported: symbol.is_global() || symbol.is_weak(),
            definition: ArtifactSymbolDefinition {
                identity: CodeIdentity::Symbol {
                    artifact_sha256: artifact_sha256.to_owned(),
                    location: SymbolLocation {
                        object: location,
                        table,
                        index: symbol.index().0 as u64,
                    },
                },
                member: member.map(str::to_owned),
                name: name.to_owned(),
                address: symbol.address(),
                bytes,
                addresses_resolved,
                memory_regions: memory_regions.clone(),
                relocations,
            },
        });
    }
    Ok(output)
}

#[tracing::instrument(
    name = "load_riscv_symbols",
    skip_all,
    fields(path = %path.display(), prefix, selection = selection.label())
)]
pub fn load_code_symbols(
    path: &Path,
    prefix: &str,
    selection: CodeSymbolSelection,
) -> Result<Vec<ArtifactSymbolDefinition>> {
    let capture = CapturedArtifact::open(path)?;
    selected_code_symbols(&capture, prefix, selection)
}

/// Return physical member names in container order; names need not be unique.
pub fn load_archive_member_names(path: &Path) -> Result<Option<Vec<String>>> {
    let capture = CapturedArtifact::open(path)?;
    Ok(
        (capture.container() == ArtifactContainerKind::Archive).then(|| {
            capture
                .objects()
                .iter()
                .map(|object| object.name().unwrap_or_default().to_owned())
                .collect()
        }),
    )
}

fn selected_code_symbols(
    capture: &CapturedArtifact<'_>,
    prefix: &str,
    selection: CodeSymbolSelection,
) -> Result<Vec<ArtifactSymbolDefinition>> {
    let mut symbols = capture
        .code_symbols(prefix, selection)?
        .into_iter()
        .map(|symbol| symbol.definition.clone())
        .collect::<Vec<_>>();
    symbols.sort_by(|left, right| {
        (&left.member, &left.name, left.address).cmp(&(&right.member, &right.name, right.address))
    });
    Ok(symbols)
}

fn collect_data_symbols(
    data: &[u8],
    member: Option<&str>,
    artifact_sha256: &str,
    object_location: ObjectLocation,
    output: &mut Vec<ArtifactDataSymbolDefinition>,
) -> Result<()> {
    let file = object::File::parse(data)?;
    if file.architecture() != object::Architecture::Riscv32 || !file.is_little_endian() {
        return Err(
            format!("artifact member {member:?} is not little-endian RISC-V 32-bit").into(),
        );
    }
    if file.kind() == ObjectKind::Relocatable {
        return Ok(());
    }
    for (table, symbol) in file
        .symbols()
        .map(|symbol| (ArtifactSymbolTable::Static, symbol))
        .chain(
            file.dynamic_symbols()
                .map(|symbol| (ArtifactSymbolTable::Dynamic, symbol)),
        )
    {
        if symbol.kind() != SymbolKind::Data || !symbol.is_definition() || symbol.size() == 0 {
            continue;
        }
        let Ok(address) = u32::try_from(symbol.address()) else {
            continue;
        };
        let Ok(size) = u32::try_from(symbol.size()) else {
            continue;
        };
        if address == 0 || address.checked_add(size).is_none() {
            continue;
        }
        output.push(ArtifactDataSymbolDefinition {
            identity: DataIdentity::Symbol {
                artifact_sha256: artifact_sha256.to_owned(),
                location: SymbolLocation {
                    object: object_location,
                    table,
                    index: symbol.index().0 as u64,
                },
            },
            member: member.map(str::to_owned),
            name: symbol.name()?.to_owned(),
            address,
            size,
            exported: symbol.is_global() || symbol.is_weak(),
        });
    }
    Ok(())
}

/// Load sized data symbols from linked images without running code-section
/// coverage analysis. Relocatable members are skipped because their section
/// relative addresses are not runtime identities.
pub fn load_data_symbols(path: &Path) -> Result<Vec<ArtifactDataSymbolDefinition>> {
    Ok(CapturedArtifact::open(path)?.data_symbols()?.to_vec())
}

pub(super) fn captured_data_symbols(
    capture: &CapturedArtifact<'_>,
) -> Result<Vec<ArtifactDataSymbolDefinition>> {
    let mut symbols = Vec::new();
    for object in capture.objects() {
        let data = capture
            .object_bytes(object.location())?
            .ok_or("missing captured object")?;
        if matches!(FileKind::parse(data), Ok(FileKind::Elf32)) {
            collect_data_symbols(
                data,
                object.name(),
                capture.sha256(),
                object.location(),
                &mut symbols,
            )?;
        }
    }
    symbols.sort_by(|left, right| {
        (
            left.address,
            left.size,
            !left.exported,
            &left.member,
            &left.name,
        )
            .cmp(&(
                right.address,
                right.size,
                !right.exported,
                &right.member,
                &right.name,
            ))
    });
    Ok(symbols)
}

fn collect_reviewed_ranges(
    data: &[u8],
    member: Option<&str>,
    object: ObjectLocation,
    artifact_sha256: &str,
    ranges: &[ReviewedCodeRange],
    output: &mut Vec<ArtifactSymbolDefinition>,
) -> Result<()> {
    let matching = ranges
        .iter()
        .filter(|range| range.member.as_deref() == member)
        .collect::<Vec<_>>();
    if matching.is_empty() {
        return Ok(());
    }
    let file = object::File::parse(data)?;
    if file.architecture() != object::Architecture::Riscv32 || !file.is_little_endian() {
        return Err(format!(
            "reviewed code-boundary member {member:?} is not little-endian RISC-V 32-bit"
        )
        .into());
    }
    let addresses_resolved = file.kind() != ObjectKind::Relocatable;
    let memory_regions: Arc<[MemoryRegion]> = if addresses_resolved {
        file.sections()
            .filter_map(|section| {
                let writable = match section.kind() {
                    SectionKind::Data
                    | SectionKind::UninitializedData
                    | SectionKind::Common
                    | SectionKind::Tls
                    | SectionKind::UninitializedTls => true,
                    SectionKind::Text | SectionKind::ReadOnlyData | SectionKind::ReadOnlyString => {
                        false
                    }
                    _ => return None,
                };
                let start = u32::try_from(section.address()).ok()?;
                let length = u32::try_from(section.size()).ok()?;
                (start != 0 && length != 0 && start.checked_add(length).is_some()).then(|| {
                    MemoryRegion {
                        start,
                        length,
                        writable,
                        name: section.name().unwrap_or("<unnamed>").to_owned(),
                    }
                })
            })
            .collect::<Vec<_>>()
            .into()
    } else {
        Arc::default()
    };
    let mut section_relocations = HashMap::new();
    for range in matching {
        let mut sections = file
            .sections()
            .filter(|section| section.name().ok() == Some(range.section.as_str()));
        let section = sections.next().ok_or_else(|| {
            format!(
                "reviewed code range {} refers to missing section {:?} in member {member:?}",
                range.name, range.section
            )
        })?;
        if sections.next().is_some() {
            return Err(format!(
                "reviewed code range {} refers to ambiguous section {:?} in member {member:?}",
                range.name, range.section
            )
            .into());
        }
        if section.kind() != SectionKind::Text {
            return Err(format!(
                "reviewed code range {} refers to non-executable section {:?}",
                range.name, range.section
            )
            .into());
        }
        if range.start_offset >= range.end_offset || range.end_offset > section.size() {
            return Err(format!(
                "reviewed code range {} has invalid section offsets {:#x}..{:#x}",
                range.name, range.start_offset, range.end_offset
            )
            .into());
        }
        let section_data = section.data()?;
        let start = usize::try_from(range.start_offset)
            .map_err(|_| format!("reviewed code range {} start is too large", range.name))?;
        let end = usize::try_from(range.end_offset)
            .map_err(|_| format!("reviewed code range {} end is too large", range.name))?;
        let bytes = section_data
            .get(start..end)
            .ok_or_else(|| format!("reviewed code range {} exceeds section data", range.name))?
            .to_vec();
        let address = section
            .address()
            .checked_add(range.start_offset)
            .ok_or_else(|| format!("reviewed code range {} address overflows", range.name))?;
        let end_address = section
            .address()
            .checked_add(range.end_offset)
            .ok_or_else(|| format!("reviewed code range {} end address overflows", range.name))?;
        if let std::collections::hash_map::Entry::Vacant(entry) =
            section_relocations.entry(section.index())
        {
            entry.insert(relocations::collect_section_relocations(
                &file,
                section.index(),
            )?);
        }
        let mut relocations = section_relocations[&section.index()]
            .iter()
            .filter(|relocation| relocation.address >= address && relocation.address < end_address)
            .map(|relocation| {
                let addend = relocation.addend();
                Ok(SymbolRelocation {
                    reference: relocation.reference(artifact_sha256, object),
                    address: u32::try_from(relocation.address).map_err(|_| {
                        format!("relocation in {} exceeds RV32 address space", range.name)
                    })?,
                    kind: relocation.kind,
                    symbol: relocation.symbol.clone(),
                    addend,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        relocations.sort_by_key(|relocation| (relocation.address, relocation.kind as u8));
        output.push(ArtifactSymbolDefinition {
            identity: CodeIdentity::SectionRange {
                artifact_sha256: artifact_sha256.to_owned(),
                object,
                section_index: section.index().0 as u64,
                start_offset: range.start_offset,
                end_offset: range.end_offset,
            },
            member: range.member.clone(),
            name: range.name.clone(),
            address,
            bytes,
            addresses_resolved,
            memory_regions: memory_regions.clone(),
            relocations,
        });
    }
    Ok(())
}

/// Loads only ranges that crossed the reviewed boundary-pack trust boundary.
pub fn load_reviewed_code_ranges(
    path: &Path,
    ranges: &[ReviewedCodeRange],
) -> Result<Vec<ArtifactSymbolDefinition>> {
    let capture = CapturedArtifact::open(path)?;
    reviewed_code_ranges(&capture, ranges)
}

pub(super) fn reviewed_code_ranges(
    capture: &CapturedArtifact<'_>,
    ranges: &[ReviewedCodeRange],
) -> Result<Vec<ArtifactSymbolDefinition>> {
    let mut symbols = Vec::new();
    for object in capture.objects() {
        if !ranges
            .iter()
            .any(|range| range.member.as_deref() == object.name())
        {
            continue;
        }
        let data = capture
            .object_bytes(object.location())?
            .ok_or("missing captured object")?;
        collect_reviewed_ranges(
            data,
            object.name(),
            object.location(),
            capture.sha256(),
            ranges,
            &mut symbols,
        )?;
    }
    if symbols.len() != ranges.len() {
        return Err(format!(
            "loaded {} of {} reviewed code ranges; member selection must be unambiguous",
            symbols.len(),
            ranges.len(),
        )
        .into());
    }
    symbols.sort_by(|left, right| left.identity.cmp(&right.identity));
    Ok(symbols)
}
