//! Structural ELF/archive loading and instruction decoding.
//!
//! This module deliberately does not invoke binutils. Symbol boundaries and
//! instruction bytes come from the binary containers themselves.

use std::{fs, path::Path};

use object::{
    FileKind, Object, ObjectKind, ObjectSection, ObjectSymbol, RelocationFlags, RelocationTarget,
    SectionFlags, SectionKind, SymbolKind, read::archive::ArchiveFile,
};
use rv_asm::{Imm, Inst, IsCompressed, Xlen};

use crate::{Error, Result};

#[derive(Clone, Debug)]
pub struct ArtifactSymbolDefinition {
    pub member: Option<String>,
    pub name: String,
    pub address: u64,
    pub bytes: Vec<u8>,
    pub addresses_resolved: bool,
    pub memory_regions: Vec<MemoryRegion>,
    pub relocations: Vec<SymbolRelocation>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelocationKind {
    Hi20,
    Lo12I,
    Lo12S,
    Call,
    CallPlt,
}

fn riscv_relocation_kind(r_type: u32) -> Option<RelocationKind> {
    match r_type {
        object::elf::R_RISCV_HI20 => Some(RelocationKind::Hi20),
        object::elf::R_RISCV_LO12_I => Some(RelocationKind::Lo12I),
        object::elf::R_RISCV_LO12_S => Some(RelocationKind::Lo12S),
        object::elf::R_RISCV_CALL => Some(RelocationKind::Call),
        object::elf::R_RISCV_CALL_PLT => Some(RelocationKind::CallPlt),
        _ => None,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SymbolRelocation {
    pub address: u32,
    pub kind: RelocationKind,
    pub symbol: String,
    pub addend: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemoryRegion {
    pub start: u32,
    pub length: u32,
    pub writable: bool,
    pub name: String,
}

impl MemoryRegion {
    pub fn contains(&self, address: u32, width: u8) -> bool {
        let length = match width {
            8 => 1,
            16 => 2,
            32 => 4,
            _ => return false,
        };
        let Some(access_end) = address.checked_add(length) else {
            return false;
        };
        let Some(region_end) = self.start.checked_add(self.length) else {
            return false;
        };
        address >= self.start && access_end <= region_end
    }
}

impl ArtifactSymbolDefinition {
    pub fn memory_region(&self, address: u32, width: u8) -> Option<&MemoryRegion> {
        self.memory_regions
            .iter()
            .find(|region| region.contains(address, width))
    }

    pub fn relocation(&self, address: u32, kind: RelocationKind) -> Option<&SymbolRelocation> {
        self.relocations
            .iter()
            .find(|relocation| relocation.address == address && relocation.kind == kind)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DecodedInstruction {
    pub address: u64,
    pub width: u8,
    pub instruction: Inst,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutableSection {
    pub name: String,
    pub address: u64,
    pub bytes: Vec<u8>,
}

/// Return the architectural immediate for ANDI.
///
/// rv-asm 0.2.1 decodes the six-bit C.ANDI immediate as unsigned even though
/// the RISC-V C extension defines it as sign-extended. Keep the workaround at
/// the decode boundary so every analysis engine observes the same value.
pub fn andi_immediate(imm: Imm, width: u8) -> u32 {
    if width == 2 {
        (((imm.as_u32() & 0x3f) << 26) as i32 >> 26) as u32
    } else {
        imm.as_u32()
    }
}

fn collect_object_symbols(
    data: &[u8],
    member: Option<&str>,
    prefix: &str,
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
            || !(symbol.is_global() || symbol.is_weak())
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
        let section_start = section.address();
        let section_end = section_start.wrapping_add(section.size());
        let mut relocations = Vec::new();
        for (offset, relocation) in section.relocations() {
            let RelocationFlags::Elf { r_type } = relocation.flags() else {
                continue;
            };
            let Some(kind) = riscv_relocation_kind(r_type) else {
                continue;
            };
            let RelocationTarget::Symbol(index) = relocation.target() else {
                continue;
            };
            let relocation_address = if offset >= section_start && offset < section_end {
                offset
            } else {
                section_start.wrapping_add(offset)
            };
            if relocation_address < symbol_start || relocation_address >= symbol_end {
                continue;
            }
            relocations.push(SymbolRelocation {
                address: u32::try_from(relocation_address)
                    .map_err(|_| format!("relocation in {name} exceeds RV32 address space"))?,
                kind,
                symbol: file.symbol_by_index(index)?.name()?.to_owned(),
                addend: relocation.addend(),
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

pub fn load_symbols(path: &Path, prefix: &str) -> Result<Vec<ArtifactSymbolDefinition>> {
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
                    collect_object_symbols(member_data, Some(&name), prefix, &mut symbols)?;
                }
            }
        }
        FileKind::Elf32 => collect_object_symbols(&data, None, prefix, &mut symbols)?,
        kind => return Err(format!("unsupported artifact kind: {kind:?}").into()),
    }
    symbols.sort_by(|left, right| (&left.member, &left.name).cmp(&(&right.member, &right.name)));
    Ok(symbols)
}

/// Load every executable section from a fully linked RV32 ELF image.
///
/// This deliberately does not use the symbol table: LTO may make functions
/// local or omit their names, while a final-image policy must cover the bytes
/// that can actually execute.
pub fn load_executable_sections(path: &Path) -> Result<Vec<ExecutableSection>> {
    let data = fs::read(path)?;
    if FileKind::parse(data.as_slice())? != FileKind::Elf32 {
        return Err("executable-section audit requires an ELF32 artifact".into());
    }
    let file = object::File::parse(data.as_slice())?;
    if file.architecture() != object::Architecture::Riscv32 {
        return Err("executable-section audit requires a RISC-V 32-bit artifact".into());
    }
    if !file.is_little_endian() {
        return Err("executable-section audit requires a little-endian artifact".into());
    }
    if file.kind() == ObjectKind::Relocatable {
        return Err("executable-section audit requires a fully linked ELF image".into());
    }

    let mut sections = Vec::new();
    for section in file.sections() {
        let executable = section.kind() == SectionKind::Text
            || matches!(
                section.flags(),
                SectionFlags::Elf { sh_flags }
                    if sh_flags & u64::from(object::elf::SHF_EXECINSTR) != 0
            );
        if !executable || section.size() == 0 {
            continue;
        }
        sections.push(ExecutableSection {
            name: section.name().unwrap_or("<unnamed>").to_owned(),
            address: section.address(),
            bytes: section.data()?.to_vec(),
        });
    }
    if sections.is_empty() {
        return Err("ELF image has no executable sections".into());
    }
    sections.sort_by_key(|section| section.address);
    Ok(sections)
}

pub fn decode_symbol(symbol: &ArtifactSymbolDefinition) -> Result<Vec<DecodedInstruction>> {
    let mut decoded = Vec::new();
    let mut offset = 0_usize;
    while offset < symbol.bytes.len() {
        let remaining = &symbol.bytes[offset..];
        if remaining.len() < 2 {
            return Err(format!("truncated instruction in {} at +{offset:#x}", symbol.name).into());
        }
        let compressed = Inst::first_byte_is_compressed(remaining[0]);
        let width = if compressed { 2 } else { 4 };
        let instruction_bytes = remaining
            .get(..width)
            .ok_or_else(|| format!("truncated instruction in {} at +{offset:#x}", symbol.name))?;
        let mut word = [0_u8; 4];
        word[..width].copy_from_slice(instruction_bytes);
        let (instruction, decoded_width) = Inst::decode(u32::from_le_bytes(word), Xlen::Rv32)
            .map_err(|error| -> Error {
                format!(
                    "cannot decode {} at {:#x}: {error}",
                    symbol.name,
                    symbol.address + offset as u64
                )
                .into()
            })?;
        let expected_width = if decoded_width == IsCompressed::Yes {
            2
        } else {
            4
        };
        if width != expected_width {
            return Err(format!("decoder width disagreement in {}", symbol.name).into());
        }
        decoded.push(DecodedInstruction {
            address: symbol.address + offset as u64,
            width: width as u8,
            instruction,
        });
        offset += width;
    }
    Ok(decoded)
}

#[cfg(test)]
mod tests;
