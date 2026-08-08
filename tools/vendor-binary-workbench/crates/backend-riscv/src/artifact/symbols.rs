//! Extraction of sized code symbols and their relocation context.

use std::{fs, path::Path};

use object::{
    FileKind, Object, ObjectKind, ObjectSection, ObjectSymbol, SectionKind, SymbolKind,
    read::archive::ArchiveFile,
};

use crate::Result;

use super::{model::*, relocations};

fn collect_object_symbols(
    data: &[u8],
    member: Option<&str>,
    prefix: &str,
    include_local: bool,
    output: &mut Vec<ArtifactSymbolDefinition>,
) -> Result<()> {
    let file = object::File::parse(data)?;
    if file.architecture() != object::Architecture::Riscv32 {
        return Err(format!("artifact member {member:?} is not RISC-V 32-bit").into());
    }
    if !file.is_little_endian() {
        return Err(format!("artifact member {member:?} is not little-endian").into());
    }
    let addresses_resolved = file.kind() != ObjectKind::Relocatable;
    let memory_regions = if addresses_resolved {
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
            .collect()
    } else {
        Vec::new()
    };

    for symbol in file.symbols() {
        if symbol.kind() != SymbolKind::Text
            || !symbol.is_definition()
            || (!include_local && !(symbol.is_global() || symbol.is_weak()))
            || symbol.size() == 0
        {
            continue;
        }
        let name = symbol.name()?;
        if !name.starts_with(prefix) {
            continue;
        }
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
        let mut relocations = Vec::new();
        for relocation in relocations::collect_section_relocations(&file, section_index)? {
            if relocation.address < symbol_start || relocation.address >= symbol_end {
                continue;
            }
            let addend = relocation.addend();
            relocations.push(SymbolRelocation {
                address: u32::try_from(relocation.address)
                    .map_err(|_| format!("relocation in {name} exceeds RV32 address space"))?,
                kind: relocation.kind,
                symbol: relocation.symbol,
                addend,
            });
        }
        relocations.sort_by_key(|relocation| (relocation.address, relocation.kind as u8));
        output.push(ArtifactSymbolDefinition {
            member: member.map(str::to_owned),
            name: name.to_owned(),
            address: symbol.address(),
            bytes,
            addresses_resolved,
            memory_regions: memory_regions.clone(),
            relocations,
        });
    }
    Ok(())
}

#[tracing::instrument(
    name = "load_riscv_symbols",
    skip_all,
    fields(path = %path.display(), prefix, include_local)
)]
fn load_symbols_with_visibility(
    path: &Path,
    prefix: &str,
    include_local: bool,
) -> Result<Vec<ArtifactSymbolDefinition>> {
    let data = fs::read(path)?;
    let mut symbols = Vec::new();
    match FileKind::parse(data.as_slice())? {
        FileKind::Archive => {
            let archive = ArchiveFile::parse(data.as_slice())?;
            for member in archive.members() {
                let member = member?;
                let name = String::from_utf8_lossy(member.name()).into_owned();
                let member_data = member.data(data.as_slice())?;
                if matches!(FileKind::parse(member_data), Ok(FileKind::Elf32)) {
                    collect_object_symbols(
                        member_data,
                        Some(&name),
                        prefix,
                        include_local,
                        &mut symbols,
                    )?;
                }
            }
        }
        FileKind::Elf32 => {
            collect_object_symbols(&data, None, prefix, include_local, &mut symbols)?
        }
        kind => return Err(format!("unsupported artifact kind: {kind:?}").into()),
    }
    symbols.sort_by(|left, right| {
        (&left.member, &left.name, left.address).cmp(&(&right.member, &right.name, right.address))
    });
    Ok(symbols)
}

/// Load exported (global or weak) code symbols.
///
/// This remains the default inventory for validation and verification: adding
/// private implementation details must not silently broaden evidence scope.
pub fn load_symbols(path: &Path, prefix: &str) -> Result<Vec<ArtifactSymbolDefinition>> {
    load_symbols_with_visibility(path, prefix, false)
}

/// Load every named, non-empty code symbol, including local/private functions.
///
/// This broader catalog is intended for exploratory IR and call-graph export.
/// It is not a completeness guarantee: stripped functions and executable bytes
/// without a sized text symbol still have no function boundary here.
pub fn load_all_code_symbols(path: &Path, prefix: &str) -> Result<Vec<ArtifactSymbolDefinition>> {
    load_symbols_with_visibility(path, prefix, true)
}
