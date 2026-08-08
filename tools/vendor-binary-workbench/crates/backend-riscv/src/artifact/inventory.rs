//! Linkage-oriented symbol inventory for ELF files and archives.

use std::{fs, path::Path};

use object::{
    FileKind, Object, ObjectKind, ObjectSection, ObjectSymbol, SymbolFlags, SymbolKind,
    SymbolScope, SymbolSection, read::archive::ArchiveFile,
};

use crate::Result;

use super::model::*;

fn object_kind(kind: ObjectKind) -> ArtifactObjectKind {
    match kind {
        ObjectKind::Relocatable => ArtifactObjectKind::Relocatable,
        ObjectKind::Executable => ArtifactObjectKind::Executable,
        ObjectKind::Dynamic => ArtifactObjectKind::Dynamic,
        ObjectKind::Core => ArtifactObjectKind::Core,
        _ => ArtifactObjectKind::Unknown,
    }
}

fn symbol_binding<'data>(symbol: &impl ObjectSymbol<'data>) -> ArtifactSymbolBinding {
    if let SymbolFlags::Elf { st_info, .. } = symbol.flags() {
        return match st_info >> 4 {
            object::elf::STB_LOCAL => ArtifactSymbolBinding::Local,
            object::elf::STB_GLOBAL => ArtifactSymbolBinding::Global,
            object::elf::STB_WEAK => ArtifactSymbolBinding::Weak,
            object::elf::STB_GNU_UNIQUE => ArtifactSymbolBinding::GnuUnique,
            value => ArtifactSymbolBinding::Unknown(value),
        };
    }
    if symbol.is_weak() {
        ArtifactSymbolBinding::Weak
    } else if symbol.is_local() {
        ArtifactSymbolBinding::Local
    } else if symbol.is_global() {
        ArtifactSymbolBinding::Global
    } else {
        ArtifactSymbolBinding::Unknown(0xff)
    }
}

fn symbol_visibility<'data>(symbol: &impl ObjectSymbol<'data>) -> ArtifactSymbolVisibility {
    match symbol.flags().elf_visibility() {
        Some(object::elf::STV_DEFAULT) => ArtifactSymbolVisibility::Default,
        Some(object::elf::STV_INTERNAL) => ArtifactSymbolVisibility::Internal,
        Some(object::elf::STV_HIDDEN) => ArtifactSymbolVisibility::Hidden,
        Some(object::elf::STV_PROTECTED) => ArtifactSymbolVisibility::Protected,
        Some(value) => ArtifactSymbolVisibility::Unknown(value),
        None => ArtifactSymbolVisibility::Default,
    }
}

fn symbol_kind(kind: SymbolKind) -> ArtifactSymbolKind {
    match kind {
        SymbolKind::Text => ArtifactSymbolKind::Text,
        SymbolKind::Data => ArtifactSymbolKind::Data,
        SymbolKind::Section => ArtifactSymbolKind::Section,
        SymbolKind::File => ArtifactSymbolKind::File,
        SymbolKind::Label => ArtifactSymbolKind::Label,
        SymbolKind::Tls => ArtifactSymbolKind::Tls,
        _ => ArtifactSymbolKind::Unknown,
    }
}

fn symbol_definition(section: SymbolSection) -> ArtifactSymbolDefinitionState {
    match section {
        SymbolSection::Undefined => ArtifactSymbolDefinitionState::Undefined,
        SymbolSection::Absolute => ArtifactSymbolDefinitionState::Absolute,
        SymbolSection::Common => ArtifactSymbolDefinitionState::Common,
        SymbolSection::Section(_) => ArtifactSymbolDefinitionState::Section,
        SymbolSection::None => ArtifactSymbolDefinitionState::None,
        _ => ArtifactSymbolDefinitionState::Unknown,
    }
}

fn symbol_scope(scope: SymbolScope) -> ArtifactSymbolScope {
    match scope {
        SymbolScope::Compilation => ArtifactSymbolScope::Compilation,
        SymbolScope::Linkage => ArtifactSymbolScope::Linkage,
        SymbolScope::Dynamic => ArtifactSymbolScope::Dynamic,
        SymbolScope::Unknown => ArtifactSymbolScope::Unknown,
    }
}

