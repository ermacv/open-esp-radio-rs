//! Public facts produced by artifact loading and decoding.

use rv_asm::Inst;

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

pub(super) fn riscv_relocation_kind(r_type: u32) -> Option<RelocationKind> {
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
