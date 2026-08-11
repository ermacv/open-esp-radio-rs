//! Linkage-oriented symbol inventory for ELF files and archives.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use object::{
    FileKind, Object, ObjectKind, ObjectSection, ObjectSymbol, SectionFlags, SectionKind,
    SymbolFlags, SymbolKind, SymbolScope, SymbolSection, read::archive::ArchiveFile,
};
use rv_asm::{Inst, Reg};

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

fn executable_section<'data>(section: &impl ObjectSection<'data>) -> bool {
    section.kind() == SectionKind::Text
        || matches!(
            section.flags(),
            SectionFlags::Elf { sh_flags }
                if sh_flags & u64::from(object::elf::SHF_EXECINSTR) != 0
        )
}

fn code_section_coverage<'data>(
    file: &object::File<'data>,
) -> Result<Vec<ArtifactCodeSectionCoverage>> {
    let mut sections = Vec::new();
    for section in file.sections() {
        if !executable_section(&section) || section.size() == 0 {
            continue;
        }
        let mut ranges = Vec::<(u64, u64)>::new();
        let mut sized_symbols = Vec::<(String, u64, u64)>::new();
        let mut zero_sized_symbols = Vec::<(String, u64)>::new();
        let mut named_sized_symbols = 0usize;
        let mut named_zero_sized_symbols = 0usize;
        for symbol in file.symbols() {
            if symbol.kind() != SymbolKind::Text
                || !symbol.is_definition()
                || symbol.section_index() != Some(section.index())
            {
                continue;
            }
            let symbol_name = String::from_utf8_lossy(symbol.name_bytes()?).into_owned();
            if symbol_name.is_empty() {
                continue;
            }
            if symbol.size() == 0 {
                named_zero_sized_symbols += 1;
                let start = symbol
                    .address()
                    .checked_sub(section.address())
                    .ok_or_else(|| format!("text symbol {symbol_name:?} precedes its section"))?;
                zero_sized_symbols.push((symbol_name, start));
                continue;
            }
            named_sized_symbols += 1;
            let start = symbol
                .address()
                .checked_sub(section.address())
                .ok_or_else(|| format!("text symbol {symbol_name:?} precedes its section"))?;
            let end = start
                .checked_add(symbol.size())
                .ok_or_else(|| format!("text symbol {symbol_name:?} address range overflows"))?;
            if end > section.size() {
                return Err(format!("text symbol {symbol_name:?} exceeds its section").into());
            }
            ranges.push((start, end));
            sized_symbols.push((symbol_name, start, end));
        }
        ranges.sort_unstable();
        let mut merged = Vec::<(u64, u64)>::new();
        for (start, end) in ranges {
            if let Some((_, previous_end)) = merged.last_mut()
                && start <= *previous_end
            {
                *previous_end = (*previous_end).max(end);
                continue;
            }
            merged.push((start, end));
        }
        let symbol_covered_bytes = merged.iter().map(|(start, end)| end - start).sum();
        let mut uncovered_ranges = Vec::new();
        let mut cursor = 0_u64;
        for (start, end) in merged {
            if cursor < start {
                uncovered_ranges.push(ArtifactCodeRange {
                    start_offset: cursor,
                    end_offset: start,
                });
            }
            cursor = end;
        }
        if cursor < section.size() {
            uncovered_ranges.push(ArtifactCodeRange {
                start_offset: cursor,
                end_offset: section.size(),
            });
        }
        let mut candidate_evidence = BTreeMap::<
            u64,
            (
                BTreeSet<String>,
                BTreeSet<ArtifactDirectControlFlowEvidence>,
            ),
        >::new();
        let mut recovery_blockers = Vec::new();
        let is_uncovered = |offset: u64| {
            uncovered_ranges
                .iter()
                .any(|range| offset >= range.start_offset && offset < range.end_offset)
        };
        for (name, offset) in zero_sized_symbols {
            if offset >= section.size() {
                recovery_blockers.push(ArtifactCodeRecoveryBlocker {
                    symbol: name,
                    message: format!(
                        "zero-sized text symbol offset {offset:#x} is outside section size {:#x}",
                        section.size()
                    ),
                });
            } else if is_uncovered(offset) {
                candidate_evidence.entry(offset).or_default().0.insert(name);
            }
        }
        let section_data = section.data()?;
        for (name, start, end) in &sized_symbols {
            let Some(bytes) = section_data.get(*start as usize..*end as usize) else {
                recovery_blockers.push(ArtifactCodeRecoveryBlocker {
                    symbol: name.clone(),
                    message: "symbol bytes are outside the executable section data".to_owned(),
                });
                continue;
            };
            let definition = ArtifactSymbolDefinition {
                member: None,
                name: name.clone(),
                address: section.address() + start,
                bytes: bytes.to_vec(),
                addresses_resolved: file.kind() != ObjectKind::Relocatable,
                memory_regions: Default::default(),
                relocations: Vec::new(),
            };
            let decoded = match super::decode::decode_symbol(&definition) {
                Ok(decoded) => decoded,
                Err(error) => {
                    recovery_blockers.push(ArtifactCodeRecoveryBlocker {
                        symbol: name.clone(),
                        message: error.to_string(),
                    });
                    continue;
                }
            };
            for instruction in decoded {
                let Inst::Jal { offset, dest } = instruction.instruction else {
                    continue;
                };
                let kind = match dest {
                    Reg::ZERO => ArtifactDirectControlFlowKind::TailCall,
                    Reg::RA | Reg::T0 => ArtifactDirectControlFlowKind::Call,
                    _ => continue,
                };
                let Ok(pc) = u32::try_from(instruction.address) else {
                    recovery_blockers.push(ArtifactCodeRecoveryBlocker {
                        symbol: name.clone(),
                        message: format!(
                            "direct control-flow site {:#x} exceeds the RV32 address space",
                            instruction.address
                        ),
                    });
                    continue;
                };
                let target = u64::from(pc.wrapping_add(offset.as_u32()));
                let Some(target_offset) = target.checked_sub(section.address()) else {
                    continue;
                };
                if target_offset >= section.size() || !is_uncovered(target_offset) {
                    continue;
                }
                candidate_evidence
                    .entry(target_offset)
                    .or_default()
                    .1
                    .insert(ArtifactDirectControlFlowEvidence {
                        caller: name.clone(),
                        site_offset: instruction.address - section.address(),
                        kind,
                    });
            }
        }
        recovery_blockers.sort_by(|left, right| {
            (&left.symbol, &left.message).cmp(&(&right.symbol, &right.message))
        });
        recovery_blockers.dedup();
        let entries = candidate_evidence.keys().copied().collect::<Vec<_>>();
        let function_candidates = entries
            .iter()
            .enumerate()
            .map(|(index, entry_offset)| {
                let gap = uncovered_ranges
                    .iter()
                    .find(|range| {
                        *entry_offset >= range.start_offset && *entry_offset < range.end_offset
                    })
                    .expect("candidate entries are restricted to uncovered ranges");
                let end_limit_offset = entries
                    .get(index + 1)
                    .copied()
                    .filter(|next| *next < gap.end_offset)
                    .unwrap_or(gap.end_offset);
                let (symbol_names, direct_control_flow) =
                    candidate_evidence.remove(entry_offset).unwrap_or_default();
                ArtifactFunctionBoundaryCandidate {
                    entry_offset: *entry_offset,
                    end_limit_offset,
                    symbol_names: symbol_names.into_iter().collect(),
                    direct_control_flow: direct_control_flow.into_iter().collect(),
                }
            })
            .collect();
        sections.push(ArtifactCodeSectionCoverage {
            name: section.name().unwrap_or("<unnamed>").to_owned(),
            address: section.address(),
            size: section.size(),
            named_sized_symbols,
            named_zero_sized_symbols,
            symbol_covered_bytes,
            uncovered_ranges,
            function_candidates,
            recovery_blockers,
        });
    }
    sections.sort_by(|left, right| (&left.name, left.address).cmp(&(&right.name, right.address)));
    Ok(sections)
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
        code_sections: code_section_coverage(&file)?,
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
    let data = crate::read_artifact(path)?;
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
