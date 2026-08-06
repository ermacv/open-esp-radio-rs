//! Structural ELF/archive loading and instruction decoding.
//!
//! This module deliberately does not invoke binutils. Symbol boundaries and
//! instruction bytes come from the binary containers themselves.

use std::{fs, path::Path};

use object::{
    FileKind, Object, ObjectKind, ObjectSection, ObjectSymbol, SectionFlags, SectionKind,
    SymbolFlags, SymbolKind, SymbolScope, SymbolSection, read::archive::ArchiveFile,
};
use rv_asm::{Imm, Inst, IsCompressed, Reg, Xlen};

use crate::{Error, Result};

mod relocations;

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

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ArtifactContainerKind {
    Elf32,
    Archive,
}

impl ArtifactContainerKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Elf32 => "elf32",
            Self::Archive => "archive",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ArtifactObjectKind {
    Relocatable,
    Executable,
    Dynamic,
    Core,
    Unknown,
}

impl ArtifactObjectKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Relocatable => "relocatable",
            Self::Executable => "executable",
            Self::Dynamic => "dynamic",
            Self::Core => "core",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ArtifactSymbolTable {
    Static,
    Dynamic,
}

impl ArtifactSymbolTable {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Static => "static",
            Self::Dynamic => "dynamic",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ArtifactSymbolBinding {
    Local,
    Global,
    Weak,
    GnuUnique,
    Unknown(u8),
}

impl ArtifactSymbolBinding {
    pub fn label(self) -> String {
        match self {
            Self::Local => "local".to_owned(),
            Self::Global => "global".to_owned(),
            Self::Weak => "weak".to_owned(),
            Self::GnuUnique => "gnu-unique".to_owned(),
            Self::Unknown(value) => format!("unknown-{value}"),
        }
    }

