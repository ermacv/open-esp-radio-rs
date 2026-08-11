//! Public facts produced by artifact loading and decoding.

use rv_asm::Inst;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct ArtifactSymbolDefinition {
    pub member: Option<String>,
    pub name: String,
    pub address: u64,
    pub bytes: Vec<u8>,
    pub addresses_resolved: bool,
    /// Immutable image layout shared by every symbol from the same object.
    ///
    /// A linked image commonly contributes thousands of functions but only a
    /// handful of memory regions. Keeping a full `Vec` in every cloned symbol
    /// made artifact-wide analysis copy the same strings millions of times.
    pub memory_regions: Arc<[MemoryRegion]>,
    pub relocations: Vec<SymbolRelocation>,
}

/// Sized data symbol from an executable image. This is used only to recover
/// object-relative memory provenance; it makes no nominal-type claim.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactDataSymbolDefinition {
    pub member: Option<String>,
    pub name: String,
    pub address: u32,
    pub size: u32,
    pub exported: bool,
}

/// Named static data object from an ELF image or relocatable archive member.
///
/// Archive members do not have a runtime address, so their stable identity is
/// the member/section/symbol tuple plus the section-relative object offset.
/// Initializer bytes are the uninterpreted object representation; relocations
/// retain symbolic targets instead of pretending that archive layout is link
/// truth.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactDataObjectDefinition {
    pub member: Option<String>,
    pub section: String,
    pub name: String,
    /// Other symbols at the exact same section offset, commonly compiler
    /// generated `.LANCHOR*` relocation targets.
    pub aliases: Vec<String>,
    pub address: Option<u32>,
    pub object_offset: u64,
    pub size: u64,
    pub writable: bool,
    pub initialized: bool,
    /// True when a zero-sized ELF anchor is the only identity for the
    /// remaining section bytes.
    pub synthetic_from_anchor: bool,
    pub exported: bool,
    pub initializer: Vec<u8>,
    pub relocations: Vec<ArtifactDataObjectRelocation>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ArtifactDataObjectRelocation {
    pub offset: u64,
    pub elf_type: Option<u32>,
    pub target: String,
    pub addend: i64,
}

/// A human-reviewed function range inside an executable section.
///
/// Offsets are section-relative so the identity remains stable for
/// relocatable archive members whose section addresses are zero.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ReviewedCodeRange {
    pub member: Option<String>,
    pub section: String,
    pub name: String,
    pub start_offset: u64,
    pub end_offset: u64,
}

/// Which named, sized text symbols should become analysis roots.
///
/// This is deliberately separate from [`ArtifactSymbolScope`], which records
/// the ELF symbol-table scope of an individual symbol.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum CodeSymbolSelection {
    /// Global and weak definitions that participate in external linkage.
    Exported,
    /// Every named definition, including local/private implementation details.
    All,
}

impl CodeSymbolSelection {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Exported => "exported",
            Self::All => "all",
        }
    }

    pub const fn includes_local(self) -> bool {
        matches!(self, Self::All)
    }
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ArtifactCodeRange {
    /// Offset from the beginning of the containing executable section.
    pub start_offset: u64,
    /// Exclusive offset from the beginning of the containing section.
    pub end_offset: u64,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ArtifactDirectControlFlowKind {
    Call,
    TailCall,
}

impl ArtifactDirectControlFlowKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Call => "call",
            Self::TailCall => "tail-call",
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ArtifactDirectControlFlowEvidence {
    pub caller: String,
    /// Section-relative offset of the JAL instruction.
    pub site_offset: u64,
    pub kind: ArtifactDirectControlFlowKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactFunctionBoundaryCandidate {
    /// Section-relative candidate entry address.
    pub entry_offset: u64,
    /// Exclusive upper bound inferred from the next candidate or covered range.
    pub end_limit_offset: u64,
    /// Zero-sized function symbols anchored at this entry, if any.
    pub symbol_names: Vec<String>,
    /// Direct linked calls from sized code into this uncovered entry.
    pub direct_control_flow: Vec<ArtifactDirectControlFlowEvidence>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactCodeRecoveryBlocker {
    pub symbol: String,
    pub message: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactCodeSectionCoverage {
    pub name: String,
    pub address: u64,
    pub size: u64,
    pub named_sized_symbols: usize,
    pub named_zero_sized_symbols: usize,
    pub symbol_covered_bytes: u64,
    pub uncovered_ranges: Vec<ArtifactCodeRange>,
    pub function_candidates: Vec<ArtifactFunctionBoundaryCandidate>,
    pub recovery_blockers: Vec<ArtifactCodeRecoveryBlocker>,
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
    pub code_sections: Vec<ArtifactCodeSectionCoverage>,
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

/// Decoder-independent classification for an instruction that the current
/// RV32 semantic backend cannot lift.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnsupportedInstructionClass {
    ZeroFillOrIllegalTrap,
    FloatingPoint,
    FloatingPointCsr,
    Csr,
    VendorCsr,
    System,
    VendorCustom,
    OtherExtension,
    Invalid,
}

impl UnsupportedInstructionClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ZeroFillOrIllegalTrap => "zero-fill-or-illegal-trap",
            Self::FloatingPoint => "floating-point",
            Self::FloatingPointCsr => "floating-point-csr",
            Self::Csr => "csr",
            Self::VendorCsr => "vendor-csr",
            Self::System => "system",
            Self::VendorCustom => "vendor-custom",
            Self::OtherExtension => "other-extension",
            Self::Invalid => "invalid",
        }
    }
}

/// One architecturally sized instruction that the current decoder cannot
/// lift. Raw bytes and their exact PC remain available as review evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UnsupportedInstruction {
    pub address: u64,
    pub width: u8,
    pub raw: u32,
    pub class: UnsupportedInstructionClass,
    pub integer_destination: Option<u8>,
    pub linear_control_flow: bool,
}

impl std::fmt::Display for UnsupportedInstruction {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "decode-blocker class={} pc={:#x} width={} raw={:#010x}",
            self.class.as_str(),
            self.address,
            self.width,
            self.raw
        )
    }
}

/// Loss-tolerant instruction stream used only by structural analysis.
/// Concrete execution continues to require [`DecodedInstruction`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AnalysisInstruction {
    Supported(DecodedInstruction),
    Unsupported(UnsupportedInstruction),
}

impl AnalysisInstruction {
    pub const fn address(self) -> u64 {
        match self {
            Self::Supported(instruction) => instruction.address,
            Self::Unsupported(instruction) => instruction.address,
        }
    }

    pub const fn width(self) -> u8 {
        match self {
            Self::Supported(instruction) => instruction.width,
            Self::Unsupported(instruction) => instruction.width,
        }
    }

    pub const fn supported(self) -> Option<DecodedInstruction> {
        match self {
            Self::Supported(instruction) => Some(instruction),
            Self::Unsupported(_) => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutableSection {
    pub name: String,
    pub address: u64,
    pub bytes: Vec<u8>,
}