fn symbol_fact<'data>(
    file: &object::File<'data>,
    symbol: impl ObjectSymbol<'data>,
    table: ArtifactSymbolTable,
) -> Result<Option<ArtifactSymbolFact>> {
    let name_bytes = symbol.name_bytes()?;
    if name_bytes.is_empty() {
        return Ok(None);
    }
    let section = symbol
        .section_index()
        .map(|index| {
            file.section_by_index(index)
                .and_then(|section| section.name().map(str::to_owned))
        })
        .transpose()?;
    Ok(Some(ArtifactSymbolFact {
        table,
        name: String::from_utf8_lossy(name_bytes).into_owned(),
        address: symbol.address(),
        size: symbol.size(),
        binding: symbol_binding(&symbol),
        visibility: symbol_visibility(&symbol),
        kind: symbol_kind(symbol.kind()),
        definition: symbol_definition(symbol.section()),
        section,
        scope: symbol_scope(symbol.scope()),
    }))
}

fn inspect_object(data: &[u8], member: Option<String>) -> Result<ArtifactObjectInventory> {
    let file = object::File::parse(data)?;
    if file.architecture() != object::Architecture::Riscv32 {
        return Err(format!("artifact member {member:?} is not RISC-V 32-bit").into());
    }
    if !file.is_little_endian() {
        return Err(format!("artifact member {member:?} is not little-endian").into());
    }
    let mut symbols = Vec::new();
    for symbol in file.symbols() {
        if let Some(fact) = symbol_fact(&file, symbol, ArtifactSymbolTable::Static)? {
            symbols.push(fact);
        }
    }
    for symbol in file.dynamic_symbols() {
        if let Some(fact) = symbol_fact(&file, symbol, ArtifactSymbolTable::Dynamic)? {
            symbols.push(fact);
        }
    }
    symbols.sort_by(|left, right| {
        (
            &left.name,
            left.table,
            left.definition,
            left.binding,
            left.address,
            left.size,
        )
            .cmp(&(
                &right.name,
                right.table,
                right.definition,
                right.binding,
                right.address,
                right.size,
            ))
    });
    Ok(ArtifactObjectInventory {
        member,
        kind: object_kind(file.kind()),
        symbols,
    })
}

/// Read the named ELF symbol facts needed for project linkage analysis.
///
/// This inventory is deliberately separate from [`ArtifactSymbolDefinition`]:
/// undefined imports, data, local and absolute symbols are linkage facts but
/// are not decodable function bodies.
#[tracing::instrument(name = "inspect_riscv_artifact", skip_all, fields(path = %path.display()))]
pub fn inspect_artifact(path: &Path) -> Result<ArtifactInventory> {
    let data = fs::read(path)?;
    match FileKind::parse(data.as_slice())? {
        FileKind::Archive => {
            let archive = ArchiveFile::parse(data.as_slice())?;
            let mut objects = Vec::new();
            let mut skipped_members = 0usize;
            for member in archive.members() {
                let member = member?;
                let member_data = member.data(data.as_slice())?;
                if FileKind::parse(member_data) != Ok(FileKind::Elf32) {
                    skipped_members += 1;
                    continue;
                }
                objects.push(inspect_object(
                    member_data,
                    Some(String::from_utf8_lossy(member.name()).into_owned()),
                )?);
            }
            if objects.is_empty() {
                return Err("archive has no RISC-V ELF32 members".into());
            }
            objects.sort_by(|left, right| left.member.cmp(&right.member));
            Ok(ArtifactInventory {
                container: ArtifactContainerKind::Archive,
                objects,
                skipped_members,
            })
        }
        FileKind::Elf32 => Ok(ArtifactInventory {
            container: ArtifactContainerKind::Elf32,
            objects: vec![inspect_object(&data, None)?],
            skipped_members: 0,
        }),
        kind => Err(format!("unsupported artifact kind: {kind:?}").into()),
    }
}