    pub const fn is_export_candidate(self) -> bool {
        matches!(self, Self::Global | Self::Weak | Self::GnuUnique)
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ArtifactSymbolVisibility {
    Default,
    Internal,
    Hidden,
    Protected,
    Unknown(u8),
}

impl ArtifactSymbolVisibility {
    pub fn label(self) -> String {
        match self {
            Self::Default => "default".to_owned(),
            Self::Internal => "internal".to_owned(),
            Self::Hidden => "hidden".to_owned(),
            Self::Protected => "protected".to_owned(),
            Self::Unknown(value) => format!("unknown-{value}"),
        }
    }

    pub const fn is_externally_visible(self) -> bool {
        matches!(self, Self::Default | Self::Protected)
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ArtifactSymbolKind {
    Unknown,
    Text,
    Data,
    Section,
    File,
    Label,
    Tls,
}

impl ArtifactSymbolKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Text => "text",
            Self::Data => "data",
            Self::Section => "section",
            Self::File => "file",
            Self::Label => "label",
            Self::Tls => "tls",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ArtifactSymbolScope {
    Compilation,
    Linkage,
    Dynamic,
    Unknown,
}

impl ArtifactSymbolScope {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Compilation => "compilation",
            Self::Linkage => "linkage",
            Self::Dynamic => "dynamic",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ArtifactSymbolDefinitionState {
    Undefined,
    Absolute,
    Common,
    Section,
    None,
    Unknown,
}

impl ArtifactSymbolDefinitionState {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Undefined => "undefined",
            Self::Absolute => "absolute",
            Self::Common => "common",
            Self::Section => "section",
            Self::None => "none",
            Self::Unknown => "unknown",
        }
    }

    pub const fn is_definition(self) -> bool {
        matches!(self, Self::Absolute | Self::Common | Self::Section)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactSymbolFact {
    pub table: ArtifactSymbolTable,
    pub name: String,
    pub address: u64,
    pub size: u64,
    pub binding: ArtifactSymbolBinding,
    pub visibility: ArtifactSymbolVisibility,
    pub kind: ArtifactSymbolKind,
    pub definition: ArtifactSymbolDefinitionState,
    pub section: Option<String>,
    pub scope: ArtifactSymbolScope,
}

impl ArtifactSymbolFact {
    pub const fn is_exported_definition(&self) -> bool {
        self.definition.is_definition()
            && self.binding.is_export_candidate()
            && self.visibility.is_externally_visible()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactObjectInventory {
    pub member: Option<String>,
    pub kind: ArtifactObjectKind,
    pub symbols: Vec<ArtifactSymbolFact>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactInventory {
    pub container: ArtifactContainerKind,
    pub objects: Vec<ArtifactObjectInventory>,
    pub skipped_members: usize,
}

impl ArtifactInventory {
    pub fn symbols(&self) -> impl Iterator<Item = (&ArtifactObjectInventory, &ArtifactSymbolFact)> {
        self.objects
            .iter()
            .flat_map(|object| object.symbols.iter().map(move |symbol| (object, symbol)))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelocationKind {
    GotHi20,
    Hi20,
    Lo12I,
    Lo12S,
    PcRelHi20,
    PcRelLo12I,
    PcRelLo12S,
    GotPcRelLo12I,
    Call,
    CallPlt,
}

fn riscv_relocation_kind(r_type: u32) -> Option<RelocationKind> {
    match r_type {
        object::elf::R_RISCV_GOT_HI20 => Some(RelocationKind::GotHi20),
        object::elf::R_RISCV_HI20 => Some(RelocationKind::Hi20),
        object::elf::R_RISCV_LO12_I => Some(RelocationKind::Lo12I),
        object::elf::R_RISCV_LO12_S => Some(RelocationKind::Lo12S),
        object::elf::R_RISCV_PCREL_HI20 => Some(RelocationKind::PcRelHi20),
        object::elf::R_RISCV_PCREL_LO12_I => Some(RelocationKind::PcRelLo12I),
        object::elf::R_RISCV_PCREL_LO12_S => Some(RelocationKind::PcRelLo12S),
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

fn inventory_object_kind(kind: ObjectKind) -> ArtifactObjectKind {
    match kind {
        ObjectKind::Relocatable => ArtifactObjectKind::Relocatable,
        ObjectKind::Executable => ArtifactObjectKind::Executable,
        ObjectKind::Dynamic => ArtifactObjectKind::Dynamic,
        ObjectKind::Core => ArtifactObjectKind::Core,
        _ => ArtifactObjectKind::Unknown,
    }
}

fn inventory_symbol_binding<'data>(symbol: &impl ObjectSymbol<'data>) -> ArtifactSymbolBinding {
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

fn inventory_symbol_visibility<'data>(
    symbol: &impl ObjectSymbol<'data>,
) -> ArtifactSymbolVisibility {
    match symbol.flags().elf_visibility() {
        Some(object::elf::STV_DEFAULT) => ArtifactSymbolVisibility::Default,
        Some(object::elf::STV_INTERNAL) => ArtifactSymbolVisibility::Internal,
        Some(object::elf::STV_HIDDEN) => ArtifactSymbolVisibility::Hidden,
        Some(object::elf::STV_PROTECTED) => ArtifactSymbolVisibility::Protected,
        Some(value) => ArtifactSymbolVisibility::Unknown(value),
        None => ArtifactSymbolVisibility::Default,
    }
}

fn inventory_symbol_kind(kind: SymbolKind) -> ArtifactSymbolKind {
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

fn inventory_symbol_definition(section: SymbolSection) -> ArtifactSymbolDefinitionState {
    match section {
        SymbolSection::Undefined => ArtifactSymbolDefinitionState::Undefined,
        SymbolSection::Absolute => ArtifactSymbolDefinitionState::Absolute,
        SymbolSection::Common => ArtifactSymbolDefinitionState::Common,
        SymbolSection::Section(_) => ArtifactSymbolDefinitionState::Section,
        SymbolSection::None => ArtifactSymbolDefinitionState::None,
        _ => ArtifactSymbolDefinitionState::Unknown,
    }
}

fn inventory_symbol_scope(scope: SymbolScope) -> ArtifactSymbolScope {
    match scope {
        SymbolScope::Compilation => ArtifactSymbolScope::Compilation,
        SymbolScope::Linkage => ArtifactSymbolScope::Linkage,
        SymbolScope::Dynamic => ArtifactSymbolScope::Dynamic,
        SymbolScope::Unknown => ArtifactSymbolScope::Unknown,
    }
}

fn inventory_symbol_fact<'data>(
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
        binding: inventory_symbol_binding(&symbol),
        visibility: inventory_symbol_visibility(&symbol),
        kind: inventory_symbol_kind(symbol.kind()),
        definition: inventory_symbol_definition(symbol.section()),
        section,
        scope: inventory_symbol_scope(symbol.scope()),
    }))
}

fn inventory_object(data: &[u8], member: Option<String>) -> Result<ArtifactObjectInventory> {
    let file = object::File::parse(data)?;
    if file.architecture() != object::Architecture::Riscv32 {
        return Err(format!("artifact member {member:?} is not RISC-V 32-bit").into());
    }
    if !file.is_little_endian() {
        return Err(format!("artifact member {member:?} is not little-endian").into());
    }
    let mut symbols = Vec::new();
    for symbol in file.symbols() {
        if let Some(fact) = inventory_symbol_fact(&file, symbol, ArtifactSymbolTable::Static)? {
            symbols.push(fact);
        }
    }
    for symbol in file.dynamic_symbols() {
        if let Some(fact) = inventory_symbol_fact(&file, symbol, ArtifactSymbolTable::Dynamic)? {
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
        kind: inventory_object_kind(file.kind()),
        symbols,
    })
}

/// Read the named ELF symbol facts needed for project linkage analysis.
///
/// This inventory is deliberately separate from [`ArtifactSymbolDefinition`]:
/// undefined imports, data, local and absolute symbols are linkage facts but
/// are not decodable function bodies.
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
                objects.push(inventory_object(
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
            objects: vec![inventory_object(&data, None)?],
            skipped_members: 0,
        }),
        kind => Err(format!("unsupported artifact kind: {kind:?}").into()),
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

/// Classify the JALR half of a standard two-instruction RISC-V call
/// relocation. Returns `None` for malformed or non-standard link registers.
pub fn relocated_call_is_tail(
    symbol: &ArtifactSymbolDefinition,
    relocation_address: u32,
) -> Option<bool> {
    let jalr_address = relocation_address.checked_add(4)?;
    let instruction = decode_symbol(symbol)
        .ok()?
        .into_iter()
        .find(|decoded| decoded.address == u64::from(jalr_address))?
        .instruction;
    match instruction {
        Inst::Jalr {
            dest: Reg::ZERO, ..
        } => Some(true),
        Inst::Jalr { dest: Reg::RA, .. } => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
