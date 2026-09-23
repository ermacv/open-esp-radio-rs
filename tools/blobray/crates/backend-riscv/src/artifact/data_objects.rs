//! Static data-object inventory for linked ELF images and archive members.

use std::{
    collections::{BTreeSet, HashMap},
    ops::Bound::{Excluded, Unbounded},
    path::Path,
};

use object::{
    FileKind, Object, ObjectKind, ObjectSection, ObjectSymbol, RelocationFlags, RelocationTarget,
    SectionIndex, SectionKind, SymbolKind,
};

use crate::Result;

use super::model::{ArtifactDataObjectDefinition, ArtifactDataObjectRelocation};

fn section_properties(kind: SectionKind) -> Option<(bool, bool)> {
    match kind {
        SectionKind::Data | SectionKind::Tls => Some((true, true)),
        SectionKind::ReadOnlyData | SectionKind::ReadOnlyString => Some((false, true)),
        SectionKind::UninitializedData | SectionKind::UninitializedTls | SectionKind::Common => {
            Some((true, false))
        }
        _ => None,
    }
}

fn collect_relocations(
    file: &object::File<'_>,
    section_index: SectionIndex,
    start: u64,
    end: u64,
) -> Result<Vec<ArtifactDataObjectRelocation>> {
    let section = file.section_by_index(section_index)?;
    let mut output = Vec::new();
    for (raw_offset, relocation) in section.relocations() {
        let address = if raw_offset >= section.address()
            && raw_offset < section.address().wrapping_add(section.size())
        {
            raw_offset
        } else {
            section.address().wrapping_add(raw_offset)
        };
        if !(start..end).contains(&address) {
            continue;
        }
        let (target, target_address) = match relocation.target() {
            RelocationTarget::Symbol(index) => {
                let symbol = file.symbol_by_index(index)?;
                (
                    symbol.name().unwrap_or("<unnamed-symbol>").to_owned(),
                    symbol.is_definition().then_some(symbol.address()),
                )
            }
            RelocationTarget::Section(index) => {
                let section = file.section_by_index(index)?;
                (
                    section.name().unwrap_or("<unnamed-section>").to_owned(),
                    Some(section.address()),
                )
            }
            RelocationTarget::Absolute => ("<absolute>".to_owned(), None),
            _ => ("<unknown>".to_owned(), None),
        };
        output.push(ArtifactDataObjectRelocation {
            offset: address - start,
            elf_type: match relocation.flags() {
                RelocationFlags::Elf { r_type } => Some(r_type),
                _ => None,
            },
            target,
            target_address,
            addend: relocation.addend(),
        });
    }
    output.sort();
    Ok(output)
}

fn collect_objects(
    data: &[u8],
    member: Option<&str>,
    artifact_sha256: &str,
    object_location: crate::ObjectLocation,
    output: &mut Vec<ArtifactDataObjectDefinition>,
) -> Result<()> {
    let file = object::File::parse(data)?;
    if file.architecture() != object::Architecture::Riscv32 || !file.is_little_endian() {
        return Err(
            format!("artifact member {member:?} is not little-endian RISC-V 32-bit").into(),
        );
    }
    let addresses_resolved = file.kind() != ObjectKind::Relocatable;
    let mut aliases_by_location = HashMap::<_, Vec<String>>::new();
    let mut symbol_locations_by_section = HashMap::<SectionIndex, BTreeSet<u64>>::new();
    for symbol in file
        .symbols()
        .chain(file.dynamic_symbols())
        .filter(|symbol| symbol.is_definition())
    {
        let (Some(section_index), Ok(name)) = (symbol.section_index(), symbol.name()) else {
            continue;
        };
        symbol_locations_by_section
            .entry(section_index)
            .or_default()
            .insert(symbol.address());
        if !name.is_empty() {
            aliases_by_location
                .entry((section_index, symbol.address()))
                .or_default()
                .push(name.to_owned());
        }
    }
    for aliases in aliases_by_location.values_mut() {
        aliases.sort();
        aliases.dedup();
    }

    for (table, symbol) in file
        .symbols()
        .map(|symbol| (super::ArtifactSymbolTable::Static, symbol))
        .chain(
            file.dynamic_symbols()
                .map(|symbol| (super::ArtifactSymbolTable::Dynamic, symbol)),
        )
    {
        if symbol.kind() != SymbolKind::Data || !symbol.is_definition() || symbol.size() == 0 {
            continue;
        }
        let name = symbol.name()?;
        let Some(section_index) = symbol.section_index() else {
            continue;
        };
        let section = file.section_by_index(section_index)?;
        let Some((writable, initialized)) = section_properties(section.kind()) else {
            continue;
        };
        let object_offset = symbol
            .address()
            .checked_sub(section.address())
            .ok_or_else(|| format!("data symbol {name} precedes its section"))?;
        let initializer = if initialized {
            let start = usize::try_from(object_offset)
                .map_err(|_| format!("data symbol {name} offset exceeds host address space"))?;
            let size = usize::try_from(symbol.size())
                .map_err(|_| format!("data symbol {name} size exceeds host address space"))?;
            let end = start
                .checked_add(size)
                .ok_or_else(|| format!("data symbol {name} size overflows"))?;
            section
                .data()?
                .get(start..end)
                .ok_or_else(|| format!("data symbol {name} exceeds its section"))?
                .to_vec()
        } else {
            Vec::new()
        };
        let symbol_end = symbol
            .address()
            .checked_add(symbol.size())
            .ok_or_else(|| format!("data symbol {name} address range overflows"))?;
        output.push(ArtifactDataObjectDefinition {
            identity: super::DataIdentity::Symbol {
                artifact_sha256: artifact_sha256.to_owned(),
                location: crate::SymbolLocation {
                    object: object_location,
                    table,
                    index: symbol.index().0 as u64,
                },
            },
            member: member.map(str::to_owned),
            section: section.name().unwrap_or("<unnamed>").to_owned(),
            name: name.to_owned(),
            aliases: aliases_by_location
                .get(&(section_index, symbol.address()))
                .into_iter()
                .flatten()
                .filter(|alias| alias.as_str() != name)
                .cloned()
                .collect(),
            address: addresses_resolved
                .then(|| u32::try_from(symbol.address()).ok())
                .flatten()
                .filter(|address| *address != 0),
            object_offset,
            size: symbol.size(),
            writable,
            initialized,
            synthetic_from_anchor: false,
            exported: symbol.is_global() || symbol.is_weak(),
            initializer,
            relocations: collect_relocations(&file, section_index, symbol.address(), symbol_end)?,
        });
    }

    for (table, anchor) in file
        .symbols()
        .map(|symbol| (super::ArtifactSymbolTable::Static, symbol))
        .chain(
            file.dynamic_symbols()
                .map(|symbol| (super::ArtifactSymbolTable::Dynamic, symbol)),
        )
    {
        if !matches!(anchor.kind(), SymbolKind::Unknown | SymbolKind::Data)
            || !anchor.is_definition()
            || anchor.size() != 0
        {
            continue;
        }
        let Some(section_index) = anchor.section_index() else {
            continue;
        };
        let name = anchor.name()?;
        let section = file.section_by_index(section_index)?;
        let Some((writable, initialized)) = section_properties(section.kind()) else {
            continue;
        };
        let object_offset = anchor
            .address()
            .checked_sub(section.address())
            .ok_or_else(|| format!("data anchor {name} precedes its section"))?;
        // A zero-sized label supplies an anchor, not a proven object extent.
        // Bound its candidate bytes by the next symbol (including unnamed
        // definitions). Keep co-located anchors as independent occurrences.
        let section_end = section.address().saturating_add(section.size());
        let object_end = symbol_locations_by_section
            .get(&section_index)
            .and_then(|locations| {
                locations
                    .range((Excluded(anchor.address()), Unbounded))
                    .next()
                    .copied()
            })
            .unwrap_or(section_end)
            .min(section_end);
        let size = object_end.saturating_sub(anchor.address());
        if size == 0 {
            continue;
        }
        let initializer = if initialized {
            let start = usize::try_from(object_offset)
                .map_err(|_| format!("data anchor {name} offset exceeds host address space"))?;
            let size = usize::try_from(size)
                .map_err(|_| format!("data anchor {name} size exceeds host address space"))?;
            let end = start
                .checked_add(size)
                .ok_or_else(|| format!("data anchor {name} size overflows"))?;
            section
                .data()?
                .get(start..end)
                .ok_or_else(|| format!("data anchor {name} exceeds its section"))?
                .to_vec()
        } else {
            Vec::new()
        };
        let anchor_end = anchor.address().saturating_add(size);
        output.push(ArtifactDataObjectDefinition {
            identity: super::DataIdentity::Symbol {
                artifact_sha256: artifact_sha256.to_owned(),
                location: crate::SymbolLocation {
                    object: object_location,
                    table,
                    index: anchor.index().0 as u64,
                },
            },
            member: member.map(str::to_owned),
            section: section.name().unwrap_or("<unnamed>").to_owned(),
            name: name.to_owned(),
            aliases: aliases_by_location
                .get(&(section_index, anchor.address()))
                .into_iter()
                .flatten()
                .filter(|alias| alias.as_str() != name)
                .cloned()
                .collect(),
            address: addresses_resolved
                .then(|| u32::try_from(anchor.address()).ok())
                .flatten()
                .filter(|address| *address != 0),
            object_offset,
            size,
            writable,
            initialized,
            synthetic_from_anchor: true,
            exported: false,
            initializer,
            relocations: collect_relocations(&file, section_index, anchor.address(), anchor_end)?,
        });
    }
    Ok(())
}

/// Load data definitions and compiler anchors from both ELF symbol tables,
/// including unnamed definitions. Archive offsets remain section-relative.
pub fn load_data_objects(path: &Path) -> Result<Vec<ArtifactDataObjectDefinition>> {
    Ok(super::CapturedArtifact::open(path)?
        .data_objects()?
        .to_vec())
}

pub(super) fn captured_data_objects(
    capture: &super::CapturedArtifact<'_>,
) -> Result<Vec<ArtifactDataObjectDefinition>> {
    let mut objects = Vec::new();
    for object in capture.objects() {
        let data = capture
            .object_bytes(object.location())?
            .ok_or("missing captured object")?;
        if matches!(FileKind::parse(data), Ok(FileKind::Elf32)) {
            collect_objects(
                data,
                object.name(),
                capture.sha256(),
                object.location(),
                &mut objects,
            )?;
        }
    }
    objects.sort_by(|left, right| {
        (&left.member, &left.section, left.object_offset, &left.name).cmp(&(
            &right.member,
            &right.section,
            right.object_offset,
            &right.name,
        ))
    });
    Ok(objects)
}
